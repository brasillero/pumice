//! Validated `formatter.yml` configuration.
//!
//! Explicit `--config FILE` wins and a missing explicit file is an error.
//! Otherwise the per-user file is looked up (`formatter.yml` next to the
//! other module configs). A missing per-user file is fine: the CLI then only
//! knows the built-in formatters.
//!
//! The config registers formatter definition files (paths relative to the
//! config file) and an optional `default`. Definition files are validated
//! with `file:line:column`; an error inside a definition file points into
//! that file, while a missing or unregistered file is reported at the
//! `path:` entry in `formatter.yml`.

mod error;
mod raw;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub use error::ConfigError;

use raw::RawConfig;

use crate::builtins::BUILTIN_CLEAN;
use crate::definition::{self, FormatterDefinition};

const PER_USER_FILE: &str = "formatter.yml";

/// Validated configuration settings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    /// Registered formatters, in config order.
    pub formatters: Vec<FormatterEntry>,
    /// Optional default formatter id (a registered id or `builtin:clean`).
    pub default: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatterEntry {
    pub definition: FormatterDefinition,
    /// The definition file this formatter was loaded from.
    pub path: PathBuf,
}

impl Config {
    /// Looks up a registered formatter by id (built-ins are not part of the
    /// config; the caller resolves them first).
    pub fn definition(&self, id: &str) -> Option<&FormatterDefinition> {
        self.formatters
            .iter()
            .map(|entry| &entry.definition)
            .find(|definition| definition.id == id)
    }
}

/// Where the effective configuration came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigSource {
    File(PathBuf),
    NoFile,
}

/// The effective configuration and the file it came from, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedConfig {
    pub config: Config,
    pub source: ConfigSource,
}

/// Loads the configuration: an explicit `--config` path when given, else the
/// per-user configuration file, else an empty config.
pub fn load(explicit: Option<&Path>) -> Result<LoadedConfig, ConfigError> {
    load_with_env(explicit, |key| std::env::var_os(key))
}

/// [`load`] with an injectable environment reader, so tests can point the
/// per-user lookup at a temporary directory without touching the process
/// environment.
pub fn load_with_env(
    explicit: Option<&Path>,
    get_env: impl Fn(&str) -> Option<OsString>,
) -> Result<LoadedConfig, ConfigError> {
    let source = match explicit {
        Some(path) => {
            if !path.exists() {
                return Err(ConfigError::file(
                    path,
                    "the configuration file does not exist",
                ));
            }
            ConfigSource::File(path.to_path_buf())
        }
        None => match per_user_config_path(&get_env) {
            Some(candidate) => match std::fs::symlink_metadata(&candidate) {
                Ok(_) => ConfigSource::File(candidate),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => ConfigSource::NoFile,
                Err(error) => {
                    return Err(ConfigError::file(
                        candidate,
                        format!("cannot inspect configuration file: {error}"),
                    ));
                }
            },
            None => ConfigSource::NoFile,
        },
    };
    let config = match &source {
        ConfigSource::File(path) => load_file(path)?,
        ConfigSource::NoFile => Config::default(),
    };
    Ok(LoadedConfig { config, source })
}

fn load_file(path: &Path) -> Result<Config, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        ConfigError::file(path, format!("cannot read configuration file: {error}"))
    })?;
    let config_dir = config_dir_of(path);
    let raw = serde_saphyr::from_str_with_options::<Option<RawConfig>>(&text, saphyr_options())
        .map_err(|error| parse_error(path, &error))?;
    validate(raw.unwrap_or_default(), path, Some(&config_dir)).map_err(|error| error.with_file(path))
}

fn validate(raw: RawConfig, config_path: &Path, config_dir: Option<&Path>) -> Result<Config, ConfigError> {
    match &raw.schema_version {
        None => {
            return Err(ConfigError::general(
                "schema_version is required (must be 1)",
            ));
        }
        Some(span) if span.value != 1 => {
            return Err(ConfigError::at(
                span.referenced,
                format!("schema_version must be 1, got {}", span.value),
            ));
        }
        _ => {}
    }

    let mut formatters: Vec<FormatterEntry> = Vec::new();
    for entry in &raw.formatters {
        let entry_location = entry.referenced;
        let entry = &entry.value;
        let path = match &entry.path {
            Some(path) => path,
            None => {
                return Err(ConfigError::at(entry_location, "formatter entry has no \"path\""));
            }
        };
        if path.value.as_os_str().is_empty() {
            return Err(ConfigError::at(
                path.referenced,
                "formatter path must not be empty",
            ));
        }
        let resolved = resolve_against(&path.value, config_dir);
        if !resolved.is_file() {
            // The file itself is missing: report at the `path:` entry of
            // this configuration file.
            return Err(ConfigError::at_file(
                config_path,
                (path.referenced.line() != 0).then(|| path.referenced.line()),
                (path.referenced.column() != 0).then(|| path.referenced.column()),
                format!("formatter definition file does not exist: {}", resolved.display()),
            ));
        }
        let (id_location, definition) = definition::load_definition_file_located(&resolved)?;
        if formatters
            .iter()
            .any(|existing| existing.definition.id == definition.id)
        {
            return Err(ConfigError::at(
                id_location,
                format!("duplicate formatter id \"{}\"", definition.id),
            )
            .with_file(&resolved));
        }
        formatters.push(FormatterEntry {
            definition,
            path: resolved,
        });
    }

    let default = match &raw.default {
        Some(span) => {
            let known = span.value == BUILTIN_CLEAN
                || formatters.iter().any(|entry| entry.definition.id == span.value);
            if !known {
                return Err(ConfigError::at(
                    span.referenced,
                    format!(
                        "default \"{}\" does not name a formatter (configured ids or {BUILTIN_CLEAN})",
                        span.value
                    ),
                ));
            }
            Some(span.value.clone())
        }
        None => None,
    };

    Ok(Config { formatters, default })
}

