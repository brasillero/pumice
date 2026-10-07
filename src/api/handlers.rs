//! Handlers for the chat-completions, model-list and health routes, plus the
//! OpenAI-style fallback for everything else.
//!
//! Dictated text flows through [`chat_completions`] but never into the
//! ordinary log: only the outcome metadata line is written. The opt-in debug
//! log ([`crate::logging`]) is the one exception, and only when the
//! configuration enables it. Errors use OpenAI's
//! `{"error":{"message","type"}}` envelope with fixed, text-free messages.

use std::future::poll_fn;
use std::time::Duration;

use axum::Json;
use axum::body::{Body, HttpBody};
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use tokio::time::Instant;

use crate::logging::DebugRecord;
use crate::pipeline::{FormatOutcome, OutcomeKind};
use crate::request::{ChatCompletionRequest, extract_request};
use crate::time::now_unix_secs;

use super::ApiState;
use super::types::*;

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

/// Formats one dictation and answers 200, non-streaming or SSE. Requests
/// that are not usable dictations get a 400; failures after extraction
/// return the raw dictation as a normal completion.
pub async fn chat_completions(State(state): State<ApiState>, request: Request) -> Response {
    let started = Instant::now();
    // The debug log (when enabled) is the only reader of request headers, and
    // only through `DebugRecord::new`; the map must be captured before the
    // request is consumed. Disabled (the default): nothing is captured.
    let debug_headers = state
        .debug_log
        .is_enabled()
        .then(|| request.headers().clone());

    let bytes = match read_body_bounded(request.into_body()).await {
        Ok(bytes) => bytes,
        Err(ReadBodyError::Timeout) => {
            return openai_error(
                StatusCode::REQUEST_TIMEOUT,
                "reading the request body timed out",
            );
        }
        Err(ReadBodyError::TooLarge) => {
            return openai_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "the request body is larger than 10 MiB",
            );
        }
        Err(ReadBodyError::Malformed) => {
            return openai_error(
                StatusCode::BAD_REQUEST,
                "the request body could not be read",
            );
        }
    };

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
            return openai_error(
                StatusCode::BAD_REQUEST,
                "the request body is not valid JSON",
            );
        }
    };
    let stream = selection.stream.unwrap_or(false);

    // The response id is minted before any processing so the opt-in debug log
    // can name the exact completion it records.
    let number = state.next_id();
    let id = format!("chatcmpl-pumice-{number}");

    // Built-in inspect diagnostic: echo the original request body verbatim,
    // bypassing transcript extraction, the busy guard, provider selection and
    // every AI CLI call. This path still requires valid JSON and respects the
    // body size/time bounds above. `stream` follows the same rule as normal
    // requests: missing or null means false; a non-boolean value is rejected
    // by the initial parse with the same text-free 400.
    if is_inspect_model(selection.model.as_deref()) {
        let echo_text = String::from_utf8(bytes.clone()).expect("request body is valid UTF-8");
        let outcome = FormatOutcome {
            text: echo_text.clone(),
            kind: OutcomeKind::Inspect,
            provider: None,
            attempts: 0,
            trail: Vec::new(),
            elapsed: started.elapsed(),
        };
        if let Some(headers) = &debug_headers {
            // A skipped field can contain an escape that cannot be decoded
            // into a JSON value. Keep that error controlled and text-free.
            let body = match serde_json::from_slice(&bytes) {
                Ok(body) => body,
                Err(_) => {
                    return openai_error(
                        StatusCode::BAD_REQUEST,
                        "the request body is not valid JSON",
                    );
                }
            };
            state
                .debug_log
                .record(&DebugRecord::new(&id, body, headers, "", &outcome));
        }
        state.log_request(number, selection.model.as_deref(), &outcome);
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
            return openai_error(
                StatusCode::BAD_REQUEST,
                "the request body is not valid JSON",
            );
        }
    };

    let extracted = match extract_request(request) {
        Ok(extracted) => extracted,
        Err(error) => return openai_error(StatusCode::BAD_REQUEST, &error.to_string()),
    };

    let outcome = state.pipeline.format(&extracted, started).await;

    // The opt-in debug log (S1.4) records the full exchange — dictation
    // included — only when `debug_log.enabled` turns it on; the metadata
    // line below stays text-free either way.
    if let Some(headers) = &debug_headers {
        // The body already parsed as a request, so it parses as JSON too;
        // unknown client fields are preserved by the `Value` round trip.
        let body = serde_json::from_slice(&bytes).expect("request body already parsed once");
        state.debug_log.record(&DebugRecord::new(
            &id,
            body,
            headers,
            &extracted.raw_text,
            &outcome,
        ));
    }
    state.log_request(number, extracted.model.as_deref(), &outcome);

    let created = now_unix_secs();
    // The producing provider when formatting succeeded; otherwise the
    // provider selection resolved to (on a raw fallback this names the
    // selected provider), else the requested string, else a neutral
    // fallback.
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

/// Lists the enabled provider IDs as models, default first — the same IDs
/// [`Pipeline::select`](crate::pipeline::Pipeline::select) resolves, so
/// listing and selection cannot drift.
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

fn openai_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(ErrorBody {
            error: ErrorDetail {
                message: message.to_owned(),
                error_type: "invalid_request_error",
            },
        }),
    )
        .into_response()
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
