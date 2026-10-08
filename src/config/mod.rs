//! Validated configuration with built-in defaults.
//!
//! One YAML file controls everything ([`load`]); when it is missing or empty
//! every setting falls back to a built-in default — which since 0.2 includes
//! no providers at all: providers are an explicit, ordered list and nothing
//! is enabled unless the file says so. Validation rejects problems with a
//! `file:line:column` position and the setting path.

mod error;
mod raw;

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_saphyr::Spanned;

pub use error::ConfigError;

use crate::providers::{self, ProviderSettings};

/// Port the service binds to when the configuration does not say otherwise.
pub const DEFAULT_PORT: u16 = 7567;

const DEFAULT_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
/// Default number of provider calls that may run at the same time.
pub const DEFAULT_MAX_PARALLEL: usize = 4;
/// Hard upper bound for `max_parallel`, kept small to limit resource use.
pub const MAX_PARALLEL_LIMIT: usize = 32;
const DEFAULT_DEBUG_LOG_FILE: &str = "pumice-debug.jsonl";
const PER_USER_FILE: &str = "pumice.yaml";

/// Validated settings with every default applied.
///
/// `Debug` is implemented by hand: environment and option values may be
/// private, so only their shape is shown.
#[derive(Clone, PartialEq, Eq)]
pub struct Config {
    pub port: u16,
    pub total_timeout: Duration,
    /// How many provider calls may run at the same time.
    pub max_parallel: usize,
    pub debug_log: DebugLogSettings,
    /// Every provider entry in file (list) order, enabled or not.
    pub providers: Vec<ProviderConfig>,
}

impl Config {
    /// The settings of one configured provider, if the list contains it.
    pub fn provider(&self, id: &str) -> Option<&ProviderSettings> {
        self.providers
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| &entry.settings)
    }
}

/// One entry of the configuration's `providers:` list: the provider id and
/// its validated, fully defaulted settings.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProviderConfig {
    pub id: String,
    pub settings: ProviderSettings,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("port", &self.port)
            .field("total_timeout", &self.total_timeout)
            .field("max_parallel", &self.max_parallel)
            .field("debug_log", &self.debug_log)
            .field("providers", &self.providers)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebugLogSettings {
    pub enabled: bool,
    pub path: PathBuf,
}

/// Where the effective configuration came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigSource {
    /// Loaded from this configuration file.
    File(PathBuf),
    /// No file was requested or found; built-in defaults apply.
    BuiltInDefaults,
}

/// The effective configuration and the file it came from, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedConfig {
    pub config: Config,
    pub source: ConfigSource,
}

/// Loads the configuration: an explicit `--config` path when given, else the
/// per-user configuration file, else built-in defaults.
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
            Some(candidate) if candidate.exists() => ConfigSource::File(candidate),
            _ => ConfigSource::BuiltInDefaults,
        },
    };
    let config = match &source {
        ConfigSource::File(path) => load_file(path)?,
        ConfigSource::BuiltInDefaults => validate(None, None, providers::PROVIDERS)?,
    };
    Ok(LoadedConfig { config, source })
}

fn load_file(path: &Path) -> Result<Config, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        ConfigError::file(path, format!("cannot read configuration file: {error}"))
    })?;
    let config_dir = config_dir_of(path);
    let raw =
        serde_saphyr::from_str_with_options::<Option<raw::RawConfig>>(&text, saphyr_options())
            .map_err(|error| parse_error(path, &error))?;
    validate(raw, Some(&config_dir), providers::PROVIDERS).map_err(|error| error.with_file(path))
}

/// Validates YAML `text` against an explicit provider descriptor list
/// instead of the built-in registry, attaching `path` to errors the way a
/// configuration file would. Relative settings have no anchor directory.
/// Exists so tests can exercise registry-driven behavior (default-off
/// providers, risk warnings, full-settings validation) with synthetic
/// descriptors.
pub fn validate_text_with_descriptors(
    text: &str,
    path: &Path,
    descriptors: &[providers::ProviderDescriptor],
) -> Result<Config, ConfigError> {
    let raw = serde_saphyr::from_str_with_options::<Option<raw::RawConfig>>(text, saphyr_options())
        .map_err(|error| parse_error(path, &error))?;
    validate(raw, None, descriptors).map_err(|error| error.with_file(path))
}