/// Directory that relative paths in the file resolve against.
fn config_dir_of(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    match absolute.parent() {
        Some(parent) => parent.to_path_buf(),
        None => PathBuf::from("."),
    }
}

/// Resolves a relative path against the configuration file's directory.
fn resolve_against(path: &Path, config_dir: Option<&Path>) -> PathBuf {
    match config_dir {
        Some(dir) if path.is_relative() => dir.join(path),
        _ => path.to_path_buf(),
    }
}

/// Shared YAML parsing options: strict, with positions for error reporting.
pub(crate) fn saphyr_options() -> serde_saphyr::Options {
    serde_saphyr::options! {
        reject_unsupported_tags: true,
        with_snippet: false,
    }
}

/// Maps a saphyr parse error to a [`ConfigError`] with file:line:column.
pub(crate) fn parse_error(path: &Path, error: &serde_saphyr::Error) -> ConfigError {
    use serde_saphyr::Error as SaphyrError;

    let message = match error {
        SaphyrError::MultipleDocuments { .. } => {
            "multiple YAML documents are not allowed; use a single document".to_owned()
        }
        SaphyrError::DuplicateMappingKey { key: Some(key), .. } => format!("duplicate key `{key}`"),
        SaphyrError::DuplicateMappingKey { .. } => "duplicate key".to_owned(),
        SaphyrError::SerdeUnknownField { field, .. } => format!("unknown setting `{field}`"),
        SaphyrError::InvalidScalar { ty, .. } => format!("invalid {ty}"),
        SaphyrError::SerdeInvalidType {
            unexpected,
            expected,
            ..
        } => format!("expected {expected}, found {unexpected}"),
        SaphyrError::SerdeInvalidValue {
            unexpected,
            expected,
            ..
        } => format!("invalid value: expected {expected}, found {unexpected}"),
        SaphyrError::Eof { .. } => "unexpected end of input".to_owned(),
        SaphyrError::Unexpected { expected, .. } => format!("expected {expected}"),
        SaphyrError::UnsupportedTag { tag, .. } => format!("unsupported tag `{tag}`"),
        SaphyrError::UnknownAnchor { .. } => "alias references an unknown anchor".to_owned(),
        _ => strip_location_suffix(&error.to_string()),
    };
    let position = error
        .location()
        .and_then(|location| (location.line() != 0).then(|| (location.line(), location.column())));
    ConfigError::at_file(
        path,
        position.map(|(line, _)| line),
        position.map(|(_, column)| column),
        message,
    )
}

fn strip_location_suffix(text: &str) -> String {
    match text.rfind(" at line ") {
        Some(index) => text[..index].to_owned(),
        None => text.to_owned(),
    }
}

/// The per-user configuration file, when the platform's conventions place
/// one: `<base>/pumice/formatter.yml` under XDG_CONFIG_HOME, ~/.config or
/// %APPDATA%, mirroring the relay and transcription lookup.
pub fn per_user_config_path(get_env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    per_user_config_path_for(get_env, cfg!(windows))
}

