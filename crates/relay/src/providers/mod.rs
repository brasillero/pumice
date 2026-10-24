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

/// Validated settings of one provider, every one written in the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderSettings {
    pub enabled: bool,
    /// The provider's command: a bare name is a PATH lookup; anything with a
    /// path separator is a path, already resolved against the configuration
    /// file's directory.
    pub binary: PathBuf,
    /// Empty only on a disabled entry that writes `model: null`.
    pub model: String,
    pub timeout: Duration,
    /// Reasoning effort for every call this provider makes
    /// (p16-provider-effort/D4).
    pub effort: Effort,
    /// Whether the CLI may think: `Some` exactly when the descriptor has a
    /// [`ProviderDescriptor::thinking_switch`] (Claude, p16-provider-effort/D5).
    pub thinking: Option<bool>,
}

/// Reasoning effort on Pumice's common scale (p16-provider-effort/D2). Each
/// plugin maps it to its own CLI setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effort {
    Low,
    Medium,
    High,
}

impl Effort {
    /// Every level, lowest first.
    pub const ALL: [Effort; 3] = [Effort::Low, Effort::Medium, Effort::High];

    /// The level as `relay.yml` writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
        }
    }

    /// The level `relay.yml` writes, exactly (no case folding, no aliases).
    pub fn parse(value: &str) -> Option<Effort> {
        Effort::ALL
            .into_iter()
            .find(|effort| effort.as_str() == value)
    }
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
    pub effort: Option<Location>,
}

/// What the registry knows about one provider.
///
/// The descriptor owns the validation of its settings, so a new provider
/// brings its own rules with it. It has no built-in settings: every one is
/// written in the configuration file (p17-serve-model-names/D3), and
/// nothing is enabled unless an entry says `enabled: true`.
#[derive(Clone, Copy)]
pub struct ProviderDescriptor {
    pub id: &'static str,
    /// Builds a runnable provider from validated settings.
    pub build: BuildFn,
    /// Whether the entry has a `thinking` key next to `effort`: required
    /// when true, rejected when false (p16-provider-effort/D5).
    pub thinking_switch: bool,
    /// Validates the provider's settings as the file writes them.
    /// Runs for every listed entry, enabled or not, so it can relate fields
    /// (such as rejecting an enabled provider without a `model`). Errors should point at the most
    /// specific [`ProviderLocations`] entry available.
    pub validate_settings: ValidateSettingsFn,
    /// How startup detection checks this provider's presence (S2.8).
    /// Detection never spends quota and never changes `enabled`.
    pub probe: ProbeSpec,
    /// The provider's standard command name: what detection looks up for a
    /// provider the file does not list, and the `binary` hint in errors.
    pub default_binary: &'static str,
    /// npm package entrypoint for Windows `.cmd` shim translation during
    /// detection. Must equal the adapter's value so detection resolves the
    /// program exactly like a formatting call; `None` refuses shims.
    pub npm_entrypoint: Option<&'static str>,
    /// Short official installation hint for a missing provider, printed by
    /// startup and `pumice relay doctor`.
    pub install_hint: &'static str,
}

/// How [`discovery`] checks a provider's presence at startup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeSpec {
    /// Resolve and run the given version arguments.
    Version(&'static [&'static str]),
    /// No CLI exists to detect (the generic loopback adapter): detection
    /// neither resolves nor spawns anything. The provider is reported
    /// available for listing purposes when enabled; its endpoint is a
    /// network target whose availability is only checked at call time
    /// (`EndpointUnavailable` then fails the request).
    NotApplicable,
}

/// Validates one provider's settings; see
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

register_providers!(claude, codex, kiro);

// Archived provider (owner decision 2026-10-08): its code and tests stay,
// but it is not registered, so configuration, detection, `doctor` and
// `/v1/models` never see it. It comes back once the plugin architecture
// settles.
pub mod generic;

/// Provider IDs that are archived or removed, with what the configuration
/// loader says when a file still lists one.
pub const RETIRED: &[(&str, &str)] = &[
    (
        "antigravity",
        "provider was removed (p16-provider-effort/D6); delete this entry",
    ),
    (
        "generic",
        "is archived for now and cannot be used; delete this entry",
    ),
    (
        "kimi",
        "provider was removed (p16-provider-effort/D6); delete this entry",
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

/// Refuses `effort` on a model the provider's CLI is known to run without
/// reasoning effort: it ignores every level, so none can be honoured
/// (p16-provider-effort/D2). `why` names the evidence. Points at `effort`,
/// or the provider's entry when the loader had no location for it.
pub fn effort_unsupported(
    id: &str,
    settings: &ProviderSettings,
    locations: &ProviderLocations,
    why: &str,
) -> ConfigError {
    ConfigError::at(
        locations
            .effort
            .or(locations.provider)
            .unwrap_or(Location::UNKNOWN),
        format!(
            "providers entry \"{id}\".effort: model \"{}\" has no reasoning effort ({why}), so \"{}\" cannot be honoured, nor can any other level; choose a model with reasoning effort",
            settings.model,
            settings.effort.as_str()
        ),
    )
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

    fn settings(id: &str, model: &str) -> ProviderSettings {
        ProviderSettings {
            enabled: true,
            binary: PathBuf::from("cli"),
            model: model.to_owned(),
            timeout: Duration::from_secs(30),
            effort: Effort::Low,
            thinking: (id == "claude").then_some(false),
        }
    }

    #[test]
    fn effort_parses_exactly_the_three_levels() {
        for effort in Effort::ALL {
            assert_eq!(Effort::parse(effort.as_str()), Some(effort));
        }
        for other in ["", "Low", "LOW", "minimal", "xhigh", "max", " low"] {
            assert_eq!(Effort::parse(other), None, "{other:?}");
        }
    }

    #[test]
    fn only_claude_has_a_thinking_switch() {
        for d in PROVIDERS {
            assert_eq!(d.thinking_switch, d.id == "claude", "{}", d.id);
        }
    }

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
        for (id, model) in [("claude", "haiku"), ("codex", "gpt-6.1-sol"), ("kiro", "x")] {
            let d = descriptor(id).expect("registered");
            let provider = (d.build)(&settings(id, model), Arc::new(ProcessRunner::new()))
                .expect("builds with an explicit model");
            assert_eq!(provider.id(), id);
        }
    }

    #[test]
    fn claude_refuses_to_build_without_thinking() {
        let d = descriptor("claude").expect("registered");
        let mut settings = settings("claude", "haiku");
        settings.thinking = None;
        assert!((d.build)(&settings, Arc::new(ProcessRunner::new())).is_err());
    }

    #[test]
    fn claude_and_codex_accept_an_unknown_model() {
        for id in ["claude", "codex"] {
            let d = descriptor(id).expect("registered");
            assert!(
                (d.validate_settings)(&settings(id, "any"), &ProviderLocations::default()).is_ok(),
                "{id} accepts an explicit model"
            );
        }
    }
}
