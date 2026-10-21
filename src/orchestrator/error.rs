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

    /// A problem at a position no parsed value carries, such as line 1 for a
    /// missing top-level setting.
    pub fn at_line(line: u64, column: u64, message: impl Into<String>) -> ConfigError {
        ConfigError {
            file: None,
            line: Some(line),
            column: Some(column),
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
    /// The request parameter at fault, for an HTTP presentation (one of
    /// the route's own field names, never a client-supplied one).
    pub param: Option<&'static str>,
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
            param: None,
        }
    }

    pub fn with_param(mut self, param: &'static str) -> StageError {
        self.param = Some(param);
        self
    }

    pub fn with_cause(mut self, cause_code: impl Into<String>) -> StageError {
        self.cause_code = Some(cause_code.into().into_boxed_str());
        self
    }

    pub fn set_location(&mut self, location: crate::orchestrator::result::ErrorLocation) {
        self.location = Some(Box::new(location));
    }

    /// Attaches `location` and prefixes the message with it in the
    /// configuration-error style (`file:line:column: message`), so plain
    /// mode names the position as `invalid_config` errors do.
    pub fn locate(&mut self, location: crate::orchestrator::result::ErrorLocation) {
        self.message = ConfigError::at_file(
            location.file.clone(),
            location.line,
            location.column,
            std::mem::take(&mut self.message),
        )
        .to_string();
        self.set_location(location);
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

    /// The operation was cancelled. A signal follows the shell convention
    /// (130 for Ctrl-C, 143 for SIGTERM) and carries no cause; a service's
    /// own reasons keep the code `cancelled` and name themselves in
    /// `cause_code`, so a presenter can tell a shutdown from a client that
    /// went away.
    pub fn cancelled(
        stage: &'static str,
        reason: crate::orchestrator::cancel::CancelReason,
    ) -> StageError {
        use crate::orchestrator::cancel::CancelReason;
        let (message, cause) = match reason {
            CancelReason::Interrupt => ("cancelled by Ctrl-C", None),
            CancelReason::Terminate => ("cancelled by SIGTERM", None),
            CancelReason::Shutdown => (
                "cancelled because the service is shutting down",
                Some("service_shutdown"),
            ),
            CancelReason::ClientDisconnected => (
                "cancelled because the client disconnected",
                Some("client_disconnected"),
            ),
        };
        let error = StageError::new(stage, "cancelled", message, reason.exit_code());
        match cause {
            Some(cause) => error.with_cause(cause),
            None => error,
        }
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

    #[test]
    fn cancellation_reasons_stay_distinguishable() {
        use crate::orchestrator::cancel::CancelReason;
        use crate::orchestrator::result::stage;

        // The P12 signal mapping is unchanged: no cause, shell exit codes.
        let interrupt = StageError::cancelled(stage::RELAY, CancelReason::Interrupt);
        assert_eq!(interrupt.code, "cancelled");
        assert_eq!(interrupt.message, "cancelled by Ctrl-C");
        assert_eq!(interrupt.exit, 130);
        assert!(interrupt.cause_code.is_none());
        let terminate = StageError::cancelled(stage::RELAY, CancelReason::Terminate);
        assert_eq!(terminate.message, "cancelled by SIGTERM");
        assert_eq!(terminate.exit, 143);
        assert!(terminate.cause_code.is_none());

        let shutdown = StageError::cancelled(stage::TRANSCRIPTION, CancelReason::Shutdown);
        assert_eq!(shutdown.code, "cancelled");
        assert_eq!(shutdown.stage, stage::TRANSCRIPTION);
        assert_eq!(shutdown.cause_code.as_deref(), Some("service_shutdown"));
        assert_eq!(shutdown.exit, 1);
        let gone = StageError::cancelled(stage::RELAY, CancelReason::ClientDisconnected);
        assert_eq!(gone.code, "cancelled");
        assert_eq!(gone.cause_code.as_deref(), Some("client_disconnected"));
        assert_eq!(gone.exit, 1);
    }
}
