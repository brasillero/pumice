//! Handlers for the chat-completions, model-list and health routes, plus the
//! OpenAI-style fallback for everything else.
//!
//! Dictated text flows through [`chat_completions`] but never into the
//! ordinary log: only the outcome metadata line is written. The opt-in debug
//! log ([`crate::logging`]) is the one exception, and only when the
//! configuration enables it. The same flag decides whether dictated text and
//! replies are emitted as live monitor events (S4.6). Errors use OpenAI's
//! `{"error":{"message","type"}}` envelope with fixed, text-free messages.
//! With text allowed, the monitor also receives the request as it arrived
//! (`Received`) and the HTTP response as sent (`Sent`) (S4.7).

use std::future::poll_fn;
use std::time::Duration;

use axum::Json;
use axum::body::{Body, HttpBody};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use tokio::time::Instant;

use crate::logging::DebugRecord;
use crate::monitor::{EventKind, ParsedText, capped};
use crate::pipeline::{FormatOutcome, OutcomeKind, Progress};
use crate::providers::diagnostic::Diagnostic;
use crate::request::{ChatCompletionRequest, extract_request};
use crate::time::now_unix_secs;

use super::ApiState;
use super::log_entry::{self, Unanswered};
use super::types::*;

/// Writes the one log entry of a completion request. Each exit path logs
/// through [`reject`](Self::reject) or [`finish`](Self::finish); when the
/// handler future is dropped first (the client disconnected, which also
/// cancels any CLI run), `Drop` logs the request as dropped.
struct EntryGuard {
    state: ApiState,
    number: u64,
    started: Instant,
    logged: bool,
}

impl EntryGuard {
    /// Logs the rejection and builds the error response.
    fn reject(&mut self, status: StatusCode, message: &str) -> Response {
        self.logged = true;
        self.state.log_unanswered(
            self.number,
            Unanswered::Rejected {
                status: status.as_u16(),
                message,
            },
            self.started.elapsed(),
        );
        self.state.emit(
            self.number,
            EventKind::Responded {
                status: status.as_u16(),
                outcome: None,
                detail: capped(message.to_owned()),
                reply: None,
            },
        );
        openai_error(status, message)
    }

    fn finish(
        &mut self,
        requested: Option<&str>,
        status: StatusCode,
        detail: &str,
        reply: Option<String>,
        outcome: &FormatOutcome,
    ) {
        self.logged = true;
        self.state.log_request(self.number, requested, outcome);
        self.state.emit(
            self.number,
            EventKind::Responded {
                status: status.as_u16(),
                outcome: Some(outcome.kind),
                detail: capped(detail.to_owned()),
                reply,
            },
        );
    }
}

impl Drop for EntryGuard {
    fn drop(&mut self) {
        if !self.logged {
            self.state.log_unanswered(
                self.number,
                Unanswered::Disconnected,
                self.started.elapsed(),
            );
            self.state.emit(self.number, EventKind::Dropped);
        }
    }
}

/// Turns pipeline progress into monitor events for one request.
struct RequestProgress<'a> {
    state: &'a ApiState,
    number: u64,
}

impl Progress for RequestProgress<'_> {
    fn queued(&self) {
        self.state.emit(self.number, EventKind::Queued);
    }

    fn started(&self, provider: &'static str, model: &str) {
        self.state.emit(
            self.number,
            EventKind::Started {
                provider,
                model: capped(model.to_owned()),
            },
        );
    }

    /// The diagnostic's `detail` quotes the CLI's output, which can echo the
    /// dictation, so it is kept only when text is allowed.
    fn attempt_ended(&self, attempt: &crate::pipeline::Attempt) {
        let text_allowed = self.state.text_allowed();
        let diagnostic = attempt.diagnostic.as_ref().map(|diagnostic| Diagnostic {
            detail: if text_allowed {
                capped(diagnostic.detail.clone())
            } else {
                String::new()
            },
            ..diagnostic.clone()
        });
        self.state.emit(
            self.number,
            EventKind::AttemptEnded {
                result: attempt.result,
                diagnostic,
            },
        );
    }
}

/// Minimal pre-flight parse: just enough to detect `inspect` before the
/// stricter dictation validation runs. Unknown fields are ignored, so a
/// diagnostic request can carry arbitrary messages, options and shapes.
#[derive(serde::Deserialize)]
struct ModelSelection {
    model: Option<String>,
    stream: Option<bool>,
}

