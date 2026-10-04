//! Handlers for the chat-completions and model-list routes, plus the
//! OpenAI-style fallback for everything else.
//!
//! Dictated text flows through [`chat_completions`] but never into logs:
//! only the outcome metadata line is written. Errors use OpenAI's
//! `{"error":{"message","type"}}` envelope with fixed, text-free messages.

use std::future::poll_fn;
use std::time::Duration;

use axum::Json;
use axum::body::{Body, HttpBody};
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use tokio::time::Instant;

use crate::pipeline::{FormatOutcome, OutcomeKind};
use crate::request::{ChatCompletionRequest, extract_request};
use crate::time::{format_rfc3339, now_unix_secs};

use super::ApiState;
use super::types::*;

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
    // from the body, which may be dictated text.
    let request: ChatCompletionRequest = match serde_json::from_slice(&bytes) {
        Ok(request) => request,
        Err(_) => {
            return openai_error(
                StatusCode::BAD_REQUEST,
                "the request body is not valid JSON",
            );
        }
    };
    let stream = request.stream.unwrap_or(false);

    let extracted = match extract_request(request) {
        Ok(extracted) => extracted,
        Err(error) => return openai_error(StatusCode::BAD_REQUEST, &error.to_string()),
    };

    let outcome = state.pipeline.format(&extracted, started).await;

    // S1.4: the opt-in debug log records request/response payloads here,
    // gated by `debug_log.enabled`; the metadata line below stays text-free.
    state.log.write_line(&completion_line(&outcome));

    let id = format!("chatcmpl-pumice-{}", state.next_id());
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

/// OpenAI-style JSON 404 for anything outside the two routes, so generic
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

/// Builds the single metadata line for one completion request: time, outcome
/// kind, raw reason, provider, elapsed and text length. Never the text.
fn completion_line(outcome: &FormatOutcome) -> String {
    let (kind, reason) = match &outcome.kind {
        OutcomeKind::Formatted => ("formatted", "-".to_owned()),
        OutcomeKind::Empty => ("empty", "-".to_owned()),
        OutcomeKind::Raw(reason) => ("raw", format!("{reason:?}")),
    };
    format!(
        "{} route=chat.completions kind={kind} reason={reason} provider={} elapsed_ms={} text_len={}",
        format_rfc3339(now_unix_secs()),
        outcome.provider.unwrap_or("-"),
        outcome.elapsed.as_millis(),
        outcome.text.len(),
    )
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