fn saphyr_options() -> serde_saphyr::Options {
    serde_saphyr::options! {
        // Tags (including !include) are never interpreted; the feature is not
        // even compiled in, and stray tags are rejected rather than ignored.
        reject_unsupported_tags: true,
        // check-config prints one error per line.
        with_snippet: false,
    }
}

/// Directory relative paths in the file resolve against. Made absolute so a
/// later working-directory change cannot break stored paths.
fn config_dir_of(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    match absolute.parent() {
        Some(parent) => parent.to_path_buf(),
        None => PathBuf::from("."),
    }
}

fn validate(
    raw: Option<raw::RawConfig>,
    config_dir: Option<&Path>,
    descriptors: &[providers::ProviderDescriptor],
) -> Result<Config, ConfigError> {
    let raw = raw.unwrap_or_default();

    // Removed settings are parsed only to fail at their own position with
    // the form that replaced them.
    if let Some(span) = &raw.default {
        return Err(ConfigError::at(
            span.referenced,
            format!(
                "\"default\" was removed in 0.2: the client's model field picks the provider, and a request without a model gets an error and the app keeps its own text. List every provider you want offered under \"providers:\", e.g.:\n{}",
                raw::PROVIDERS_LIST_FORM
            ),
        ));
    }
    if let Some(span) = &raw.default_provider {
        return Err(ConfigError::at(
            span.referenced,
            format!(
                "\"default_provider\" was removed in 0.2: providers are an explicit ordered list now, and the provider a request runs is the one its model names — a request without a model gets an error and the app keeps its own text. Use e.g.:\n{}",
                raw::PROVIDERS_LIST_FORM
            ),
        ));
    }
    if let Some(span) = &raw.fallback_order {
        return Err(ConfigError::at(
            span.referenced,
            format!(
                "\"fallback_order\" was removed in 0.2: there is no fallback anymore — on failure the app gets an error and keeps its own text. List every provider you want offered under \"providers:\" in order, e.g.:\n{}",
                raw::PROVIDERS_LIST_FORM
            ),
        ));
    }
    if let Some(span) = &raw.prompts {
        return Err(ConfigError::at(
            span.referenced,
            "\"prompts\" was removed: the app that sends the dictation (Handy, OpenWhispr, …) sends its own prompt, and Pumice adds none. Delete this section and put your instructions in the app's prompt instead.",
        ));
    }

    let port = match &raw.port {
        Some(span) if span.value == 0 => {
            return Err(ConfigError::at(
                span.referenced,
                "port must be greater than zero",
            ));
        }
        Some(span) => span.value,
        None => DEFAULT_PORT,
    };

    let mut configured: Vec<ProviderConfig> = Vec::new();
    for entry in &raw.providers {
        let entry_location = entry.referenced;
        let entry = &entry.value;
        let Some(id) = &entry.id else {
            return Err(ConfigError::at(
                entry_location,
                "a providers entry has no \"id\"; every entry starts with \"- id: <provider>\"",
            ));
        };
        let Some(descriptor) = descriptors.iter().find(|d| d.id == id.value) else {
            // Archived and removed providers get a precise message.
            let message = match providers::RETIRED
                .iter()
                .find(|(retired, _)| *retired == id.value)
            {
                Some((_, why)) => format!("\"{}\" {why}", id.value),
                None => format!("\"{}\" is not a known provider", id.value),
            };
            return Err(ConfigError::at(id.referenced, message));
        };
        if configured.iter().any(|existing| existing.id == id.value) {
            return Err(ConfigError::at(
                id.referenced,
                format!("duplicate provider \"{}\" in providers", id.value),
            ));
        }
        let Some(enabled) = &entry.enabled else {
            return Err(ConfigError::at(
                id.referenced,
                format!(
                    "providers entry \"{}\" has no \"enabled\"; add \"enabled: true\" or \"enabled: false\"",
                    id.value
                ),
            ));
        };
        let settings = provider_settings(descriptor, id, entry, enabled, config_dir)?;
        configured.push(ProviderConfig {
            id: id.value.clone(),
            settings,
        });
    }

    let total_timeout = match &raw.total_timeout_secs {
        Some(span) if span.value == 0 => {
            return Err(ConfigError::at(
                span.referenced,
                "total_timeout_secs must be greater than zero",
            ));
        }
        Some(span) => Duration::from_secs(span.value),
        None => DEFAULT_TOTAL_TIMEOUT,
    };

    let max_parallel = match &raw.max_parallel {
        Some(span) if span.value == 0 => {
            return Err(ConfigError::at(
                span.referenced,
                "max_parallel must be at least 1",
            ));
        }
        Some(span) if span.value > MAX_PARALLEL_LIMIT as u64 => {
            return Err(ConfigError::at(
                span.referenced,
                format!("max_parallel must be at most {MAX_PARALLEL_LIMIT}"),
            ));
        }
        Some(span) => span.value as usize,
        None => DEFAULT_MAX_PARALLEL,
    };

    let debug_log = {
        let raw_log = raw.debug_log.as_ref();
        DebugLogSettings {
            enabled: raw_log
                .and_then(|log| log.enabled.as_ref())
                .map(|enabled| enabled.value)
                .unwrap_or(false),
            path: raw_log
                .and_then(|log| log.path.as_ref())
                .map(|path| resolve_against(&path.value, config_dir))
                .unwrap_or_else(|| resolve_against(Path::new(DEFAULT_DEBUG_LOG_FILE), config_dir)),
        }
    };

    Ok(Config {
        port,
        total_timeout,
        max_parallel,
        debug_log,
        providers: configured,
    })
}

