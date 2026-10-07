//! Providers and their registry.
//!
//! A provider is a module exporting a [`ProviderDescriptor`] named
//! `DESCRIPTOR`. Adding one is a new module plus one entry in
//! `register_providers!` below.

pub mod cli;
pub mod diagnostic;
pub mod discovery;
mod interface;

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_saphyr::Location;

pub use interface::{
    FormatInput, Provider, ProviderError, ProviderErrorCode, ProviderFuture, UserPrompt,
};

use crate::config::{Config, ConfigError};
use crate::process::ProcessRunner;

/// Validated, fully defaulted settings of one provider.
///
/// `Debug` is implemented by hand: environment and option values may be
/// private, so only their keys are shown.
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderSettings {
    pub enabled: bool,
    /// Overrides the provider's command. `None` uses the provider's command
    /// name (a PATH lookup); a bare name is also a PATH lookup; anything
    /// with a path separator is a path, already resolved against the
    /// configuration file's directory.
    pub binary: Option<PathBuf>,
    pub model: String,
    pub timeout: Duration,
    /// Non-secret routing overrides the adapter forwards to the CLI.
    pub env: BTreeMap<String, String>,
    /// Provider-specific options, validated by the provider's descriptor.
    pub options: BTreeMap<String, String>,
}

impl fmt::Debug for ProviderSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderSettings")
            .field("enabled", &self.enabled)
            .field("binary", &self.binary)
            .field("model", &self.model)
            .field("timeout", &self.timeout)
            .field("env_keys", &self.env.keys().collect::<Vec<_>>())
            .field("option_keys", &self.options.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// One provider option as written in the configuration file, with the
/// locations the descriptor needs for precise errors.
pub struct RawOption<'a> {
    pub key: &'a str,
    pub value: &'a str,
    pub key_at: Location,
    pub value_at: Location,
}

/// Source locations of one provider's configuration entries.
///
/// The loader fills this from the provider's list entry; a field stays
/// `None` when the entry has no value for that setting, so a validator can
/// point at the right line or fall back to a file-level error.
#[derive(Clone, Debug, Default)]
pub struct ProviderLocations {
    /// The provider's `id` in its `providers:` entry.
    pub provider: Option<Location>,
    pub enabled: Option<Location>,
    pub model: Option<Location>,
    pub binary: Option<Location>,
    /// Each option value's location, keyed by option name.
    pub options: BTreeMap<String, Location>,
    /// The port Pumice itself listens on. Adapters that call out (the generic
    /// loopback adapter) must reject endpoints pointing back at it, to prevent
    /// recursive requests. `Default` is 0, which no validated endpoint port
    /// (1–65535) can equal, so direct construction without the loader stays
    /// safe.
    pub port: u16,
}

/// What the registry knows about one provider.
///
/// The descriptor owns the provider's defaults and the validation of its
/// option map, so a new provider brings its own rules with it. Enablement is
/// never the descriptor's business: the configuration file decides, and
/// nothing is enabled unless an entry says `enabled: true`.
#[derive(Clone, Copy)]
pub struct ProviderDescriptor {
    pub id: &'static str,
    /// Built-in settings (command, timeout, empty model). The loader
    /// replaces `enabled` with the entry's explicit value.
    pub defaults: fn() -> ProviderSettings,
    /// Non-secret routing environment variables the configuration may set
    /// for this provider. Anything else is rejected by the configuration
    /// loader, as are credential-looking names.
    pub allowed_env: &'static [&'static str],
    /// Validates the provider's `options:` entries. The provider composes
    /// its own fully pathed messages (it knows which keys it accepts); the
    /// configuration loader attaches the file.
    pub validate_options: fn(&[RawOption<'_>]) -> Result<(), ConfigError>,
    /// Builds a runnable provider from validated settings.
    pub build: BuildFn,
    /// Fixed notice `check-config` and startup print while the provider is
    /// enabled. Never sent to a CLI and never part of formatted dictation.
    pub risk_warning: Option<&'static str>,
    /// Validates the provider's fully defaulted settings after overrides.
    /// Runs for every listed entry, enabled or not, so it can relate fields
    /// `validate_options` sees separately (such as rejecting an enabled
    /// provider without a `model`). Errors should point at the most
    /// specific [`ProviderLocations`] entry available.
    pub validate_settings: ValidateSettingsFn,
    /// How startup detection checks this provider's presence (S2.8).
    /// Detection never spends quota and never changes `enabled`.
    pub probe: ProbeSpec,
    /// Command name looked up on PATH when the configuration sets no
    /// `binary`; the same default the adapter builds with.
    pub default_binary: &'static str,
    /// npm package entrypoint for Windows `.cmd` shim translation during
    /// detection. Must equal the adapter's value so detection resolves the
    /// program exactly like a formatting call; `None` refuses shims.
    pub npm_entrypoint: Option<&'static str>,
    /// Short official installation hint for a missing provider, printed by
    /// startup and (later) `pumice doctor`.
    pub install_hint: &'static str,
}

/// How [`discovery`] checks a provider's presence at startup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeSpec {
    /// Resolve and run the given version arguments.
    Version(&'static [&'static str]),
    /// Resolve on PATH only; never spawn.
    PathOnly,
    /// No CLI exists to detect (the generic loopback adapter): detection
    /// neither resolves nor spawns anything. The provider is reported
    /// available for listing purposes when enabled; its endpoint is a
    /// network target whose availability is only checked at call time
    /// (`EndpointUnavailable` then triggers the fallback chain).
    NotApplicable,
}

