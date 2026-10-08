//! Providers and their registry.
//!
//! A provider is a module exporting a [`ProviderDescriptor`] named
//! `DESCRIPTOR`. Adding one is a new module plus one entry in
//! `register_providers!` below.

pub mod cli;
pub mod diagnostic;
pub mod discovery;
mod interface;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_saphyr::Location;

pub use interface::{
    FormatInput, NEUTRAL_SYSTEM_PROMPT, Provider, ProviderError, ProviderErrorCode, ProviderFuture,
    UserPrompt, non_empty_system_prompt,
};

use crate::config::{Config, ConfigError};
use crate::process::ProcessRunner;

/// Validated, fully defaulted settings of one provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderSettings {
    pub enabled: bool,
    /// Overrides the provider's command. `None` uses the provider's command
    /// name (a PATH lookup); a bare name is also a PATH lookup; anything
    /// with a path separator is a path, already resolved against the
    /// configuration file's directory.
    pub binary: Option<PathBuf>,
    pub model: String,
    pub timeout: Duration,
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
}

/// What the registry knows about one provider.
///
/// The descriptor owns the provider's defaults and the validation of its
/// settings, so a new provider brings its own rules with it. Enablement is
/// never the descriptor's business: the configuration file decides, and
/// nothing is enabled unless an entry says `enabled: true`.
#[derive(Clone, Copy)]
pub struct ProviderDescriptor {
    pub id: &'static str,
    /// Built-in settings (command, timeout, empty model). The loader
    /// replaces `enabled` with the entry's explicit value.
    pub defaults: fn() -> ProviderSettings,
    /// Builds a runnable provider from validated settings.
    pub build: BuildFn,
    /// Validates the provider's fully defaulted settings after overrides.
    /// Runs for every listed entry, enabled or not, so it can relate fields
    /// (such as rejecting an enabled provider without a `model`). Errors should point at the most
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
    /// (`EndpointUnavailable` then returns the original text).
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

register_providers!(claude, codex, kimi);

// Archived providers (owner decision 2026-10-08): their code and tests stay,
// but they are not registered, so configuration, detection, `doctor` and
// `/v1/models` never see them. They come back once the plugin architecture
// settles.
pub mod antigravity;
pub mod generic;
pub mod kiro;

/// Provider IDs that are archived or removed, with what the configuration
/// loader says when a file still lists one.
pub const RETIRED: &[(&str, &str)] = &[
    (
        "antigravity",
        "is archived for now and cannot be used; delete this entry",
    ),
    (
        "generic",
        "is archived for now and cannot be used; delete this entry",
    ),
    (
        "kiro",
        "is archived for now and cannot be used; delete this entry",
    ),
    ("opencode", "support was removed; delete this entry"),
];

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
        assert_eq!(PROVIDERS.len(), 3, "three providers are registered");
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
