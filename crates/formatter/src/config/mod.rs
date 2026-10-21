//! Validated `formatter.yml` configuration.
//!
//! Explicit `--config FILE` wins; otherwise the per-user file is looked up
//! (`formatter.yml` next to the other module configs). Every setting must
//! be written in the file: an omitted one is an error, never a built-in
//! default (p17-serve-model-names/D3). [`load`] creates missing initial
//! per-user files before validation (auto-initial-settings/D1);
//! [`load_optional`] serves the commands that also run without one (`list`
//! then knows only the built-in formatters).
//!
//! The config registers formatter definition files (paths relative to the
//! config file) and the `default` formatter (or `null` for none).
//! Definition files are validated with `file:line:column`; an error inside a
//! definition file points into that file, while a missing or unregistered
//! file is reported at the `path:` entry in `formatter.yml`.

mod error;
mod raw;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub use error::ConfigError;

use raw::RawConfig;

use crate::builtins::BUILTIN_CLEAN;
use crate::definition::{self, FormatterDefinition};

const PER_USER_FILE: &str = "formatter.yml";

/// Validated configuration settings, every one written in the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Registered formatters, in config order.
    pub formatters: Vec<FormatterEntry>,
    /// Default formatter id (a registered id or `builtin:clean`); `None` is
    /// `default: null` (`apply` then needs `--formatter`).
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

/// The effective configuration and the file it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedConfig {
    pub config: Config,
    pub path: PathBuf,
}

/// Loads the configuration: an explicit `--config` path when given, else the
/// per-user configuration file, creating missing initial per-user files first.
/// A missing explicit file is an error.
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
    if explicit.is_none()
        && let Some(candidate) = per_user_config_path(&get_env)
    {
        pumice_settings::initialize(&candidate)
            .map_err(|error| ConfigError::file(error.path(), error.to_string()))?;
    }
    match locate(explicit, &get_env)? {
        Located::File(path) => load_file(path),
        Located::Absent(Some(candidate)) => Err(ConfigError::file(
            candidate,
            "the formatter configuration file does not exist; pass --config FILE or create \
             it (formatter.example.yml has every setting)",
        )),
        Located::Absent(None) => Err(ConfigError::general(
            "no per-user configuration directory found (XDG_CONFIG_HOME, HOME or APPDATA); \
             pass --config FILE",
        )),
    }
}

/// [`load`] for the commands that also run without a configuration file:
/// `None` when no `--config` is given and the per-user file does not exist.
/// A missing explicit file is still an error, and a file that exists must be
/// complete.
pub fn load_optional(explicit: Option<&Path>) -> Result<Option<LoadedConfig>, ConfigError> {
    load_optional_with_env(explicit, |key| std::env::var_os(key))
}

/// [`load_optional`] with an injectable environment reader.
pub fn load_optional_with_env(
    explicit: Option<&Path>,
    get_env: impl Fn(&str) -> Option<OsString>,
) -> Result<Option<LoadedConfig>, ConfigError> {
    match locate(explicit, &get_env)? {
        Located::File(path) => load_file(path).map(Some),
        Located::Absent(_) => Ok(None),
    }
}

/// Where the configuration file is, or the per-user path it was looked for
/// at (`None` when the platform places none).
enum Located {
    File(PathBuf),
    Absent(Option<PathBuf>),
}

fn locate(
    explicit: Option<&Path>,
    get_env: &dyn Fn(&str) -> Option<OsString>,
) -> Result<Located, ConfigError> {
    if let Some(path) = explicit {
        if !path.exists() {
            return Err(ConfigError::file(
                path,
                "the configuration file does not exist",
            ));
        }
        return Ok(Located::File(path.to_path_buf()));
    }
    let Some(candidate) = per_user_config_path(get_env) else {
        return Ok(Located::Absent(None));
    };
    match std::fs::symlink_metadata(&candidate) {
        Ok(_) => Ok(Located::File(candidate)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(Located::Absent(Some(candidate)))
        }
        Err(error) => Err(ConfigError::file(
            candidate,
            format!("cannot inspect configuration file: {error}"),
        )),
    }
}

fn load_file(path: PathBuf) -> Result<LoadedConfig, ConfigError> {
    let text = std::fs::read_to_string(&path).map_err(|error| {
        ConfigError::file(&path, format!("cannot read configuration file: {error}"))
    })?;
    let config_dir = config_dir_of(&path);
    let raw = serde_saphyr::from_str_with_options::<Option<RawConfig>>(&text, saphyr_options())
        .map_err(|error| parse_error(&path, &error))?;
    let config = validate(raw.unwrap_or_default(), &path, Some(&config_dir))
        .map_err(|error| error.with_file(&path))?;
    Ok(LoadedConfig { config, path })
}

