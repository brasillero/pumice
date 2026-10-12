use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    InvalidArguments,
    InvalidInput,
    InputTooLarge,
    AudioTooLong,
    UnsupportedFormat,
    EngineNotAvailable,
    EngineFailed,
    ModelLoadFailed,
    ModelNotFound,
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
            Self::EngineNotAvailable => "engine_not_available",
            Self::EngineFailed => "engine_failed",
            Self::ModelLoadFailed => "model_load_failed",
            Self::ModelNotFound => "model_not_found",
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
pub struct JsonError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
}

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
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

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }

    pub fn to_json_error(&self) -> JsonError {
        JsonError {
            code: self.code.as_str().to_string(),
            message: self.message.clone(),
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
