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

    pub fn message(&self) -> &str {
        &self.message
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