fn validate(
    raw: RawConfig,
    config_path: &Path,
    config_dir: Option<&Path>,
) -> Result<Config, ConfigError> {
    let schema_version = raw.schema_version.required("\"schema_version\"")?;
    if schema_version.value != 1 {
        return Err(ConfigError::at(
            schema_version.referenced,
            format!("schema_version must be 1, got {}", schema_version.value),
        ));
    }

    let entries = raw.formatters.required("\"formatters\"")?;
    // Checked against the formatters below; required here, in file order.
    let default = raw.default.nullable("\"default\"")?;

    let mut formatters: Vec<FormatterEntry> = Vec::new();
    for entry in &entries.value {
        let entry_location = entry.referenced;
        let entry = &entry.value;
        let path = match &entry.path {
            Some(path) => path,
            None => {
                return Err(ConfigError::at(
                    entry_location,
                    "formatter entry has no \"path\"",
                ));
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
                format!(
                    "formatter definition file does not exist: {}",
                    resolved.display()
                ),
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

    let default = match default {
        Some(span) => {
            let known = span.value == BUILTIN_CLEAN
                || formatters
                    .iter()
                    .any(|entry| entry.definition.id == span.value);
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

    Ok(Config {
        formatters,
        default,
    })
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
        assert_eq!(loaded.path, path);
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
            "schema_version: 1\nformatters: []\ndefault: builtin:clean\n",
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
            "schema_version: 1\nformatters:\n  - path: gone.yml\ndefault: null\n",
        );
        let err = load(Some(&path)).unwrap_err();
        let err = err.to_string();
        assert!(
            err.contains("formatter definition file does not exist"),
            "{err}"
        );
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
            "schema_version: 1\nformatters:\n  - path: a.yml\n  - path: b.yml\ndefault: null\n",
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
        std::fs::write(
            &bad,
            "schema_version: 1\nid: \"Bad Id\"\ndescription: x\nsystem: y\n",
        )
        .unwrap();
        let path = write_config(
            tmp.path(),
            "schema_version: 1\nformatters:\n  - path: bad.yml\ndefault: null\n",
        );
        let err = load(Some(&path)).unwrap_err();
        let err = err.to_string();
        assert!(
            err.contains("bad.yml"),
            "the definition file is named: {err}"
        );
        assert!(
            err.contains(":2:"),
            "line inside the definition file: {err}"
        );
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
        assert!(
            err.contains("default \"missing\" does not name a formatter"),
            "{err}"
        );
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
    fn every_setting_is_required_and_reported_at_line_1() {
        let tmp = tempfile::tempdir().unwrap();
        let lines = [
            ("schema_version", "schema_version: 1\n"),
            ("formatters", "formatters: []\n"),
            ("default", "default: null\n"),
        ];
        for (key, _) in lines {
            let text: String = lines
                .iter()
                .filter(|(other, _)| *other != key)
                .map(|(_, line)| *line)
                .collect();
            let err = load(Some(&write_config(tmp.path(), &text))).unwrap_err();
            assert_eq!(
                err.message(),
                format!(
                    "\"{key}\" is missing; every setting must be written (see formatter.example.yml)"
                ),
                "{key}"
            );
            assert_eq!((err.line(), err.column()), (Some(1), Some(1)), "{key}");
        }
        let err = load(Some(&write_config(tmp.path(), ""))).unwrap_err();
        assert!(err.message().contains("\"schema_version\" is missing"));
    }

    #[test]
    fn null_default_means_none_and_null_elsewhere_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        // The values that were the built-in defaults, written out.
        let path = write_config(
            tmp.path(),
            "schema_version: 1\nformatters: []\ndefault: null\n",
        );
        let config = load(Some(&path)).unwrap().config;
        assert!(config.formatters.is_empty());
        assert_eq!(config.default, None);

        for (text, line, column) in [
            ("schema_version: ~\nformatters: []\ndefault: null\n", 1, 17),
            (
                "schema_version: 1\nformatters: null\ndefault: null\n",
                2,
                13,
            ),
        ] {
            let err = load(Some(&write_config(tmp.path(), text))).unwrap_err();
            assert!(err.message().contains("must not be null"), "{err}");
            assert_eq!((err.line(), err.column()), (Some(line), Some(column)));
        }
    }

    #[test]
    fn required_load_creates_settings_but_optional_load_stays_read_only() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf().into_os_string();
        let env =
            |key: &str| matches!(key, "XDG_CONFIG_HOME" | "HOME" | "APPDATA").then(|| base.clone());
        let expected = per_user_config_path(&env).unwrap();
        assert_eq!(load_optional_with_env(None, env).unwrap(), None);
        assert!(!expected.exists());
        let loaded = load_with_env(None, env).unwrap();
        assert_eq!(loaded.path, expected);
        assert_eq!(loaded.config.default.as_deref(), Some("clean-example"));
        assert_eq!(loaded.config.formatters.len(), 1);
        std::fs::write(&expected, "schema_version: 1\n").unwrap();
        let error = load_with_env(None, env).unwrap_err();
        assert!(error.message().contains("is missing"));
        assert_eq!(
            std::fs::read_to_string(&expected).unwrap(),
            "schema_version: 1\n"
        );

        let err = load_with_env(None, no_env).unwrap_err();
        assert!(
            err.message()
                .contains("no per-user configuration directory")
        );
        assert_eq!(load_optional_with_env(None, no_env).unwrap(), None);
        let missing = tmp.path().join("missing.yml");
        assert!(load_optional(Some(&missing)).is_err());
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
