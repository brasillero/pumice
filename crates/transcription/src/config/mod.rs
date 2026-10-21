//! Validated `transcribe.yml` configuration.
//!
//! Explicit `--config FILE` wins; otherwise the per-user file is looked up.
//! Every setting must be written in the file: an omitted one is an error,
//! never a built-in default (p17-serve-model-names/D3). [`load`] needs the
//! file; [`load_optional`] serves the commands that also run without one
//! (catalog listing, downloads, `backends`).

mod error;
mod raw;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::compute::BackendMode;

pub use error::ConfigError;

use raw::{RawConfig, RawThreads, Within};

const PER_USER_FILE: &str = "transcribe.yml";
const MAX_THREADS: usize = 32;

/// Validated configuration settings, every one written in the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Default model id, which must name an entry in `models`; `None` is
    /// `default_model: null` (a command must then name its model).
    pub default_model: Option<String>,
    /// Default language hint; `None` is `language: null` (detect).
    pub language: Option<String>,
    /// Default thread count.
    pub threads: Threads,
    /// Compute backend mode (p14.1-windows-vulkan/D3).
    pub backend: BackendMode,
    /// Default server port; `None` is `server.port: null` (`serve` then
    /// needs `--port`).
    pub server_port: Option<u16>,
    /// The models directory for `catalog:` model entries, resolved to an
    /// absolute path; `None` is `models_dir: null` (the per-user data
    /// directory).
    pub models_dir: Option<PathBuf>,
    /// Explicit list of configured models. Nothing is enabled implicitly.
    pub models: Vec<ModelEntry>,
}

/// The `threads:` setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Threads {
    /// `auto`: the available parallelism, capped at 32.
    Auto,
    /// A fixed count from 1 to 32.
    Count(usize),
}

