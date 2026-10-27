//! YAML structures retaining source positions for precise validation errors.
//!
//! Every setting is a [`Setting`], which keeps an omitted key apart from an
//! explicit `null` and carries the value's position, so semantic validation
//! can require each key (p17-serve-model-names/D3) and point at the exact
//! value, or at the mapping a missing key belongs in. Removed keys are parsed
//! through [`removed_key`] so even an explicit `null` reaches validation and
//! fails at its line. `providers` is a sequence, parsed by [`ProviderList`],
//! so the removed mapping form can fail with a migration message instead of a
//! generic type error.

use std::fmt;
use std::path::PathBuf;

use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_saphyr::{Location, Spanned};

use super::ConfigError;

/// Shown when a removed setting or the removed mapping form of `providers`
/// is used; points at the list form that replaces it.
pub(crate) const PROVIDERS_LIST_FORM: &str = "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: claude\n    timeout_secs: 30\n    effort: low\n    thinking: false";

/// The commented reference configuration every missing-setting error points
/// at.
const EXAMPLE_FILE: &str = "relay.example.yml";

/// The configuration file before validation. Every setting is required;
/// an empty document is a file with every setting missing.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawConfig {
    #[serde(default)]
    pub port: Setting<u16>,
    /// Removed in 0.2; parsed only so the error can point at the key and
    /// show the list form that replaced it.
    #[serde(default, deserialize_with = "removed_key")]
    pub default_provider: Option<Spanned<serde::de::IgnoredAny>>,
    /// Removed in 0.2 (owner decision 2026-10-07): the client's `model`
    /// field picks the provider and a request without a model gets HTTP 400.
    /// Parsed only so the error can point at the key.
    #[serde(default, deserialize_with = "removed_key")]
    pub default: Option<Spanned<serde::de::IgnoredAny>>,
    #[serde(default)]
    pub total_timeout_secs: Setting<u64>,
    /// How many provider calls may run at the same time.
    #[serde(default)]
    pub max_parallel: Setting<u64>,
    #[serde(default)]
    pub providers: Setting<ProviderList>,
    /// Removed in 0.2; parsed only so the error can point at the key.
    #[serde(default, deserialize_with = "removed_key")]
    pub fallback_order: Option<Spanned<serde::de::IgnoredAny>>,
    /// Removed (owner decision 2026-10-08): the client app sends its own
    /// prompt and Pumice adds none. Parsed only so the error can point at
    /// the key.
    #[serde(default, deserialize_with = "removed_key")]
    pub prompts: Option<Spanned<serde::de::IgnoredAny>>,
    #[serde(default)]
    pub debug_log: Setting<RawDebugLog>,
}

/// One entry of the `providers:` list. Every key is required by semantic
/// validation (so the error can name the entry and point at it); `model`
/// may be `null` only on a disabled entry, and `thinking` belongs only to
/// providers with a thinking switch (Claude).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawProviderEntry {
    #[serde(default)]
    pub id: Setting<String>,
    #[serde(default)]
    pub enabled: Setting<bool>,
    #[serde(default)]
    pub binary: Setting<PathBuf>,
    #[serde(default)]
    pub model: Setting<String>,
    #[serde(default)]
    pub timeout_secs: Setting<u64>,
    /// `low`, `medium` or `high` (p16-provider-effort/D2), checked by
    /// semantic validation so the error can list the levels.
    #[serde(default)]
    pub effort: Setting<String>,
    #[serde(default)]
    pub thinking: Setting<bool>,
    /// Removed (owner decision 2026-10-08): each CLI keeps its own
    /// configuration. Parsed only so the error can point at the key.
    #[serde(default, deserialize_with = "removed_key")]
    pub env: Option<Spanned<serde::de::IgnoredAny>>,
    /// Removed (owner decision 2026-10-08), like `env`.
    #[serde(default, deserialize_with = "removed_key")]
    pub options: Option<Spanned<serde::de::IgnoredAny>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawDebugLog {
    #[serde(default)]
    pub enabled: Setting<bool>,
    #[serde(default)]
    pub path: Setting<PathBuf>,
}

