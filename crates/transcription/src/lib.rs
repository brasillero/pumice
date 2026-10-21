pub mod audio;
mod bundle;
pub mod catalog;
pub mod cli;
pub mod compute;
pub mod config;
pub mod download;
pub mod engine;
pub mod error;
pub mod inventory;
pub mod managed;
pub mod models_dir;
#[cfg(feature = "opus")]
mod opus;
mod riff;
pub mod server;
pub mod service;

pub use engine::{CancelToken, Engine, FakeEngine};
pub use error::{Error, ErrorCode, JsonError};
pub use service::{normalize_language, normalize_threads, transcribe_pcm_from_reader};

/// Options passed to the engine. No `Debug`: `prompt` is client text that must
/// never reach a log by accident.
#[derive(Clone, Default)]
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
    /// A native failure that may leave the engine or its device unsafe to
    /// reuse; on a GPU it retires the process (p14.2-backend-policy/D1).
    Failed,
    /// The engine could not transcribe this request because of its content
    /// or input (for example truncated or repeating output) and stays usable.
    Rejected,
    ModelLoadFailed,
}

impl EngineError {
    pub fn to_error(self) -> Error {
        match self {
            Self::Cancelled => Error::cancelled(),
            Self::Failed => Error::engine_failed().with_engine_unusable(),
            Self::Rejected => Error::engine_failed(),
            Self::ModelLoadFailed => Error::model_load_failed(),
        }
    }
}
