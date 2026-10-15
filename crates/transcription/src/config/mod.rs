//! Validated `transcribe.yml` configuration.
//!
//! Explicit `--config FILE` wins and a missing explicit file is an error.
//! Otherwise the per-user file is looked up. A missing per-user file is fine:
//! the CLI then works from flags only.

mod error;
mod raw;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub use error::ConfigError;

use raw::RawConfig;

const PER_USER_FILE: &str = "transcribe.yml";
const MAX_THREADS: usize = 32;

/// Validated configuration settings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    /// Default model id, which must name an entry in `models` when present.
    pub default_model: Option<String>,
    /// Optional default language hint.
    pub language: Option<String>,
    /// Optional default thread count (1–32).
    pub threads: Option<usize>,
    /// Explicit list of configured models. Nothing is enabled implicitly.
    pub models: Vec<ModelEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelEntry {
    pub id: String,
    /// Absolute or config-dir-relative model path, resolved to an absolute path.
    pub path: PathBuf,
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
    validate(raw, Some(&config_dir)).map_err(|error| error.with_file(path))
}

fn saphyr_options() -> serde_saphyr::Options {
    serde_saphyr::options! {
        reject_unsupported_tags: true,
        with_snippet: false,
    }
}

fn validate(raw: Option<RawConfig>, config_dir: Option<&Path>) -> Result<Config, ConfigError> {
    let raw = raw.unwrap_or_default();

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

    let language = match &raw.language {
        Some(span) => Some(
            validate_language(&span.value)
                .map_err(|message| ConfigError::at(span.referenced, message))?,
        ),
        None => None,
    };

    if let Some(span) = &raw.threads
        && (span.value == 0 || span.value > MAX_THREADS)
    {
        return Err(ConfigError::at(
            span.referenced,
            format!("threads must be between 1 and {MAX_THREADS}"),
        ));
    }

    let mut models: Vec<ModelEntry> = Vec::new();
    for entry in &raw.models {
        let entry_location = entry.referenced;
        let entry = &entry.value;
        let id = match &entry.id {
            Some(id) => id,
            None => return Err(ConfigError::at(entry_location, "model entry has no \"id\"")),
        };
        let path = match &entry.path {
            Some(path) => path,
            None => {
                return Err(ConfigError::at(
                    entry_location,
                    format!("model entry \"{}\" has no \"path\"", id.value),
                ));
            }
        };

        if path.value.as_os_str().is_empty() {
            return Err(ConfigError::at(
                path.referenced,
                format!("model \"{}\" path must not be empty", id.value),
            ));
        }

        if id.value.is_empty() {
            return Err(ConfigError::at(id.referenced, "model id must not be empty"));
        }
        if !is_valid_id(&id.value) {
            return Err(ConfigError::at(
                id.referenced,
                format!(
                    "model id \"{}\" is invalid; ids must match [a-z0-9][a-z0-9._-]*",
                    id.value
                ),
            ));
        }
        if models.iter().any(|existing| existing.id == id.value) {
            return Err(ConfigError::at(
                id.referenced,
                format!("duplicate model id \"{}\"", id.value),
            ));
        }

        models.push(ModelEntry {
            id: id.value.clone(),
            path: resolve_against(&path.value, config_dir),
        });
    }

    let default_model = match &raw.default_model {
        Some(span) => {
            if !models.iter().any(|m| m.id == span.value) {
                return Err(ConfigError::at(
                    span.referenced,
                    format!(
                        "default_model \"{}\" does not name a model in models",
                        span.value
                    ),
                ));
            }
            Some(span.value.clone())
        }
        None => None,
    };

    Ok(Config {
        default_model,
        language,
        threads: raw.threads.map(|s| s.value),
        models,
    })
}

fn is_valid_id(id: &str) -> bool {
    let mut bytes = id.bytes();
    match bytes.next() {
        Some(b) if b.is_ascii_lowercase() || b.is_ascii_digit() => {}
        _ => return false,
    }
    bytes.all(|b| {
        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'_' || b == b'-'
    })
}

