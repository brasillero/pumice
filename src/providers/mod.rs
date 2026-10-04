//! Providers and their registry.
//!
//! A provider is a module exporting a [`ProviderDescriptor`] named
//! `DESCRIPTOR`. Adding one is a new module plus one entry in
//! `register_providers!` below.

pub mod cli;
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

/// What the registry knows about one provider.
///
/// The descriptor owns the provider's defaults and the validation of its
/// option map, so a new provider brings its own rules with it.
pub struct ProviderDescriptor {
    pub id: &'static str,
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
}

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

register_providers!(claude, codex);

/// Looks up a provider by ID.
pub fn descriptor(id: &str) -> Option<&'static ProviderDescriptor> {
    PROVIDERS.iter().find(|d| d.id == id)
}

/// Builds every enabled provider in `config`, in deterministic ID order.
pub fn build_from_config(
    config: &Config,
    runner: Arc<ProcessRunner>,
) -> Result<Vec<Arc<dyn Provider>>, ConfigError> {
    let mut built = Vec::new();
    for (id, settings) in &config.providers {
        if !settings.enabled {
            continue;
        }
        let Some(descriptor) = descriptor(id) else {
            return Err(ConfigError::general(format!(
                "provider \"{id}\" is not registered"
            )));
        };
        built.push((descriptor.build)(settings, Arc::clone(&runner))?);
    }
    Ok(built)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_ids_are_unique_and_found() {
        for (i, d) in PROVIDERS.iter().enumerate() {
            assert!(PROVIDERS[..i].iter().all(|other| other.id != d.id));
            assert!(std::ptr::eq(descriptor(d.id).unwrap(), d));
        }
        assert!(descriptor("no-such-provider").is_none());
    }

    #[test]
    fn built_provider_reports_its_id() {
        let d = descriptor("claude").expect("claude is registered");
        let settings = (d.defaults)();
        assert!(settings.enabled);
        assert_eq!(settings.binary, None);
        assert_eq!(settings.model, "haiku");
        assert_eq!(settings.timeout, Duration::from_secs(30));
        let provider = (d.build)(&settings, Arc::new(ProcessRunner::new()))
            .expect("claude builds from its defaults");
        assert_eq!(provider.id(), "claude");

        let d = descriptor("codex").expect("codex is registered");
        let settings = (d.defaults)();
        assert!(settings.enabled);
        assert_eq!(settings.binary, None);
        assert_eq!(settings.model, "gpt-6.1-sol");
        assert_eq!(settings.timeout, Duration::from_secs(30));
        assert!(d.allowed_env.is_empty());
        let provider = (d.build)(&settings, Arc::new(ProcessRunner::new()))
            .expect("codex builds from its defaults");
        assert_eq!(provider.id(), "codex");
    }
}