impl Threads {
    /// The fixed count, `None` for `auto`.
    pub fn count(self) -> Option<usize> {
        match self {
            Threads::Auto => None,
            Threads::Count(count) => Some(count),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelEntry {
    pub id: String,
    /// Absolute or config-dir-relative model path, resolved to an absolute
    /// path. For `catalog:` entries this is the catalog file name inside the
    /// models directory.
    pub path: PathBuf,
    /// Built-in catalog id when the entry uses `catalog:` instead of `path`.
    pub catalog: Option<String>,
}

/// The effective configuration and the file it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedConfig {
    pub config: Config,
    pub path: PathBuf,
}

/// Loads the configuration: an explicit `--config` path when given, else the
/// per-user configuration file. A missing file is an error.
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
    match locate(explicit, &get_env)? {
        Located::File(path) => load_file(path, &get_env),
        Located::Absent(Some(candidate)) => Err(ConfigError::file(
            candidate,
            "the transcription configuration file does not exist; pass --config FILE or \
             create it (transcribe.example.yml has every setting)",
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
        Located::File(path) => load_file(path, &get_env).map(Some),
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

fn load_file(
    path: PathBuf,
    get_env: &dyn Fn(&str) -> Option<OsString>,
) -> Result<LoadedConfig, ConfigError> {
    let text = std::fs::read_to_string(&path).map_err(|error| {
        ConfigError::file(&path, format!("cannot read configuration file: {error}"))
    })?;
    let config_dir = config_dir_of(&path);
    let raw = serde_saphyr::from_str_with_options::<Option<RawConfig>>(&text, saphyr_options())
        .map_err(|error| parse_error(&path, &error))?;
    let config =
        validate(raw, Some(&config_dir), get_env).map_err(|error| error.with_file(&path))?;
    Ok(LoadedConfig { config, path })
}

fn saphyr_options() -> serde_saphyr::Options {
    serde_saphyr::options! {
        reject_unsupported_tags: true,
        with_snippet: false,
    }
}

fn validate(
    raw: Option<RawConfig>,
    config_dir: Option<&Path>,
    get_env: &dyn Fn(&str) -> Option<OsString>,
) -> Result<Config, ConfigError> {
    // An empty document is a file that writes no setting at all.
    let raw = raw.unwrap_or_default();

    let schema_version = raw
        .schema_version
        .required(Within::Root, "\"schema_version\"")?;
    if schema_version.value != 1 {
        return Err(ConfigError::at(
            schema_version.referenced,
            format!("schema_version must be 1, got {}", schema_version.value),
        ));
    }

    // Checked against `models` below; required here, in file order.
    let default_model = raw
        .default_model
        .nullable(Within::Root, "\"default_model\"")?;

    let language = match raw.language.nullable(Within::Root, "\"language\"")? {
        Some(span) => Some(
            validate_language(&span.value)
                .map_err(|message| ConfigError::at(span.referenced, message))?,
        ),
        None => None,
    };

    let threads = raw.threads.required(Within::Root, "\"threads\"")?;
    let threads = match &threads.value {
        RawThreads::Text(text) if text == "auto" => Threads::Auto,
        RawThreads::Count(count) if (1..=MAX_THREADS as u64).contains(count) => {
            Threads::Count(*count as usize)
        }
        _ => {
            return Err(ConfigError::at(
                threads.referenced,
                format!("threads must be auto or a number from 1 to {MAX_THREADS}"),
            ));
        }
    };

    let backend = raw.backend.required(Within::Root, "\"backend\"")?;
    let backend = BackendMode::parse(&backend.value).ok_or_else(|| {
        ConfigError::at(backend.referenced, "backend must be one of cpu, auto, gpu")
    })?;

    let server = raw.server.required(Within::Root, "\"server\"")?;
    let server_port = server
        .value
        .port
        .nullable(Within::Mapping(server.referenced), "\"server.port\"")?
        .map(|port| port.value);

    let models_dir = match raw.models_dir.nullable(Within::Root, "\"models_dir\"")? {
        Some(span) => {
            if span.value.as_os_str().is_empty() {
                return Err(ConfigError::at(
                    span.referenced,
                    "models_dir must not be empty",
                ));
            }
            Some(resolve_against(&span.value, config_dir))
        }
        None => None,
    };

    let entries = raw.models.required(Within::Root, "\"models\"")?;
    let mut models: Vec<ModelEntry> = Vec::new();
    for entry in &entries.value {
        let entry_location = entry.referenced;
        let entry = &entry.value;
        let id = match &entry.id {
            Some(id) => id,
            None => return Err(ConfigError::at(entry_location, "model entry has no \"id\"")),
        };

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

        let (path, catalog) = match (&entry.path, &entry.catalog) {
            (Some(_), Some(_)) => {
                return Err(ConfigError::at(
                    entry_location,
                    format!(
                        "model entry \"{}\" must have exactly one of \"path\" or \"catalog\"",
                        id.value
                    ),
                ));
            }
            (None, None) => {
                return Err(ConfigError::at(
                    entry_location,
                    format!(
                        "model entry \"{}\" has no \"path\" or \"catalog\"",
                        id.value
                    ),
                ));
            }
            (Some(path), None) => {
                if path.value.as_os_str().is_empty() {
                    return Err(ConfigError::at(
                        path.referenced,
                        format!("model \"{}\" path must not be empty", id.value),
                    ));
                }
                (resolve_against(&path.value, config_dir), None)
            }
            (None, Some(catalog)) => {
                let catalog_entry = crate::catalog::find(&catalog.value).ok_or_else(|| {
                    ConfigError::at(
                        catalog.referenced,
                        format!(
                            "unknown catalog model \"{}\"; run `pumice-transcribe models catalog` to list available models",
                            catalog.value
                        ),
                    )
                })?;
                let dir = models_dir
                    .clone()
                    .or_else(|| crate::models_dir::default_models_dir(get_env))
                    .ok_or_else(|| {
                        ConfigError::at(
                            catalog.referenced,
                            "no models directory: set models_dir in transcribe.yml or define XDG_DATA_HOME or HOME",
                        )
                    })?;
                (
                    dir.join(catalog_entry.file_name),
                    Some(catalog.value.clone()),
                )
            }
        };

        models.push(ModelEntry {
            id: id.value.clone(),
            path,
            catalog,
        });
    }

    let default_model = match default_model {
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
        threads,
        backend,
        server_port,
        models_dir,
        models,
    })
}

pub(crate) fn is_valid_id(id: &str) -> bool {
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

    /// Every top-level setting, keyed by its name, with the value that was
    /// the built-in default before every setting had to be written.
    const SETTINGS: [(&str, &str); 8] = [
        ("schema_version", "schema_version: 1\n"),
        ("default_model", "default_model: null\n"),
        ("language", "language: null\n"),
        ("threads", "threads: auto\n"),
        ("backend", "backend: cpu\n"),
        ("server", "server:\n  port: null\n"),
        ("models_dir", "models_dir: null\n"),
        ("models", "models: []\n"),
    ];

    /// `text`, then every top-level setting it does not write itself.
    /// Appending keeps the line numbers in `text` unchanged.
    fn complete(text: &str) -> String {
        let mut yaml = text.to_owned();
        for (key, line) in SETTINGS {
            let prefix = format!("{key}:");
            if !text.lines().any(|written| written.starts_with(&prefix)) {
                yaml.push_str(line);
            }
        }
        yaml
    }

    /// Writes exactly `text` as `transcribe.yml`.
    fn write_config(tmp: &tempfile::TempDir, text: &str) -> PathBuf {
        let path = tmp.path().join("transcribe.yml");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(text.as_bytes()).unwrap();
        path
    }

    /// Writes `text`, completed by [`complete`], as `transcribe.yml`.
    fn write_complete(tmp: &tempfile::TempDir, text: &str) -> PathBuf {
        write_config(tmp, &complete(text))
    }

    #[test]
    fn valid_config_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_complete(
            &tmp,
            r#"schema_version: 1
models:
  - id: tiny
    path: tiny.gguf
"#,
        );
        let loaded = load(Some(&path)).unwrap();
        assert_eq!(loaded.path, path);
        assert_eq!(loaded.config.default_model, None);
        assert_eq!(loaded.config.models.len(), 1);
        assert_eq!(loaded.config.models[0].id, "tiny");
        assert_eq!(
            loaded.config.models[0].path,
            std::path::absolute(tmp.path()).unwrap().join("tiny.gguf")
        );
    }

    #[test]
    fn the_old_defaults_written_out_give_the_same_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let config = load(Some(&write_complete(&tmp, ""))).unwrap().config;
        assert_eq!(
            config,
            Config {
                default_model: None,
                language: None,
                threads: Threads::Auto,
                backend: BackendMode::Cpu,
                server_port: None,
                models_dir: None,
                models: Vec::new(),
            }
        );
    }

    #[test]
    fn every_top_level_setting_is_required() {
        let tmp = tempfile::tempdir().unwrap();
        for (key, _) in SETTINGS {
            let text: String = SETTINGS
                .iter()
                .filter(|(other, _)| *other != key)
                .map(|(_, line)| *line)
                .collect();
            let err = load(Some(&write_config(&tmp, &text))).unwrap_err();
            assert_eq!(
                err.message(),
                format!(
                    "\"{key}\" is missing; every setting must be written (see transcribe.example.yml)"
                ),
                "{key}"
            );
            assert_eq!((err.line(), err.column()), (Some(1), Some(1)), "{key}");
        }
        // An empty file names the first one.
        let err = load(Some(&write_config(&tmp, ""))).unwrap_err();
        assert!(err.message().contains("\"schema_version\" is missing"));
    }

    #[test]
    fn a_missing_server_port_is_reported_at_the_server_mapping() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_complete(&tmp, "schema_version: 1\nserver: {}\n");
        let err = load(Some(&path)).unwrap_err();
        assert!(
            err.message().contains("\"server.port\" is missing"),
            "{err}"
        );
        assert_eq!((err.line(), err.column()), (Some(2), Some(9)));

        let path = write_complete(&tmp, "schema_version: 1\nserver:\n  port: 17568\n");
        assert_eq!(load(Some(&path)).unwrap().config.server_port, Some(17568));
    }

    #[test]
    fn explicit_null_keeps_its_meaning_where_it_has_one() {
        let tmp = tempfile::tempdir().unwrap();
        // language: null detects, default_model: null names none,
        // models_dir: null is the per-user directory, server.port: null
        // needs --port: all written, so all valid.
        let config = load(Some(&write_complete(&tmp, ""))).unwrap().config;
        assert_eq!(config.language, None);
        assert_eq!(config.default_model, None);
        assert_eq!(config.models_dir, None);
        assert_eq!(config.server_port, None);

        for (line, at) in [
            ("schema_version: null\n", (1, 17)),
            ("threads: ~\n", (1, 10)),
            ("backend: null\n", (1, 10)),
            ("server: null\n", (1, 9)),
            ("models: null\n", (1, 9)),
        ] {
            let err = load(Some(&write_complete(&tmp, line))).unwrap_err();
            assert!(err.message().contains("must not be null"), "{line}: {err}");
            assert_eq!(
                (err.line(), err.column()),
                (Some(at.0), Some(at.1)),
                "{line}"
            );
        }
    }

    #[test]
    fn threads_are_auto_or_a_count_from_1_to_32() {
        let tmp = tempfile::tempdir().unwrap();
        for (text, threads) in [
            ("auto", Threads::Auto),
            ("1", Threads::Count(1)),
            ("8", Threads::Count(8)),
            ("32", Threads::Count(32)),
        ] {
            let path = write_complete(&tmp, &format!("threads: {text}\n"));
            assert_eq!(load(Some(&path)).unwrap().config.threads, threads, "{text}");
        }
        assert_eq!(Threads::Auto.count(), None);
        assert_eq!(Threads::Count(4).count(), Some(4));
        for bad in ["0", "33", "-1", "Auto", "eight", "\"8\"", "''"] {
            let path = write_complete(&tmp, &format!("threads: {bad}\n"));
            let err = load(Some(&path)).unwrap_err();
            assert_eq!(
                err.message(),
                "threads must be auto or a number from 1 to 32",
                "{bad}"
            );
            assert_eq!((err.line(), err.column()), (Some(1), Some(10)), "{bad}");
        }
    }

    #[test]
    fn missing_schema_version_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_config(&tmp, "models: []\n");
        let err = load(Some(&path)).unwrap_err();
        assert!(err.to_string().contains("\"schema_version\" is missing"));
        assert_eq!(err.line(), Some(1));
    }