/// Applies one provider's defaults and the entry's overrides, then runs its
/// full-settings validator. The entry's `enabled` is authoritative: nothing
/// is enabled unless the file says so.
fn provider_settings(
    descriptor: &providers::ProviderDescriptor,
    id: &Spanned<String>,
    raw: &raw::RawProviderEntry,
    enabled: &Spanned<bool>,
    config_dir: Option<&Path>,
) -> Result<ProviderSettings, ConfigError> {
    let base = format!("providers entry \"{}\"", id.value);
    let mut settings = (descriptor.defaults)();
    settings.enabled = enabled.value;
    let mut locations = providers::ProviderLocations {
        provider: Some(id.referenced),
        enabled: Some(enabled.referenced),
        ..providers::ProviderLocations::default()
    };
    if let Some(binary) = &raw.binary {
        if binary.value.as_os_str().is_empty() {
            return Err(ConfigError::at(
                binary.referenced,
                format!(
                    "{base}.binary must not be empty; remove the key to use the command on PATH"
                ),
            ));
        }
        locations.binary = Some(binary.referenced);
        settings.binary = Some(resolve_binary(&binary.value, config_dir));
    }
    if let Some(model) = &raw.model {
        locations.model = Some(model.referenced);
        if model.value.trim().is_empty() && enabled.value {
            return Err(ConfigError::at(
                model.referenced,
                format!("{base}.model must not be empty"),
            ));
        }
        // Every CLI receives the model as a command-line argument: a
        // leading '-' would be read as a flag.
        if model.value.trim_start().starts_with('-') {
            return Err(ConfigError::at(
                model.referenced,
                format!("{base}.model must not start with '-'"),
            ));
        }
        settings.model = model.value.clone();
    } else {
        // Missing-model complaints point at the entry's id.
        locations.model = Some(id.referenced);
    }
    if let Some(timeout) = &raw.timeout_secs {
        if timeout.value == 0 {
            return Err(ConfigError::at(
                timeout.referenced,
                format!("{base}.timeout_secs must be greater than zero"),
            ));
        }
        settings.timeout = Duration::from_secs(timeout.value);
    }

    // Each CLI keeps its own configuration (gateway, routing, login); Pumice
    // inherits it and does not duplicate it here.
    for (key, span) in [("env", &raw.env), ("options", &raw.options)] {
        if let Some(span) = span {
            return Err(ConfigError::at(
                span.referenced,
                format!(
                    "{base}.{key} was removed: Pumice uses each CLI's own configuration (gateway, routing, login). Delete this key and configure the CLI itself instead."
                ),
            ));
        }
    }
    // The descriptor runs first: its refusal (Antigravity) and its own
    // requirements (an explicit provider/model, a plausible alias) keep
    // their established messages and positions.
    (descriptor.validate_settings)(&settings, &locations)?;
    // Loader backstop for descriptors without requirements of their own:
    // an enabled entry always needs a usable model.
    if settings.enabled && settings.model.trim().is_empty() {
        return Err(ConfigError::at(
            id.referenced,
            format!("{base} is enabled but has no model; add \"model: <name>\""),
        ));
    }
    Ok(settings)
}

