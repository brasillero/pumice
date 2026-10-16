use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    InvalidArguments,
    InvalidInput,
    InputTooLarge,
    AudioTooLong,
    UnsupportedFormat,
    InvalidConfig,
    EngineNotAvailable,
    EngineFailed,
    ModelLoadFailed,
    ModelNotFound,
    GpuUnavailable,
    ChecksumMismatch,
    DownloadFailed,
    Cancelled,
    Internal,
}

impl ErrorCode {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::InvalidArguments => "invalid_arguments",
            Self::InvalidInput => "invalid_input",
            Self::InputTooLarge => "input_too_large",
            Self::AudioTooLong => "audio_too_long",
            Self::UnsupportedFormat => "unsupported_format",
            Self::InvalidConfig => "invalid_config",
            Self::EngineNotAvailable => "engine_not_available",
            Self::EngineFailed => "engine_failed",
            Self::ModelLoadFailed => "model_load_failed",
            Self::ModelNotFound => "model_not_found",
            Self::GpuUnavailable => "gpu_unavailable",
            Self::ChecksumMismatch => "checksum_mismatch",
            Self::DownloadFailed => "download_failed",
            Self::Cancelled => "cancelled",
            Self::Internal => "internal",
        }
    }
}

impl Serialize for ErrorCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct JsonLocation {
    pub file: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JsonError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<JsonLocation>,
}

#[derive(Debug, Clone)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
    pub location: Option<ErrorLocation>,
    /// See [`Error::engine_unusable`].
    engine_unusable: bool,
}

#[derive(Debug, Clone)]
pub struct ErrorLocation {
    pub file: PathBuf,
    pub line: Option<u64>,
    pub column: Option<u64>,
}

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            location: None,
            engine_unusable: false,
        }
    }

    /// Marks a native failure that may have left the engine unsafe to reuse.
    pub(crate) fn with_engine_unusable(mut self) -> Self {
        self.engine_unusable = true;
        self
    }

    /// Whether the engine that produced this error may be unsafe to reuse
    /// ([`crate::EngineError::Failed`]). A server on a GPU retires then
    /// (p14.2-backend-policy/D1); content and input errors never set it.
    pub fn engine_unusable(&self) -> bool {
        self.engine_unusable
    }

    pub fn invalid_arguments(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArguments, message)
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidInput, message)
    }

    pub fn input_too_large(bytes: u64) -> Self {
        Self::new(
            ErrorCode::InputTooLarge,
            format!(
                "audio file is too large ({} bytes; maximum is 25,000,000 bytes)",
                bytes
            ),
        )
    }

    pub fn audio_too_long(seconds: f64) -> Self {
        Self::new(
            ErrorCode::AudioTooLong,
            format!(
                "audio is too long ({:.2} seconds; maximum is 120 seconds)",
                seconds
            ),
        )
    }

    pub fn unsupported_format(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::UnsupportedFormat, message)
    }

    pub fn invalid_config(config_error: crate::config::ConfigError) -> Self {
        Self {
            code: ErrorCode::InvalidConfig,
            message: config_error.to_string(),
            location: config_error.path().map(|file| ErrorLocation {
                file: file.to_path_buf(),
                line: config_error.line(),
                column: config_error.column(),
            }),
            engine_unusable: false,
        }
    }

    pub fn engine_not_available() -> Self {
        Self::new(
            ErrorCode::EngineNotAvailable,
            "this build has no transcription engine",
        )
    }

    pub fn engine_failed() -> Self {
        Self::new(ErrorCode::EngineFailed, "transcription engine failed")
    }

    pub fn model_load_failed() -> Self {
        Self::new(ErrorCode::ModelLoadFailed, "failed to load model")
    }

    pub fn model_not_found(path: &std::path::Path) -> Self {
        Self::new(
            ErrorCode::ModelNotFound,
            format!("model file not found: {}", path.display()),
        )
    }

    /// `backend: gpu` cannot get a GPU (see [`crate::compute`]); `message`
    /// is one of that module's fixed texts.
    pub fn gpu_unavailable(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::GpuUnavailable, message)
    }

    pub fn checksum_mismatch(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ChecksumMismatch, message)
    }

    pub fn download_failed(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::DownloadFailed, message)
    }

    pub fn cancelled() -> Self {
        Self::new(ErrorCode::Cancelled, "transcription was cancelled")
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }

    pub fn to_json_error(&self) -> JsonError {
        JsonError {
            code: self.code.as_str().to_string(),
            message: self.message.clone(),
            location: self.location.as_ref().map(|loc| JsonLocation {
                file: loc.file.clone(),
                line: loc.line,
                column: loc.column,
            }),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Self::invalid_input(format!("I/O error: {err}"))
    }
}

impl From<crate::EngineError> for Error {
    fn from(err: crate::EngineError) -> Self {
        err.to_error()
    }
}
