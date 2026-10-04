//! Providers and their registry.
//!
//! A provider is a module exporting a [`ProviderDescriptor`] named
//! `DESCRIPTOR`. Adding one is a new module plus one entry in
//! `register_providers!` below.

pub mod cli;
mod interface;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub use interface::{
    FormatInput, Provider, ProviderError, ProviderErrorCode, ProviderFuture, UserPrompt,
};

use crate::process::ProcessRunner;

/// Settings shared by every CLI provider.
///
/// Placeholder until the configuration file (S6.1) adds located settings,
/// `enabled`, environment overrides and provider options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderSettings {
    /// Command name looked up on PATH, or a path to the CLI.
    pub binary: PathBuf,
    pub model: String,
    pub timeout: Duration,
}

/// What the registry knows about one provider.
pub struct ProviderDescriptor {
    pub id: &'static str,
    pub defaults: fn() -> ProviderSettings,
    pub build: fn(&ProviderSettings, Arc<ProcessRunner>) -> Arc<dyn Provider>,
}

/// Declares provider modules and lists their descriptors in [`PROVIDERS`].
macro_rules! register_providers {
    ($($module:ident),+ $(,)?) => {
        $(pub mod $module;)+

        /// Every known provider, in registration order.
        pub static PROVIDERS: &[ProviderDescriptor] = &[$($module::DESCRIPTOR),+];
    };
}

register_providers!(claude);

/// Looks up a provider by ID.
pub fn descriptor(id: &str) -> Option<&'static ProviderDescriptor> {
    PROVIDERS.iter().find(|d| d.id == id)
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
        assert_eq!(settings.binary, PathBuf::from("claude"));
        assert_eq!(settings.model, "haiku");
        let provider = (d.build)(&settings, Arc::new(ProcessRunner::new()));
        assert_eq!(provider.id(), "claude");
    }
}