/// Validates one provider's fully defaulted settings; see
/// [`ProviderDescriptor::validate_settings`].
pub type ValidateSettingsFn = fn(&ProviderSettings, &ProviderLocations) -> Result<(), ConfigError>;

/// Builds a runnable provider from validated settings.
pub type BuildFn =
    fn(&ProviderSettings, Arc<ProcessRunner>) -> Result<Arc<dyn Provider>, ConfigError>;

/// Declares provider modules and lists their descriptors in [`PROVIDERS`].
macro_rules! register_providers {
    ($($module:ident),+ $(,)?) => {
        $(pub mod $module;)+

        /// Every known provider, in registration order.
        pub static PROVIDERS: &[ProviderDescriptor] = &[$($module::DESCRIPTOR),+];
    };
}

register_providers!(claude, codex, opencode, antigravity, generic, kimi, kiro);

/// Looks up a provider by ID.
pub fn descriptor(id: &str) -> Option<&'static ProviderDescriptor> {
    PROVIDERS.iter().find(|d| d.id == id)
}

/// Full-settings validator for providers without requirements beyond the
/// loader's own checks.
pub fn validate_settings_noop(
    _settings: &ProviderSettings,
    _locations: &ProviderLocations,
) -> Result<(), ConfigError> {
    Ok(())
}

/// `warning: <id>: <text>` lines for every enabled provider that carries a
/// risk warning, in configuration (list) order. Shared by `check-config`
/// and startup.
pub fn risk_warnings(config: &Config, descriptors: &[ProviderDescriptor]) -> Vec<String> {
    config
        .providers
        .iter()
        .filter(|entry| entry.settings.enabled)
        .filter_map(|entry| {
            let warning = descriptors
                .iter()
                .find(|descriptor| descriptor.id == entry.id)?
                .risk_warning?;
            Some(format!("warning: {}: {warning}", entry.id))
        })
        .collect()
}

/// Builds every enabled provider in `config`, in configuration (list) order.
pub fn build_from_config(
    config: &Config,
    runner: Arc<ProcessRunner>,
) -> Result<Vec<Arc<dyn Provider>>, ConfigError> {
    let mut built = Vec::new();
    for entry in &config.providers {
        if !entry.settings.enabled {
            continue;
        }
        let Some(descriptor) = descriptor(&entry.id) else {
            return Err(ConfigError::general(format!(
                "provider \"{}\" is not registered",
                entry.id
            )));
        };
        built.push((descriptor.build)(&entry.settings, Arc::clone(&runner))?);
    }
    Ok(built)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_ids_are_unique_and_found() {
        assert_eq!(PROVIDERS.len(), 7, "seven providers are registered");
        for (i, d) in PROVIDERS.iter().enumerate() {
            assert!(PROVIDERS[..i].iter().all(|other| other.id != d.id));
            assert!(std::ptr::eq(descriptor(d.id).unwrap(), d));
        }
        assert!(descriptor("no-such-provider").is_none());
    }

    #[test]
    fn built_provider_reports_its_id() {
        // Built-in settings carry no model anymore: the configuration file
        // sets one on every enabled entry.
        let d = descriptor("claude").expect("claude is registered");
        let settings = (d.defaults)();
        assert_eq!(settings.model, "");
        assert_eq!(settings.binary, None);
        assert_eq!(settings.timeout, Duration::from_secs(30));
        let mut settings = settings;
        settings.enabled = true;
        settings.model = "haiku".to_owned();
        let provider = (d.build)(&settings, Arc::new(ProcessRunner::new()))
            .expect("claude builds with an explicit model");
        assert_eq!(provider.id(), "claude");

        let d = descriptor("codex").expect("codex is registered");
        let settings = (d.defaults)();
        assert_eq!(settings.model, "");
        assert_eq!(settings.binary, None);
        assert_eq!(settings.timeout, Duration::from_secs(30));
        assert!(d.allowed_env.is_empty());
        let mut settings = settings;
        settings.enabled = true;
        settings.model = "gpt-6.1-sol".to_owned();
        let provider = (d.build)(&settings, Arc::new(ProcessRunner::new()))
            .expect("codex builds with an explicit model");
        assert_eq!(provider.id(), "codex");
    }

    #[test]
    fn claude_and_codex_have_no_new_capabilities() {
        for id in ["claude", "codex"] {
            let d = descriptor(id).expect("registered");
            assert_eq!(d.risk_warning, None, "{id} carries no risk warning");
            let mut settings = (d.defaults)();
            settings.enabled = true;
            settings.model = "any".to_owned();
            assert!(
                (d.validate_settings)(&settings, &ProviderLocations::default()).is_ok(),
                "{id} full-settings validation is a no-op"
            );
        }
    }
}