/// POST /v1/chat/completions accepts bodies up to 10 MiB, read within 5 s.
/// Both bounds are enforced before parsing, so a slow or oversized client
/// never eats the formatting budget reserved for the CLI.
const BODY_LIMIT: usize = 10 * 1024 * 1024;
const BODY_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// The client-facing handler: assigns the request number, emits `Arrived`,
/// runs the completion, and (when text is allowed) captures the exact HTTP
/// response for the `Sent` event before returning it unchanged.
pub async fn chat_completions(State(state): State<ApiState>, request: Request) -> Response {
    let started = Instant::now();
    let number = state.next_id();
    let client = stated_client(request.headers());
    state.emit_at(number, started.into_std(), EventKind::Arrived { client });

    let response = complete(state.clone(), request, number, started).await;
    if !state.text_allowed() {
        return response;
    }
    // Every completion body is already in memory (JSON, an error or the
    // whole SSE stream), so collecting it waits for nothing; only copying it
    // into the event adds work before the response goes out.
    let (parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .unwrap_or_default();
    state.emit(
        number,
        EventKind::Sent {
            status: parts.status.as_u16(),
            headers: response_header_pairs(&parts.headers),
            body: capped(String::from_utf8_lossy(&bytes).into_owned()),
        },
    );
    Response::from_parts(parts, Body::from(bytes))
}

/// The body of the completion handler, split out so the public wrapper can
/// capture the response for the monitor after `Responded` is emitted.
async fn complete(state: ApiState, request: Request, number: u64, started: Instant) -> Response {
    // Every request gets its number and exactly one log entry, whatever
    // happens next: the guard logs a rejection, a completion, or (when this
    // future is dropped because the client went away) a dropped request.
    let mut entry = EntryGuard {
        state: state.clone(),
        number,
        started,
        logged: false,
    };
    // Headers must be captured before the request is consumed. The debug log
    // (when enabled) reads them only through `DebugRecord::new`; disabled (the
    // default), nothing is captured for it.
    let debug_headers = state
        .debug_log
        .is_enabled()
        .then(|| request.headers().clone());
    // The live view shows them (credential values masked) when text is
    // allowed.
    let received_headers = state
        .text_allowed()
        .then(|| shown_headers(request.headers()));

    let bytes = match read_body_bounded(request.into_body()).await {
        Ok(bytes) => bytes,
        Err(ReadBodyError::Timeout) => {
            return entry.reject(
                StatusCode::REQUEST_TIMEOUT,
                "reading the request body timed out",
            );
        }
        Err(ReadBodyError::TooLarge) => {
            return entry.reject(
                StatusCode::PAYLOAD_TOO_LARGE,
                "the request body is larger than 10 MiB",
            );
        }
        Err(ReadBodyError::Malformed) => {
            return entry.reject(
                StatusCode::BAD_REQUEST,
                "the request body could not be read",
            );
        }
    };

    if let Some(headers) = received_headers {
        state.emit(
            number,
            EventKind::Received {
                headers,
                body: capped(String::from_utf8_lossy(&bytes).into_owned()),
            },
        );
    }

    // The serde error is deliberately dropped: its messages can quote values
    // from the body, which may be dictated text. First do a minimal parse so
    // `inspect` can be selected without rejecting requests that would fail the
    // normal dictation validation (missing messages, non-string content, etc.).
    // Ignored JSON string fields need not be UTF-8-checked by serde, so
    // validate the entire body before selecting the diagnostic path.
    let selection: ModelSelection = match std::str::from_utf8(&bytes)
        .ok()
        .and_then(|body| serde_json::from_str(body).ok())
    {
        Some(selection) => selection,
        None => {
            return entry.reject(
                StatusCode::BAD_REQUEST,
                "the request body is not valid JSON",
            );
        }
    };
    let stream = selection.stream.unwrap_or(false);

    // The response id comes from the request number, so the opt-in debug log
    // can name the exact completion it records.
    let id = format!("chatcmpl-pumice-{number}");

    // Built-in inspect diagnostic: echo the original request body verbatim,
    // bypassing transcript extraction, the busy guard, provider selection and
    // every AI CLI call. This path still requires valid JSON and respects the
    // body size/time bounds above. `stream` follows the same rule as normal
    // requests: missing or null means false; a non-boolean value is rejected
    // by the initial parse with the same text-free 400.
    if is_inspect_model(selection.model.as_deref()) {
        let echo_text = String::from_utf8(bytes.clone()).expect("request body is valid UTF-8");
        state.emit(
            number,
            EventKind::Parsed {
                model: selection.model.clone().map(capped),
                text: state.text_allowed().then(|| ParsedText {
                    system: vec![],
                    before: String::new(),
                    input: capped(echo_text.clone()),
                    after: String::new(),
                }),
            },
        );
        let outcome = FormatOutcome {
            text: echo_text.clone(),
            kind: OutcomeKind::Inspect,
            provider: None,
            attempts: 0,
            trail: Vec::new(),
            elapsed: started.elapsed(),
        };
        if let Some(headers) = &debug_headers {
            state.debug_log.record(&DebugRecord::new(
                &id,
                debug_body(&bytes),
                headers,
                "",
                &outcome,
            ));
        }
        let reply = state.text_allowed().then(|| capped(outcome.text.clone()));
        entry.finish(
            selection.model.as_deref(),
            StatusCode::OK,
            "inspect",
            reply,
            &outcome,
        );
        let created = now_unix_secs();
        return if stream {
            sse_response(&id, created, "inspect", echo_text)
        } else {
            Json(ChatCompletion {
                id,
                object: "chat.completion",
                created,
                model: "inspect".to_owned(),
                choices: vec![CompletionChoice {
                    index: 0,
                    message: AssistantMessage {
                        role: "assistant",
                        content: echo_text,
                    },
                    finish_reason: "stop",
                }],
            })
            .into_response()
        };
    }

    // Normal dictation path: stricter parsing and transcript extraction.
    let request: ChatCompletionRequest = match serde_json::from_slice(&bytes) {
        Ok(request) => request,
        Err(_) => {
            return entry.reject(
                StatusCode::BAD_REQUEST,
                "the request body is not valid JSON",
            );
        }
    };

    let extracted = match extract_request(request) {
        Ok(extracted) => extracted,
        Err(error) => return entry.reject(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    state.emit(
        number,
        EventKind::Parsed {
            model: extracted.model.clone().map(capped),
            text: state.text_allowed().then(|| ParsedText {
                system: extracted.system_texts.iter().cloned().map(capped).collect(),
                before: capped(extracted.before_text.clone()),
                input: capped(extracted.text.clone()),
                after: capped(extracted.after_text.clone()),
            }),
        },
    );

    let progress = RequestProgress {
        state: &state,
        number,
    };
    let outcome = state
        .pipeline
        .format_observed(&extracted, started, &progress)
        .await;

    // The opt-in debug log (S1.4) records the full exchange — dictation
    // included — only when `debug_log.enabled` turns it on; the metadata
    // line below stays text-free either way.
    if let Some(headers) = &debug_headers {
        state.debug_log.record(&DebugRecord::new(
            &id,
            debug_body(&bytes),
            headers,
            &extracted.raw_text,
            &outcome,
        ));
    }

    // The status and the text-free detail the log line, the live monitor and
    // (for an error) the client all share.
    let (status, detail) = match outcome.kind {
        OutcomeKind::Raw(reason) => (
            log_entry::failure_status(reason),
            log_entry::raw_reason(reason, extracted.model.as_deref()),
        ),
        OutcomeKind::Formatted => (StatusCode::OK, "formatted".to_owned()),
        OutcomeKind::Passthrough => (StatusCode::OK, "passthrough".to_owned()),
        OutcomeKind::Inspect => (StatusCode::OK, "inspect".to_owned()),
        OutcomeKind::Empty => (StatusCode::OK, "empty".to_owned()),
    };
    let reply =
        (status == StatusCode::OK && state.text_allowed()).then(|| capped(outcome.text.clone()));
    entry.finish(extracted.model.as_deref(), status, &detail, reply, &outcome);
    // Could not format: answer with an HTTP error carrying the safe reason.
    // The app keeps and pastes its own transcript, so Pumice never has to
    // pick the transcript out of the app's prompt.
    if status != StatusCode::OK {
        return openai_error(status, &detail);
    }

    let created = now_unix_secs();
    // The producing provider when formatting succeeded; otherwise (the
    // built-in passthrough, an empty transcript) the provider selection
    // resolved to, else the requested string, else a neutral default.
    let model = outcome
        .provider
        .map(str::to_owned)
        .or_else(|| {
            state
                .pipeline
                .select(extracted.model.as_deref())
                .map(str::to_owned)
        })
        .or_else(|| extracted.model.clone())
        .unwrap_or_else(|| "pumice".to_owned());

    if stream {
        sse_response(&id, created, &model, outcome.text)
    } else {
        Json(ChatCompletion {
            id,
            object: "chat.completion",
            created,
            model,
            choices: vec![CompletionChoice {
                index: 0,
                message: AssistantMessage {
                    role: "assistant",
                    content: outcome.text,
                },
                finish_reason: "stop",
            }],
        })
        .into_response()
    }
}

/// Lists the enabled provider IDs as models, in configuration (list)
/// order — the same IDs [`Pipeline::select`](crate::pipeline::Pipeline::select)
/// resolves, so listing and selection cannot drift.
pub async fn list_models(State(state): State<ApiState>) -> Json<ModelList> {
    Json(ModelList {
        object: "list",
        data: state
            .pipeline
            .model_ids()
            .into_iter()
            .map(|id| ModelEntry {
                id: id.to_owned(),
                object: "model",
                created: 0,
                owned_by: "pumice",
            })
            .collect(),
    })
}

/// GET /health: liveness for monitors and Handy setups. Never touches the
/// pipeline, so it stays responsive while a dictation is being formatted.
pub async fn health() -> Json<HealthBody> {
    Json(HealthBody {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// OpenAI-style JSON 404 for anything outside the three routes, so generic
/// clients receive an envelope they already know how to read.
pub async fn not_found() -> Response {
    openai_error(
        StatusCode::NOT_FOUND,
        "the requested resource was not found",
    )
}

enum ReadBodyError {
    Timeout,
    TooLarge,
    Malformed,
}

/// Reads the whole body within [`BODY_READ_TIMEOUT`] and up to
/// [`BODY_LIMIT`]; the cap counts received bytes, so a client advertising a
/// huge `Content-Length` fails at the limit, not after.
async fn read_body_bounded(body: Body) -> Result<Vec<u8>, ReadBodyError> {
    let read = async {
        let mut body = body;
        let mut data = Vec::new();
        loop {
            match poll_fn(|cx| std::pin::Pin::new(&mut body).poll_frame(cx)).await {
                None => return Ok(data),
                Some(Err(_)) => return Err(ReadBodyError::Malformed),
                Some(Ok(frame)) => {
                    if let Ok(chunk) = frame.into_data() {
                        if data.len() + chunk.len() > BODY_LIMIT {
                            return Err(ReadBodyError::TooLarge);
                        }
                        data.extend_from_slice(&chunk);
                    }
                }
            }
        }
    };
    match tokio::time::timeout(BODY_READ_TIMEOUT, read).await {
        Ok(result) => result,
        Err(_) => Err(ReadBodyError::Timeout),
    }
}

/// The app name the request states in its `X-Title` header, trimmed and cut
/// to 32 characters. `None` when the header is absent, empty, or holds
/// anything but printable ASCII (letters, digits, punctuation, spaces).
fn stated_client(headers: &HeaderMap) -> Option<String> {
    let value = headers.get("x-title")?.to_str().ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() || !trimmed.chars().all(|c| c == ' ' || c.is_ascii_graphic()) {
        return None;
    }
    Some(trimmed.chars().take(32).collect())
}

/// Every request header, with credential-related values replaced by
/// `[hidden]`. The order is the `HeaderMap`'s (values of one name stay
/// together), not necessarily the order on the wire. Names are lowercase, as
/// axum gives them.
fn shown_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            let name = name.as_str().to_owned();
            let lower = name.to_lowercase();
            let hidden = ["auth", "cookie", "key", "token", "secret", "session"]
                .iter()
                .any(|needle| lower.contains(needle));
            let value = if hidden {
                "[hidden]".to_owned()
            } else {
                value
                    .to_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|_| String::from_utf8_lossy(value.as_bytes()).into_owned())
            };
            (name, capped(value))
        })
        .collect()
}

/// Every response header as a `(name, value)` pair, for the `Sent` event.
fn response_header_pairs(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            let name = name.as_str().to_owned();
            let value = value
                .to_str()
                .map(str::to_owned)
                .unwrap_or_else(|_| String::from_utf8_lossy(value.as_bytes()).into_owned());
            (name, capped(value))
        })
        .collect()
}