    #[test]
    fn wrong_schema_version_reports_line() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_complete(&tmp, "schema_version: 2\n");
        let err = load(Some(&path)).unwrap_err();
        let err_str = err.to_string();
        assert!(err_str.contains("schema_version must be 1"));
        assert!(err_str.contains(":1:"));
    }

    #[test]
    fn unknown_top_level_key_reports_line() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_complete(&tmp, "schema_version: 1\nunknown: 5\n");
        let err = load(Some(&path)).unwrap_err();
        let err_str = err.to_string();
        assert!(err_str.contains("unknown setting `unknown`"));
        assert!(err_str.contains(":2:"));
    }

    #[test]
    fn duplicate_model_id_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_complete(
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
        let path = write_complete(
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
        let path = write_complete(
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
        let path = write_complete(
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
        let path = write_complete(
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
        let path = write_complete(
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
    fn empty_models_allowed_with_a_null_default() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_complete(&tmp, "schema_version: 1\ndefault_model: null\nmodels: []\n");
        let loaded = load(Some(&path)).unwrap();
        assert!(loaded.config.models.is_empty());
        assert!(loaded.config.default_model.is_none());
    }

    #[test]
    fn backend_accepts_each_mode() {
        let tmp = tempfile::tempdir().unwrap();
        for (text, mode) in [
            ("cpu", BackendMode::Cpu),
            ("auto", BackendMode::Auto),
            ("gpu", BackendMode::Gpu),
        ] {
            let path = write_complete(&tmp, &format!("schema_version: 1\nbackend: {text}\n"));
            assert_eq!(load(Some(&path)).unwrap().config.backend, mode);
        }
    }

    #[test]
    fn invalid_backend_reports_its_value() {
        let tmp = tempfile::tempdir().unwrap();
        for bad in ["vulkan", "GPU", "\"\"", "1"] {
            let path = write_complete(&tmp, &format!("schema_version: 1\nbackend: {bad}\n"));
            let err = load(Some(&path)).unwrap_err();
            assert_eq!(
                err.message(),
                "backend must be one of cpu, auto, gpu",
                "{bad}"
            );
            assert_eq!(err.line(), Some(2), "{bad}");
            assert_eq!(err.column(), Some(10), "{bad}");
        }
        let path = write_complete(&tmp, "schema_version: 1\nbackend: [gpu]\n");
        let err = load(Some(&path)).unwrap_err();
        assert_eq!(err.line(), Some(2));
    }

    #[test]
    fn missing_explicit_config_is_error() {
        let path = PathBuf::from("/tmp/does-not-exist-pumice-transcribe.yml");
        let err = load(Some(&path)).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
        let err = load_optional(Some(&path)).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn a_missing_per_user_file_is_an_error_unless_optional() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf().into_os_string();
        let env =
            |key: &str| matches!(key, "XDG_CONFIG_HOME" | "HOME" | "APPDATA").then(|| base.clone());
        let expected = per_user_config_path(&env).unwrap();
        let err = load_with_env(None, env).unwrap_err();
        assert_eq!(err.path(), Some(expected.as_path()));
        assert!(
            err.message()
                .contains("the transcription configuration file does not exist"),
            "{err}"
        );
        assert_eq!(load_optional_with_env(None, env).unwrap(), None);

        std::fs::create_dir_all(expected.parent().unwrap()).unwrap();
        std::fs::write(&expected, complete("")).unwrap();
        assert_eq!(load_with_env(None, env).unwrap().path, expected);
        assert_eq!(
            load_optional_with_env(None, env)
                .unwrap()
                .map(|loaded| loaded.path),
            Some(expected)
        );

        let err = load_with_env(None, no_env).unwrap_err();
        assert!(
            err.message()
                .contains("no per-user configuration directory")
        );
        assert_eq!(load_optional_with_env(None, no_env).unwrap(), None);
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

    #[test]
    fn catalog_entry_resolves_into_configured_models_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_complete(
            &tmp,
            r#"schema_version: 1
models_dir: downloaded
models:
  - id: tiny
    catalog: whisper-tiny-q8_0
"#,
        );
        let loaded = load(Some(&path)).unwrap();
        let expected = tmp.path().join("downloaded").join("whisper-tiny-Q8_0.gguf");
        assert_eq!(loaded.config.models[0].path, expected);
        assert_eq!(
            loaded.config.models[0].catalog.as_deref(),
            Some("whisper-tiny-q8_0")
        );
        assert_eq!(
            loaded.config.models_dir,
            Some(tmp.path().join("downloaded"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn catalog_entry_falls_back_to_xdg_data_home() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_complete(
            &tmp,
            r#"schema_version: 1
models:
  - id: tiny
    catalog: whisper-tiny-q8_0
"#,
        );
        let xdg = tmp.path().join("xdg");
        let env_value = xdg.clone().into_os_string();
        let loaded = load_with_env(Some(&path), |key| {
            (key == "XDG_DATA_HOME").then(|| env_value.clone())
        })
        .unwrap();
        assert_eq!(
            loaded.config.models[0].path,
            xdg.join("pumice")
                .join("models")
                .join("whisper-tiny-Q8_0.gguf")
        );
    }

    #[cfg(windows)]
    #[test]
    fn catalog_entry_falls_back_to_localappdata() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_complete(
            &tmp,
            r#"schema_version: 1
models:
  - id: tiny
    catalog: whisper-tiny-q8_0
"#,
        );
        let local = tmp.path().join("local");
        let env_value = local.clone().into_os_string();
        let loaded = load_with_env(Some(&path), |key| {
            (key == "LOCALAPPDATA").then(|| env_value.clone())
        })
        .unwrap();
        assert_eq!(
            loaded.config.models[0].path,
            local
                .join("pumice")
                .join("models")
                .join("whisper-tiny-Q8_0.gguf")
        );
    }

    #[test]
    fn catalog_entry_without_any_models_dir_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_complete(
            &tmp,
            r#"schema_version: 1
models:
  - id: tiny
    catalog: whisper-tiny-q8_0
"#,
        );
        let err = load_with_env(Some(&path), no_env).unwrap_err();
        assert!(err.to_string().contains("no models directory"));
    }

    #[test]
    fn unknown_catalog_id_reports_line() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_complete(
            &tmp,
            r#"schema_version: 1
models:
  - id: tiny
    catalog: whisper-base-q8_0
"#,
        );
        let err = load(Some(&path)).unwrap_err();
        let err_str = err.to_string();
        assert!(err_str.contains("unknown catalog model \"whisper-base-q8_0\""));
        assert!(err_str.contains(":4:"));
    }

    #[test]
    fn path_and_catalog_together_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_complete(
            &tmp,
            r#"schema_version: 1
models:
  - id: tiny
    path: tiny.gguf
    catalog: whisper-tiny-q8_0
"#,
        );
        let err = load(Some(&path)).unwrap_err();
        assert!(
            err.to_string()
                .contains("must have exactly one of \"path\" or \"catalog\"")
        );
    }

    #[test]
    fn model_entry_without_path_or_catalog_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_complete(
            &tmp,
            r#"schema_version: 1
models:
  - id: tiny
"#,
        );
        let err = load(Some(&path)).unwrap_err();
        assert!(err.to_string().contains("has no \"path\" or \"catalog\""));
    }

    #[test]
    fn empty_models_dir_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_complete(&tmp, "schema_version: 1\nmodels_dir: \"\"\n");
        let err = load(Some(&path)).unwrap_err();
        assert!(err.to_string().contains("models_dir must not be empty"));
    }

    #[test]
    fn absolute_models_dir_is_used_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let abs = tmp.path().join("elsewhere");
        let path = write_complete(
            &tmp,
            &format!("schema_version: 1\nmodels_dir: {}\n", abs.display()),
        );
        let loaded = load(Some(&path)).unwrap();
        assert_eq!(loaded.config.models_dir, Some(abs));
    }
}
