//! Orchestrator errors with source file, line and column, mirroring the
//! module configuration errors (`file:line:column: message` when the
//! position is known, `file: message` for whole-file problems, or the bare
//! message otherwise).

use std::fmt;
use std::path::{Path, PathBuf};

use serde_saphyr::Location;

/// One configuration problem.
#[derive(Debug)]
pub struct ConfigError {
    file: Option<PathBuf>,
    line: Option<u64>,
    column: Option<u64>,
    message: String,
}

impl ConfigError {
    /// A semantic problem at a source position. Spans can be lost (for
    /// example through anchors or serde buffering), in which case
    /// `Location::UNKNOWN` degrades this to a message without a position.
    pub fn at(location: Location, message: impl Into<String>) -> ConfigError {
        let known = location.line() != 0;
        ConfigError {
            file: None,
            line: known.then(|| location.line()),
            column: known.then(|| location.column()),
            message: message.into(),
        }
    }

    /// A problem that concerns a whole file (read failures have no position).
    pub fn file(path: impl Into<PathBuf>, message: impl Into<String>) -> ConfigError {
        ConfigError {
            file: Some(path.into()),
            line: None,
            column: None,
            message: message.into(),
        }
    }

    /// A problem with no source position at all.
    pub fn general(message: impl Into<String>) -> ConfigError {
        ConfigError {
            file: None,
            line: None,
            column: None,
            message: message.into(),
        }
    }

    /// A problem at an already-extracted position, with the file attached.
    /// Used when translating parser errors, which keep their position in a
    /// different shape than [`Location`].
    pub fn at_file(
        file: impl Into<PathBuf>,
        line: Option<u64>,
        column: Option<u64>,
        message: impl Into<String>,
    ) -> ConfigError {
        ConfigError {
            file: Some(file.into()),
            line,
            column,
            message: message.into(),
        }
    }

    /// Attaches `path` unless the error already names a file.
    pub fn with_file(mut self, path: &Path) -> ConfigError {
        if self.file.is_none() {
            self.file = Some(path.to_path_buf());
        }
        self
    }

    pub fn path(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    pub fn line(&self) -> Option<u64> {
        self.line
    }

    pub fn column(&self) -> Option<u64> {
        self.column
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.file, self.line) {
            (Some(file), Some(line)) => write!(
                f,
                "{}:{line}:{}: {}",
                file.display(),
                self.column.unwrap_or(1),
                self.message
            ),
            (Some(file), None) => write!(f, "{}: {}", file.display(), self.message),
            (None, _) => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for ConfigError {}

/// A stage failure inside a `pumice run` execution: a stable code, a fixed
/// safe message and the process exit code it maps to (D7).
///
/// Messages never contain dictated text, transcripts or audio; configuration
/// errors may name the configuration file and setting location. The optional
/// parts are boxed to keep `Result<_, StageError>` small.
#[derive(Debug)]
pub struct StageError {
    pub stage: &'static str,
    pub code: &'static str,
    pub message: String,
    pub cause_code: Option<Box<str>>,
    pub location: Option<Box<crate::orchestrator::result::ErrorLocation>>,
    pub exit: u8,
}

impl StageError {
    pub fn new(
        stage: &'static str,
        code: &'static str,
        message: impl Into<String>,
        exit: u8,
    ) -> StageError {
        StageError {
            stage,
            code,
            message: message.into(),
            cause_code: None,
            location: None,
            exit,
        }
    }

    pub fn with_cause(mut self, cause_code: impl Into<String>) -> StageError {
        self.cause_code = Some(cause_code.into().into_boxed_str());
        self
    }

    pub fn set_location(&mut self, location: crate::orchestrator::result::ErrorLocation) {
        self.location = Some(Box::new(location));
    }

    /// A configuration error with its `file:line:column` position, shared by
    /// the orchestrator's own config errors and the modules'.
    pub fn invalid_config(error: &ConfigError) -> StageError {
        let message = error.to_string();
        let mut stage_error = StageError::new(
            crate::orchestrator::result::stage::PREPARE,
            "invalid_config",
            message,
            2,
        );
        if let Some(file) = error.path() {
            stage_error.set_location(crate::orchestrator::result::ErrorLocation {
                file: file.to_path_buf(),
                line: error.line(),
                column: error.column(),
            });
        }
        stage_error
    }

    /// The run was cancelled by a signal; the exit code follows the shell
    /// convention (130 for Ctrl-C, 143 for SIGTERM).
    pub fn cancelled(
        stage: &'static str,
        reason: crate::orchestrator::cancel::CancelReason,
    ) -> StageError {
        let message = match reason {
            crate::orchestrator::cancel::CancelReason::Interrupt => "cancelled by Ctrl-C",
            crate::orchestrator::cancel::CancelReason::Terminate => "cancelled by SIGTERM",
        };
        StageError::new(stage, "cancelled", message, reason.exit_code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_formats_file_line_column_and_bare() {
        let error = ConfigError::at_file(
            "orchestrator.yml",
            Some(7),
            Some(3),
            "flow id must not be empty",
        );
        assert_eq!(
            error.to_string(),
            "orchestrator.yml:7:3: flow id must not be empty"
        );
        let file_only = ConfigError::file("orchestrator.yml", "cannot read");
        assert_eq!(file_only.to_string(), "orchestrator.yml: cannot read");
        let bare = ConfigError::general("nothing at all");
        assert_eq!(bare.to_string(), "nothing at all");
    }
}