fn per_user_config_path_for(
    get_env: &dyn Fn(&str) -> Option<OsString>,
    windows: bool,
) -> Option<PathBuf> {
    let base = if windows {
        get_env("APPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    } else {
        match get_env("XDG_CONFIG_HOME") {
            Some(value) if !value.is_empty() => {
                let path = PathBuf::from(&value);
                if path.is_absolute() {
                    Some(path)
                } else {
                    home_config_dir(get_env)
                }
            }
            _ => home_config_dir(get_env),
        }
    }?;
    Some(base.join("pumice").join(PER_USER_FILE))
}

fn home_config_dir(get_env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    get_env("HOME").map(|home| PathBuf::from(home).join(".config"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn no_env(_: &str) -> Option<OsString> {
        None
    }

    fn write_config(dir: &Path, text: &str) -> PathBuf {
        let path = dir.join("formatter.yml");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(text.as_bytes()).unwrap();
        path
    }

    fn write_definition(dir: &Path, name: &str, id: &str) -> PathBuf {
        let path = dir.join(name);
        let mut file = std::fs::File::create(&path).unwrap();
        write!(
            file,
            "schema_version: 1\nid: {id}\ndescription: test\nsystem: Rules.\n"
        )
        .unwrap();
        path
    }

    #[test]
    fn valid_config_loads_definitions() {
        let tmp = tempfile::tempdir().unwrap();
        write_definition(tmp.path(), "clean.yml", "clean");
        let path = write_config(
            tmp.path(),
            "schema_version: 1\nformatters:\n  - path: clean.yml\ndefault: clean\n",
        );
        let loaded = load(Some(&path)).unwrap();
        assert_eq!(loaded.source, ConfigSource::File(path));
        assert_eq!(loaded.config.formatters.len(), 1);
        assert_eq!(loaded.config.formatters[0].definition.id, "clean");
        assert_eq!(
            loaded.config.formatters[0].path,
            tmp.path().join("clean.yml")
        );
        assert_eq!(loaded.config.default.as_deref(), Some("clean"));
    }

    #[test]
    fn builtin_default_is_valid_without_registered_formatters() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_config(
            tmp.path(),
            "schema_version: 1\ndefault: builtin:clean\n",
        );
        let loaded = load(Some(&path)).unwrap();
        assert!(loaded.config.formatters.is_empty());
        assert_eq!(loaded.config.default.as_deref(), Some("builtin:clean"));
    }

    #[test]
    fn missing_definition_file_points_at_the_config_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_config(
            tmp.path(),
            "schema_version: 1\nformatters:\n  - path: gone.yml\n",
        );
        let err = load(Some(&path)).unwrap_err();
        let err = err.to_string();
        assert!(err.contains("formatter definition file does not exist"), "{err}");
        assert!(err.contains("gone.yml"), "{err}");
        assert!(err.contains(":3:"), "the path entry's line: {err}");
    }

    #[test]
    fn duplicate_ids_are_rejected_in_the_second_file() {
        let tmp = tempfile::tempdir().unwrap();
        write_definition(tmp.path(), "a.yml", "clean");
        write_definition(tmp.path(), "b.yml", "clean");
        let path = write_config(
            tmp.path(),
            "schema_version: 1\nformatters:\n  - path: a.yml\n  - path: b.yml\n",
        );
        let err = load(Some(&path)).unwrap_err();
        let err = err.to_string();
        assert!(err.contains("duplicate formatter id \"clean\""), "{err}");
        assert!(
            err.contains(tmp.path().join("b.yml").display().to_string().as_str()),
            "points into the second definition file: {err}"
        );
    }

    #[test]
    fn invalid_definition_reports_the_definition_file() {
        let tmp = tempfile::tempdir().unwrap();
        let bad = tmp.path().join("bad.yml");
        std::fs::write(&bad, "schema_version: 1\nid: \"Bad Id\"\ndescription: x\nsystem: y\n")
            .unwrap();
        let path = write_config(
            tmp.path(),
            "schema_version: 1\nformatters:\n  - path: bad.yml\n",
        );
        let err = load(Some(&path)).unwrap_err();
        let err = err.to_string();
        assert!(err.contains("bad.yml"), "the definition file is named: {err}");
        assert!(err.contains(":2:"), "line inside the definition file: {err}");
    }

    #[test]
    fn dangling_default_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        write_definition(tmp.path(), "clean.yml", "clean");
        let path = write_config(
            tmp.path(),
            "schema_version: 1\nformatters:\n  - path: clean.yml\ndefault: missing\n",
        );
        let err = load(Some(&path)).unwrap_err();
        let err = err.to_string();
        assert!(err.contains("default \"missing\" does not name a formatter"), "{err}");
        assert!(err.contains(":4:"), "the default entry's line: {err}");
    }

    #[test]
    fn unknown_top_level_key_reports_line() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_config(tmp.path(), "schema_version: 1\nunknown: 5\n");
        let err = load(Some(&path)).unwrap_err();
        let err = err.to_string();
        assert!(err.contains("unknown setting `unknown`"), "{err}");
        assert!(err.contains(":2:"), "{err}");
    }

    #[test]
    fn missing_explicit_config_is_error() {
        let path = PathBuf::from("/tmp/does-not-exist-pumice-formatter.yml");
        let err = load(Some(&path)).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn no_file_gives_an_empty_config() {
        let loaded = load_with_env(None, no_env).unwrap();
        assert_eq!(loaded.source, ConfigSource::NoFile);
        assert!(loaded.config.formatters.is_empty());
        assert!(loaded.config.default.is_none());
    }

    #[test]
    fn per_user_path_honours_xdg() {
        let xdg = std::env::temp_dir().join("xdg-formatter");
        let env_value = xdg.clone().into_os_string();
        let path = per_user_config_path_for(
            &|key| (key == "XDG_CONFIG_HOME").then(|| env_value.clone()),
            false,
        );
        assert_eq!(path, Some(xdg.join("pumice").join("formatter.yml")));
    }

    #[test]
    fn per_user_path_uses_appdata_on_windows() {
        let path = per_user_config_path_for(
            &|key| (key == "APPDATA").then(|| OsString::from(r"C:\Users\me\AppData\Roaming")),
            true,
        );
        assert_eq!(
            path,
            Some(
                PathBuf::from(r"C:\Users\me\AppData\Roaming")
                    .join("pumice")
                    .join("formatter.yml")
            )
        );
    }
}
