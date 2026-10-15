//! Typed errors with a stable, log-safe JSON shape for `--json` consumers.
//!
//! Mirrors `pumice-transcription`'s error model: a typed [`ErrorCode`], a
//! display message that never contains request text (dictated content), and a
//! [`JsonError`] envelope field. Exit-code mapping lives in `cli`.

use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    InvalidArguments,
    InvalidInput,
    InputTooLarge,
    InvalidConfig,
    FormatterNotFound,
    FormatterInstructionConflict,
    Internal,
}

impl ErrorCode {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::InvalidArguments => "invalid_arguments",
            Self::InvalidInput => "invalid_input",
            Self::InputTooLarge => "input_too_large",
            Self::InvalidConfig => "invalid_config",
            Self::FormatterNotFound => "formatter_not_found",
            Self::FormatterInstructionConflict => "formatter_instruction_conflict",
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
        }
    }

    pub fn invalid_arguments(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArguments, message)
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidInput, message)
    }

    pub fn input_too_large(bytes: usize) -> Self {
        Self::new(
            ErrorCode::InputTooLarge,
            format!(
                "the request is too large ({bytes} bytes; maximum is 1,048,576 bytes)"
            ),
        )
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
        }
    }

    pub fn formatter_not_found(id: &str) -> Self {
        Self::new(
            ErrorCode::FormatterNotFound,
            format!("unknown formatter \"{id}\"; run with `list` to see the available formatters"),
        )
    }

    pub fn formatter_instruction_conflict() -> Self {
        Self::new(
            ErrorCode::FormatterInstructionConflict,
            "the request already has non-empty system/developer instructions; \
             retry with --on-conflict append to keep them and add the formatter after them",
        )
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
