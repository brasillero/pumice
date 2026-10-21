//! Configuration errors with source file, line and column.

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
    /// A semantic problem at a source position.
    pub fn at(location: Location, message: impl Into<String>) -> ConfigError {
        let known = location.line() != 0;
        ConfigError {
            file: None,
            line: known.then(|| location.line()),
            column: known.then(|| location.column()),
            message: message.into(),
        }
    }

    /// A problem at a position no parsed value carries, such as line 1 for a
    /// missing top-level setting.
    pub(crate) fn at_line(line: u64, column: u64, message: impl Into<String>) -> ConfigError {
        ConfigError {
            file: None,
            line: Some(line),
            column: Some(column),
            message: message.into(),
        }
    }

    /// A problem that concerns a whole file.
    pub(crate) fn file(path: impl Into<PathBuf>, message: impl Into<String>) -> ConfigError {
        ConfigError {
            file: Some(path.into()),
            line: None,
            column: None,
            message: message.into(),
        }
    }

    /// A problem with no source position.
    pub(crate) fn general(message: impl Into<String>) -> ConfigError {
        ConfigError {
            file: None,
            line: None,
            column: None,
            message: message.into(),
        }
    }

    /// A problem at an already-extracted position, with the file attached.
    pub(crate) fn at_file(
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
    pub(crate) fn with_file(mut self, path: &Path) -> ConfigError {
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
