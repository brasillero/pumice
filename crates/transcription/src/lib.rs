pub mod audio;
pub mod cli;
pub mod engine;
pub mod error;

pub use engine::{Engine, FakeEngine};
pub use error::{Error, ErrorCode, JsonError};

#[derive(Debug, Clone)]
pub struct TranscriptionOptions {
    pub language: Option<String>,
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