/// One setting as written: omitted, an explicit `null` (also `~` or no
/// value), or a value. `Option<Spanned<T>>` cannot tell the first two apart:
/// serde-saphyr maps an explicit `null` to `None` before the field type runs.
/// Every field of this type needs `#[serde(default)]`.
#[derive(Debug, Default)]
pub(crate) enum Setting<T> {
    #[default]
    Missing,
    Null(Location),
    Value(Spanned<T>),
}

/// Where a missing setting is reported: the mapping it belongs in.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Within {
    /// The top level of the file, reported at line 1.
    Root,
    /// A nested mapping or list entry, reported at its first key.
    Mapping(Location),
}

impl<T> Setting<T> {
    /// The written value; an omitted key or an explicit `null` is an error.
    /// `name` is the setting as error messages show it, e.g. `"port"`.
    pub(crate) fn required(&self, within: Within, name: &str) -> Result<&Spanned<T>, ConfigError> {
        match self {
            Setting::Value(value) => Ok(value),
            Setting::Null(location) => Err(ConfigError::at(
                *location,
                format!("{name} must not be null; write a value"),
            )),
            Setting::Missing => Err(missing(within, name)),
        }
    }

    /// The written value, or `None` for an explicit `null`; an omitted key
    /// is an error.
    pub(crate) fn nullable(
        &self,
        within: Within,
        name: &str,
    ) -> Result<Option<&Spanned<T>>, ConfigError> {
        match self {
            Setting::Missing => Err(missing(within, name)),
            Setting::Null(_) => Ok(None),
            Setting::Value(value) => Ok(Some(value)),
        }
    }

    /// The position of a written value or `null`.
    pub(crate) fn location(&self) -> Option<Location> {
        match self {
            Setting::Missing => None,
            Setting::Null(location) => Some(*location),
            Setting::Value(value) => Some(value.referenced),
        }
    }
}

/// The error for a setting the file does not write.
pub(crate) fn missing(within: Within, name: &str) -> ConfigError {
    let message = format!("{name} is missing; every setting must be written (see {EXAMPLE_FILE})");
    match within {
        Within::Root => ConfigError::at_line(1, 1, message),
        Within::Mapping(location) => ConfigError::at(location, message),
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Setting<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let spanned = Spanned::<Option<T>>::deserialize(deserializer)?;
        Ok(match spanned.value {
            None => Setting::Null(spanned.referenced),
            Some(value) => Setting::Value(Spanned {
                value,
                referenced: spanned.referenced,
                defined: spanned.defined,
            }),
        })
    }
}

/// Records that a removed key is present, whatever its value. A plain
/// `Option` would read `default: null` as absent and silently accept it.
fn removed_key<'de, D>(deserializer: D) -> Result<Option<Spanned<serde::de::IgnoredAny>>, D::Error>
where
    D: Deserializer<'de>,
{
    Spanned::<serde::de::IgnoredAny>::deserialize(deserializer).map(Some)
}

/// The `providers:` sequence as located entries. The mapping form
/// (`providers: {claude: ...}` or `providers: {}`) was removed in 0.2: it
/// fails here with the list form that replaces it.
#[derive(Debug, Default)]
pub(crate) struct ProviderList(pub Vec<Spanned<RawProviderEntry>>);

impl<'de> Deserialize<'de> for ProviderList {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct List;

        impl<'de> Visitor<'de> for List {
            type Value = ProviderList;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a sequence of provider entries (- id: <provider>)")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = seq.next_element::<Spanned<RawProviderEntry>>()? {
                    entries.push(entry);
                }
                Ok(ProviderList(entries))
            }

            fn visit_map<A: MapAccess<'de>>(self, _map: A) -> Result<Self::Value, A::Error> {
                // One-line example: serde renders custom errors with escaped
                // newlines, unlike the semantic errors below.
                Err(serde::de::Error::custom(
                    "the mapping form of \"providers\" was removed in 0.2; use an ordered list instead, e.g. providers: [{id: claude, enabled: true, model: haiku, binary: claude, timeout_secs: 30, effort: low, thinking: false}]",
                ))
            }
        }

        deserializer.deserialize_any(List)
    }
}
