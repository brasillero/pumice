//! Validated configuration.
//!
//! One YAML file configures the relay ([`load`]); each CLI keeps its own
//! login, routing and settings. Every setting must be written in the file:
//! an omitted one is an error, never a built-in default
//! (p17-serve-model-names/D3). Missing initial per-user files are created
//! before validation (auto-initial-settings/D1). Providers are an
//! explicit, ordered list and nothing is enabled unless the file says so.
//! Validation rejects problems with a `file:line:column` position and the
//! setting path; a missing setting is reported at the mapping it belongs in
//! (line 1 for a top-level one).

mod error;
mod raw;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_saphyr::Spanned;

pub use error::ConfigError;

use crate::providers::{self, Effort, ProviderSettings};
use raw::{Setting, Within};

/// Hard upper bound for `max_parallel`, kept small to limit resource use.
pub const MAX_PARALLEL_LIMIT: usize = 32;
const PER_USER_FILE: &str = "relay.yml";

/// Validated settings, every one written in the file.
#[derive(Clone, Debug, PartialEq, Eq)]
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
/// its validated settings.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProviderConfig {
    pub id: String,
    pub settings: ProviderSettings,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebugLogSettings {
    pub enabled: bool,
    pub path: PathBuf,
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
            "the relay configuration file does not exist; pass --config <path> or create it \
             (relay.example.yml has every setting)",
        )),
        Located::Absent(None) => Err(ConfigError::general(
            "no per-user configuration directory found (XDG_CONFIG_HOME, HOME or APPDATA); \
             pass --config <path>",
        )),
    }
}

/// [`load`] for the commands that also work without a configuration file
/// (`doctor`'s report): `None` when no `--config` is given and the per-user
/// file does not exist. A missing explicit file is still an error, and a
/// file that exists must be complete.
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
    match explicit {
        Some(path) if !path.exists() => Err(ConfigError::file(
            path,
            "the configuration file does not exist",
        )),
        Some(path) => Ok(Located::File(path.to_path_buf())),
        None => Ok(match per_user_config_path(get_env) {
            Some(candidate) if candidate.exists() => Located::File(candidate),
            candidate => Located::Absent(candidate),
        }),
    }
}

fn load_file(path: PathBuf) -> Result<LoadedConfig, ConfigError> {
    let text = std::fs::read_to_string(&path).map_err(|error| {
        ConfigError::file(&path, format!("cannot read configuration file: {error}"))
    })?;
    let config_dir = config_dir_of(&path);
    let raw =
        serde_saphyr::from_str_with_options::<Option<raw::RawConfig>>(&text, saphyr_options())
            .map_err(|error| parse_error(&path, &error))?;
    let config = validate(raw, Some(&config_dir), providers::PROVIDERS)
        .map_err(|error| error.with_file(&path))?;
    Ok(LoadedConfig { config, path })
}

/// Validates YAML `text` against an explicit provider descriptor list
/// instead of the built-in registry, attaching `path` to errors the way a
/// configuration file would. Relative settings have no anchor directory.
/// Exists so tests can exercise registry-driven behavior (full-settings
/// validation) with synthetic descriptors.
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
    // An empty document is a file that writes no setting at all.
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

    let port = raw.port.required(Within::Root, "\"port\"")?;
    if port.value == 0 {
        return Err(ConfigError::at(
            port.referenced,
            "port must be greater than zero",
        ));
    }
    let port = port.value;

    let total_timeout = raw
        .total_timeout_secs
        .required(Within::Root, "\"total_timeout_secs\"")?;
    if total_timeout.value == 0 {
        return Err(ConfigError::at(
            total_timeout.referenced,
            "total_timeout_secs must be greater than zero",
        ));
    }
    let total_timeout = Duration::from_secs(total_timeout.value);

    let max_parallel = raw
        .max_parallel
        .required(Within::Root, "\"max_parallel\"")?;
    if max_parallel.value == 0 {
        return Err(ConfigError::at(
            max_parallel.referenced,
            "max_parallel must be at least 1",
        ));
    }
    if max_parallel.value > MAX_PARALLEL_LIMIT as u64 {
        return Err(ConfigError::at(
            max_parallel.referenced,
            format!("max_parallel must be at most {MAX_PARALLEL_LIMIT}"),
        ));
    }
    let max_parallel = max_parallel.value as usize;

    let entries = raw.providers.required(Within::Root, "\"providers\"")?;
    let mut configured: Vec<ProviderConfig> = Vec::new();
    for entry in &entries.value.0 {
        let entry_location = entry.referenced;
        let within = Within::Mapping(entry_location);
        let entry = &entry.value;
        let id = match &entry.id {
            Setting::Value(id) => id,
            Setting::Missing | Setting::Null(_) => {
                return Err(ConfigError::at(
                    entry.id.location().unwrap_or(entry_location),
                    "a providers entry has no \"id\"; every entry starts with \"- id: <provider>\"",
                ));
            }
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
        let enabled = match &entry.enabled {
            Setting::Missing => {
                return Err(ConfigError::at(
                    id.referenced,
                    format!(
                        "providers entry \"{}\" has no \"enabled\"; add \"enabled: true\" or \"enabled: false\"",
                        id.value
                    ),
                ));
            }
            enabled => {
                enabled.required(within, &format!("providers entry \"{}\".enabled", id.value))?
            }
        };
        let settings = provider_settings(descriptor, id, entry, enabled, within, config_dir)?;
        configured.push(ProviderConfig {
            id: id.value.clone(),
            settings,
        });
    }

    let debug_log = raw.debug_log.required(Within::Root, "\"debug_log\"")?;
    let within = Within::Mapping(debug_log.referenced);
    let enabled = debug_log
        .value
        .enabled
        .required(within, "\"debug_log.enabled\"")?;
    let path = debug_log
        .value
        .path
        .required(within, "\"debug_log.path\"")?;
    if path.value.as_os_str().is_empty() {
        return Err(ConfigError::at(
            path.referenced,
            "debug_log.path must not be empty",
        ));
    }
    let debug_log = DebugLogSettings {
        enabled: enabled.value,
        path: resolve_against(&path.value, config_dir),
    };

    Ok(Config {
        port,
        total_timeout,
        max_parallel,
        debug_log,
        providers: configured,
    })
}

