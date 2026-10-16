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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_formats_file_line_column() {
        let error = ConfigError {
            file: Some(PathBuf::from("transcribe.yml")),
            line: Some(3),
            column: Some(17),
            message: "schema_version must be 1".to_owned(),
        };
        assert_eq!(
            error.to_string(),
            "transcribe.yml:3:17: schema_version must be 1"
        );
    }

    #[test]
    fn display_formats_file_only_and_bare() {
        let file_only = ConfigError::file("transcribe.yml", "cannot read file: nope");
        assert_eq!(
            file_only.to_string(),
            "transcribe.yml: cannot read file: nope"
        );
        let bare = ConfigError::general("nothing at all");
        assert_eq!(bare.to_string(), "nothing at all");
    }

    #[test]
    fn at_ignores_unknown_locations() {
        let error = ConfigError::at(Location::UNKNOWN, "no position");
        assert_eq!(error.to_string(), "no position");
    }
}
