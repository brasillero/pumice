pub mod audio;
pub mod cli;
pub mod config;
pub mod engine;
pub mod error;
mod riff;
pub mod server;
pub mod service;

pub use engine::{CancelToken, Engine, FakeEngine};
pub use error::{Error, ErrorCode, JsonError};
pub use service::{
    normalize_language, normalize_threads, resolve_model_id, transcribe_pcm_from_reader,
};

#[derive(Debug, Clone)]
pub struct TranscriptionOptions {
    pub language: Option<String>,
    pub prompt: Option<String>,
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct Transcript {
    pub text: String,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineError {
    Cancelled,
    Failed,
    ModelLoadFailed,
}

impl EngineError {
    pub fn to_error(self) -> Error {
        match self {
            Self::Cancelled => Error::new(
                error::ErrorCode::EngineFailed,
                "transcription was cancelled",
            ),
            Self::Failed => Error::engine_failed(),
            Self::ModelLoadFailed => Error::model_load_failed(),
        }
    }
}