/// The request body as a JSON value for the debug log. The body already
/// parsed once, but a field the typed parse skipped can hold an escape that
/// does not decode into a value (for example a lone surrogate): then a fixed,
/// text-free marker stands in for the body.
fn debug_body(bytes: &[u8]) -> serde_json::Value {
    serde_json::from_slice(bytes).unwrap_or_else(|_| {
        serde_json::Value::String("request body could not be re-parsed".to_owned())
    })
}

fn openai_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(ErrorBody {
            error: ErrorDetail {
                message: message.to_owned(),
                error_type: error_type(status),
            },
        }),
    )
        .into_response()
}

/// OpenAI's error `type` for `status`.
fn error_type(status: StatusCode) -> &'static str {
    if status == StatusCode::TOO_MANY_REQUESTS {
        "rate_limit_error"
    } else if status.is_server_error() {
        "server_error"
    } else {
        "invalid_request_error"
    }
}

/// Whether the request's `model` field selects the built-in `inspect` model.
fn is_inspect_model(model: Option<&str>) -> bool {
    model
        .map(str::trim)
        .is_some_and(|trimmed| trimmed.eq_ignore_ascii_case("inspect"))
}

/// Builds the SSE body: one chunk carrying the complete text, a final empty
/// chunk with `finish_reason: "stop"`, then `[DONE]`. Formatting finishes
/// before the first byte leaves, so the body is built whole.
fn sse_response(id: &str, created: u64, model: &str, text: String) -> Response {
    let chunks = [
        ChatCompletionChunk {
            id: id.to_owned(),
            object: "chat.completion.chunk",
            created,
            model: model.to_owned(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: Delta {
                    role: Some("assistant"),
                    content: Some(text),
                },
                finish_reason: None,
            }],
        },
        ChatCompletionChunk {
            id: id.to_owned(),
            object: "chat.completion.chunk",
            created,
            model: model.to_owned(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: Delta {
                    role: None,
                    content: None,
                },
                finish_reason: Some("stop"),
            }],
        },
    ];
    let mut body = String::new();
    for chunk in &chunks {
        body.push_str("data: ");
        body.push_str(
            &serde_json::to_string(chunk).expect("serializing a chunk cannot fail on Strings"),
        );
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(body))
        .expect("static response parts are valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stated_client_from_x_title() {
        let mut headers = HeaderMap::new();
        assert_eq!(stated_client(&headers), None);

        headers.insert("x-title", "Handy".parse().unwrap());
        assert_eq!(stated_client(&headers), Some("Handy".to_owned()));

        headers.insert("x-title", "  Handy  ".parse().unwrap());
        assert_eq!(stated_client(&headers), Some("Handy".to_owned()));

        headers.insert("x-title", "".parse().unwrap());
        assert_eq!(stated_client(&headers), None);

        headers.insert("x-title", "   ".parse().unwrap());
        assert_eq!(stated_client(&headers), None);

        headers.insert("x-title", "My Dictation App".parse().unwrap());
        assert_eq!(stated_client(&headers), Some("My Dictation App".to_owned()));

        headers.insert(
            "x-title",
            axum::http::HeaderValue::from_bytes("Olá".as_bytes()).unwrap(),
        );
        assert_eq!(stated_client(&headers), None);

        headers.insert("x-title", "Handy\tApp".parse().unwrap());
        assert_eq!(stated_client(&headers), None);

        let long = "a".repeat(50);
        headers.insert("x-title", long.parse().unwrap());
        assert_eq!(stated_client(&headers), Some("a".repeat(32)));
    }

    #[test]
    fn shown_headers_masks_credentials() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "secret".parse().unwrap());
        headers.insert("x-api-key", "secret".parse().unwrap());
        headers.insert("cookie", "session=xyz".parse().unwrap());
        headers.insert("user-agent", "test".parse().unwrap());
        headers.insert("x-title", "Handy".parse().unwrap());

        let shown = shown_headers(&headers);
        assert_eq!(shown.len(), 5);
        assert!(
            shown
                .iter()
                .any(|(n, v)| n == "authorization" && v == "[hidden]")
        );
        assert!(
            shown
                .iter()
                .any(|(n, v)| n == "x-api-key" && v == "[hidden]")
        );
        assert!(shown.iter().any(|(n, v)| n == "cookie" && v == "[hidden]"));
        assert!(shown.iter().any(|(n, v)| n == "user-agent" && v == "test"));
        assert!(shown.iter().any(|(n, v)| n == "x-title" && v == "Handy"));
    }
}
