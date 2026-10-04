//! OpenAI-compatible response bodies for the routes in [`crate::api`].
//!
//! These types serialize response bodies that carry dictated text; they are
//! never logged, so they implement no `Debug`.

use serde::Serialize;

/// `POST /v1/chat/completions` success body (non-streaming).
#[derive(Serialize)]
pub struct ChatCompletion {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub model: String,
    pub choices: Vec<CompletionChoice>,
}

#[derive(Serialize)]
pub struct CompletionChoice {
    pub index: u32,
    pub message: AssistantMessage,
    pub finish_reason: &'static str,
}

#[derive(Serialize)]
pub struct AssistantMessage {
    pub role: &'static str,
    pub content: String,
}

/// One `data:` line of a streaming completion response.
#[derive(Serialize)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChunkChoice>,
}

#[derive(Serialize)]
pub struct ChunkChoice {
    pub index: u32,
    pub delta: Delta,
    pub finish_reason: Option<&'static str>,
}

/// An empty delta serializes as `{}`, matching OpenAI's final chunk.
#[derive(Serialize)]
pub struct Delta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// `GET /health` body: liveness, so monitors and Handy setups can check the
/// service without touching the pipeline (no CLI call, no quota).
#[derive(Serialize)]
pub struct HealthBody {
    pub status: &'static str,
    pub version: &'static str,
}

/// `GET /v1/models` body: the enabled provider IDs as models.
#[derive(Serialize)]
pub struct ModelList {
    pub object: &'static str,
    pub data: Vec<ModelEntry>,
}

#[derive(Serialize)]
pub struct ModelEntry {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub owned_by: &'static str,
}

/// Error envelope for every non-200 response.
#[derive(Serialize)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Serialize)]
pub struct ErrorDetail {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: &'static str,
}