/// A bare command name stays a PATH lookup; anything with a path separator
/// is a path, resolved against the configuration file's directory.
fn resolve_binary(path: &Path, config_dir: Option<&Path>) -> PathBuf {
    if path.components().count() <= 1 {
        return path.to_path_buf();
    }
    resolve_against(path, config_dir)
}

/// Resolves a relative path against the configuration file's directory.
fn resolve_against(path: &Path, config_dir: Option<&Path>) -> PathBuf {
    match config_dir {
        Some(dir) if path.is_relative() => dir.join(path),
        _ => path.to_path_buf(),
    }
}

/// The per-user configuration file, when the platform's conventions place
/// one. The environment reader is injected so tests can use a temporary
/// home directory without mutating the process environment.
pub fn per_user_config_path(get_env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    per_user_config_path_for(get_env, cfg!(windows))
}

/// `windows` is a parameter (rather than `cfg!` inside the body) so both
/// platform rules are exercised on every CI platform.
fn per_user_config_path_for(
    get_env: &dyn Fn(&str) -> Option<OsString>,
    windows: bool,
) -> Option<PathBuf> {
    let base = if windows {
        // An empty APPDATA would put the file under the current directory;
        // treat it as unset.
        get_env("APPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    } else {
        match get_env("XDG_CONFIG_HOME") {
            // The XDG spec requires an absolute path; ignore relative values.
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

/// `~/.config` on Linux and macOS.
fn home_config_dir(get_env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    get_env("HOME").map(|home| PathBuf::from(home).join(".config"))
}

/// Translates a YAML problem into a [`ConfigError`], keeping the parser's
/// position and using concise messages for the common cases (the crate's
/// own messages are developer-oriented).
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

/// The crate appends ` at line N, column M` to its messages; the position is
/// already extracted separately, so strip it to avoid reporting it twice.
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

    fn no_env(_: &str) -> Option<OsString> {
        None
    }

    #[test]
    fn per_user_path_honours_xdg() {
        // An absolute path on every OS (`/tmp/xdg` is relative on Windows).
        let xdg = std::env::temp_dir().join("xdg");
        let env_value = xdg.clone().into_os_string();
        let path = per_user_config_path_for(
            &|key| (key == "XDG_CONFIG_HOME").then(|| env_value.clone()),
            false,
        );
        assert_eq!(path, Some(xdg.join("pumice").join("pumice.yaml")));
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
                    .join("pumice.yaml")
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
                    .join("pumice.yaml")
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
                    .join("pumice.yaml")
            )
        );
    }

    #[test]
    fn per_user_path_is_none_without_env() {
        assert_eq!(per_user_config_path(&no_env), None);
    }

    #[test]
    fn unknown_location_is_dropped() {
        let error =
            ConfigError::at(Location::UNKNOWN, "lost position").with_file(Path::new("pumice.yaml"));
        assert_eq!(error.to_string(), "pumice.yaml: lost position");
    }
}