/// Validates a language hint the same way the CLI does.
pub fn validate_language(lang: &str) -> Result<String, &'static str> {
    let normalized = lang.trim().to_lowercase();
    if normalized.len() != 2 || !normalized.bytes().all(|b| b.is_ascii_lowercase()) {
        return Err("language must be a 2-letter ISO-639-1 code");
    }
    if !SUPPORTED_LANGUAGES.contains(&normalized.as_str()) {
        return Err("unsupported language code");
    }
    Ok(normalized)
}

/// Whisper's supported ISO-639-1 language codes.
const SUPPORTED_LANGUAGES: &[&str] = &[
    "af", "am", "ar", "as", "az", "ba", "be", "bg", "bn", "bo", "br", "bs", "ca", "cs", "cy", "da",
    "de", "el", "en", "es", "et", "eu", "fa", "fi", "fo", "fr", "gl", "gu", "ha", "haw", "he",
    "hi", "hr", "ht", "hu", "hy", "id", "is", "it", "ja", "jw", "ka", "km", "kn", "ko", "la", "lb",
    "ln", "lo", "lt", "lv", "mg", "mi", "mk", "ml", "mn", "mr", "ms", "mt", "my", "ne", "nl", "nn",
    "no", "oc", "pa", "pl", "ps", "pt", "ro", "ru", "sa", "sd", "si", "sk", "sl", "sn", "so", "sq",
    "sr", "su", "sv", "sw", "ta", "te", "tg", "th", "tk", "tl", "tr", "tt", "uk", "ur", "uz", "vi",
    "yi", "yo", "zh",
];

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

/// The per-user configuration file, when the platform's conventions place
/// one.
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