/// Builds one provider's settings from its entry, then runs its
/// full-settings validator. Every key of the entry is written; the entry's
/// `enabled` is authoritative: nothing is enabled unless the file says so.
fn provider_settings(
    descriptor: &providers::ProviderDescriptor,
    id: &Spanned<String>,
    raw: &raw::RawProviderEntry,
    enabled: &Spanned<bool>,
    within: Within,
    config_dir: Option<&Path>,
) -> Result<ProviderSettings, ConfigError> {
    let base = format!("providers entry \"{}\"", id.value);
    // Removed keys fail first, like the top-level ones. Each CLI keeps its
    // own configuration (gateway, routing, login); Pumice inherits it and
    // does not duplicate it here.
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
    let mut locations = providers::ProviderLocations {
        provider: Some(id.referenced),
        enabled: Some(enabled.referenced),
        ..providers::ProviderLocations::default()
    };

    let command_hint = format!(
        "write the command, e.g. \"binary: {}\" to look it up on PATH",
        descriptor.default_binary
    );
    let binary = match &raw.binary {
        Setting::Value(binary) => binary,
        Setting::Null(location) => {
            return Err(ConfigError::at(
                *location,
                format!("{base}.binary must not be null; {command_hint}"),
            ));
        }
        Setting::Missing => return Err(raw::missing(within, &format!("{base}.binary"))),
    };
    if binary.value.as_os_str().is_empty() {
        return Err(ConfigError::at(
            binary.referenced,
            format!("{base}.binary must not be empty; {command_hint}"),
        ));
    }
    locations.binary = Some(binary.referenced);
    let binary = resolve_binary(&binary.value, config_dir);

    // `model: null` means no model; only a disabled entry may say so (the
    // checks below reject it on an enabled one).
    let model = match raw.model.nullable(within, &format!("{base}.model"))? {
        Some(model) => {
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
            model.value.clone()
        }
        None => {
            locations.model = raw.model.location();
            String::new()
        }
    };

    let timeout = raw
        .timeout_secs
        .required(within, &format!("{base}.timeout_secs"))?;
    if timeout.value == 0 {
        return Err(ConfigError::at(
            timeout.referenced,
            format!("{base}.timeout_secs must be greater than zero"),
        ));
    }
    let effort = match &raw.effort {
        Setting::Value(effort) => {
            locations.effort = Some(effort.referenced);
            Effort::parse(&effort.value).ok_or_else(|| {
                ConfigError::at(
                    effort.referenced,
                    format!(
                        "{base}.effort must be low, medium or high, found \"{}\"",
                        effort.value
                    ),
                )
            })?
        }
        // Unlike `model`, every provider takes every level (only models
        // known to have no effort are refused, by the descriptor), so a
        // disabled entry writes one too.
        Setting::Null(location) => {
            return Err(ConfigError::at(
                *location,
                format!("{base}.effort must not be null; write low, medium or high"),
            ));
        }
        Setting::Missing => return Err(raw::missing(within, &format!("{base}.effort"))),
    };

    // Only a provider with a thinking switch (Claude, p16-provider-effort/D5)
    // has the key; the others reason through effort alone.
    let thinking = if descriptor.thinking_switch {
        match &raw.thinking {
            Setting::Value(thinking) => {
                locations.thinking = Some(thinking.referenced);
                Some(thinking.value)
            }
            Setting::Null(location) => {
                return Err(ConfigError::at(
                    *location,
                    format!("{base}.thinking must not be null; write true or false"),
                ));
            }
            Setting::Missing => return Err(raw::missing(within, &format!("{base}.thinking"))),
        }
    } else {
        if let Some(location) = raw.thinking.location() {
            return Err(ConfigError::at(
                location,
                format!(
                    "{base}.thinking is not a setting of this provider: only claude has it, and {} reasons through effort alone. Delete this key.",
                    id.value
                ),
            ));
        }
        None
    };

    let settings = ProviderSettings {
        enabled: enabled.value,
        binary,
        model,
        timeout: Duration::from_secs(timeout.value),
        effort,
        thinking,
    };

    // The descriptor runs first: its refusal (the archived generic adapter)
    // and its own requirements keep their established messages and
    // positions.
    (descriptor.validate_settings)(&settings, &locations)?;
    // Loader backstop for descriptors without requirements of their own:
    // an enabled entry always needs a usable model.
    if settings.enabled && settings.model.trim().is_empty() {
        return Err(ConfigError::at(
            locations.model.unwrap_or(id.referenced),
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
        assert_eq!(path, Some(xdg.join("pumice").join("relay.yml")));
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
                    .join("relay.yml")
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
                    .join("relay.yml")
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
                    .join("relay.yml")
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
            ConfigError::at(Location::UNKNOWN, "lost position").with_file(Path::new("relay.yml"));
        assert_eq!(error.to_string(), "relay.yml: lost position");
    }
}
