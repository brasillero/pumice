//! Configuration errors: what is wrong and where, in a user-facing form.
//!
//! Messages name the setting path and the problem; they never contain
//! prompts, environment values or option values.

use std::fmt;
use std::path::{Path, PathBuf};

use serde_saphyr::Location;

/// One configuration problem.
///
/// `Display` renders `file:line:column: message` when the position inside
/// the file is known, `file: message` for whole-file problems (such as read
/// failures, which have no YAML position), or just the message otherwise.
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
    /// Provider descriptors use this to compose their own located errors.
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
    pub(crate) fn file(path: impl Into<PathBuf>, message: impl Into<String>) -> ConfigError {
        ConfigError {
            file: Some(path.into()),
            line: None,
            column: None,
            message: message.into(),
        }
    }

    /// A problem with no source position at all.
    pub(crate) fn general(message: impl Into<String>) -> ConfigError {
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
            file: Some(PathBuf::from("pumice.yaml")),
            line: Some(12),
            column: Some(19),
            message: "providers.codex.timeout_secs must be greater than zero".to_owned(),
        };
        assert_eq!(
            error.to_string(),
            "pumice.yaml:12:19: providers.codex.timeout_secs must be greater than zero"
        );
    }

    #[test]
    fn display_formats_file_only_and_bare() {
        let file_only = ConfigError::file("pumice.yaml", "cannot read configuration file: nope");
        assert_eq!(
            file_only.to_string(),
            "pumice.yaml: cannot read configuration file: nope"
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