fn parse_error(path: &Path, error: &serde_saphyr::Error) -> ConfigError {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_saphyr::Location;
    use std::io::Write;

    fn no_env(_: &str) -> Option<OsString> {
        None
    }

    fn write_config(tmp: &tempfile::TempDir, text: &str) -> PathBuf {
        let path = tmp.path().join("transcribe.yml");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(text.as_bytes()).unwrap();
        path
    }

    #[test]
    fn valid_config_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_config(
            &tmp,
            r#"schema_version: 1
models:
  - id: tiny
    path: tiny.gguf
"#,
        );
        let loaded = load(Some(&path)).unwrap();
        assert_eq!(loaded.source, ConfigSource::File(path));
        assert_eq!(loaded.config.default_model, None);
        assert_eq!(loaded.config.models.len(), 1);
        assert_eq!(loaded.config.models[0].id, "tiny");
        assert_eq!(
            loaded.config.models[0].path,
            std::path::absolute(tmp.path()).unwrap().join("tiny.gguf")
        );
    }

    #[test]
    fn missing_schema_version_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_config(&tmp, "models: []\n");
        let err = load(Some(&path)).unwrap_err();
        assert!(err.to_string().contains("schema_version is required"));
    }

    #[test]
    fn wrong_schema_version_reports_line() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_config(&tmp, "schema_version: 2\n");
        let err = load(Some(&path)).unwrap_err();
        let err_str = err.to_string();
        assert!(err_str.contains("schema_version must be 1"));
        assert!(err_str.contains(":1:"));
    }

    #[test]
    fn unknown_top_level_key_reports_line() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_config(&tmp, "schema_version: 1\nunknown: 5\n");
        let err = load(Some(&path)).unwrap_err();
        let err_str = err.to_string();
        assert!(err_str.contains("unknown setting `unknown`"));
        assert!(err_str.contains(":2:"));
    }

    #[test]
    fn duplicate_model_id_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_config(
            &tmp,
            r#"schema_version: 1
models:
  - id: tiny
    path: a.gguf
  - id: tiny
    path: b.gguf
"#,
        );
        let err = load(Some(&path)).unwrap_err();
        assert!(err.to_string().contains("duplicate model id \"tiny\""));
    }

    #[test]
    fn dangling_default_model_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_config(
            &tmp,
            r#"schema_version: 1
default_model: missing
models:
  - id: tiny
    path: a.gguf
"#,
        );
        let err = load(Some(&path)).unwrap_err();
        let err_str = err.to_string();
        assert!(err_str.contains("default_model \"missing\" does not name a model"));
        assert!(err_str.contains(":2:"));
    }

    #[test]
    fn invalid_id_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_config(
            &tmp,
            r#"schema_version: 1
models:
  - id: "Bad Id"
    path: a.gguf
"#,
        );
        let err = load(Some(&path)).unwrap_err();
        assert!(err.to_string().contains("model id \"Bad Id\" is invalid"));
    }

    #[test]
    fn relative_path_resolves_against_config_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_config(
            &tmp,
            r#"schema_version: 1
models:
  - id: tiny
    path: models/tiny.gguf
"#,
        );
        let loaded = load(Some(&path)).unwrap();
        let expected = tmp.path().join("models/tiny.gguf");
        assert_eq!(loaded.config.models[0].path, expected);
    }

    #[test]
    fn empty_model_path_reports_its_value() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_config(
            &tmp,
            "schema_version: 1\nmodels:\n  - id: tiny\n    path: \"\"\n",
        );
        let err = load(Some(&path)).unwrap_err();
        assert_eq!(err.line(), Some(4));
        assert_eq!(err.column(), Some(11));
        assert!(err.message().contains("path must not be empty"));
    }

    #[test]
    fn absolute_path_is_used_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let abs = tmp.path().join("abs.gguf");
        let path = write_config(
            &tmp,
            &format!(
                r#"schema_version: 1
models:
  - id: tiny
    path: {}
"#,
                abs.display()
            ),
        );
        let loaded = load(Some(&path)).unwrap();
        assert_eq!(loaded.config.models[0].path, abs);
    }

    #[test]
    fn empty_models_allowed_without_default() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_config(&tmp, "schema_version: 1\nmodels: []\n");
        let loaded = load(Some(&path)).unwrap();
        assert!(loaded.config.models.is_empty());
        assert!(loaded.config.default_model.is_none());
    }

    #[test]
    fn missing_explicit_config_is_error() {
        let path = PathBuf::from("/tmp/does-not-exist-pumice-transcribe.yml");
        let err = load(Some(&path)).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn per_user_path_honours_xdg() {
        let xdg = std::env::temp_dir().join("xdg-transcribe");
        let env_value = xdg.clone().into_os_string();
        let path = per_user_config_path_for(
            &|key| (key == "XDG_CONFIG_HOME").then(|| env_value.clone()),
            false,
        );
        assert_eq!(path, Some(xdg.join("pumice").join("transcribe.yml")));
    }

    #[test]
    fn per_user_path_falls_back_to_home() {
        let path = per_user_config_path_for(
            &|key| (key == "HOME").then(|| OsString::from("/tmp/home")),
            false,
        );
        assert_eq!(
            path,
            Some(
                PathBuf::from("/tmp/home")
                    .join(".config")
                    .join("pumice")
                    .join("transcribe.yml")
            )
        );
    }

    #[test]
    fn per_user_path_ignores_relative_xdg() {
        let path = per_user_config_path_for(
            &|key| match key {
                "XDG_CONFIG_HOME" => Some(OsString::from("relative/dir")),
                "HOME" => Some(OsString::from("/tmp/home")),
                _ => None,
            },
            false,
        );
        assert_eq!(
            path,
            Some(
                PathBuf::from("/tmp/home")
                    .join(".config")
                    .join("pumice")
                    .join("transcribe.yml")
            )
        );
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
                    .join("transcribe.yml")
            )
        );
    }

    #[test]
    fn per_user_path_is_none_without_env() {
        assert_eq!(per_user_config_path(&no_env), None);
    }

    #[test]
    fn unknown_location_is_dropped() {
        let error = ConfigError::at(Location::UNKNOWN, "lost position")
            .with_file(Path::new("transcribe.yml"));
        assert_eq!(error.to_string(), "transcribe.yml: lost position");
    }
}
