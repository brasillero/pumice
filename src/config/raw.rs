//! YAML structures retaining source positions for precise validation errors.
//!
//! Every value that can fail semantic validation is wrapped in
//! [`Spanned`]. Removed keys are parsed through [`removed_key`] so even an
//! explicit `null` reaches validation and fails at its line. `providers` is
//! a sequence, parsed by
//! [`provider_list`], so the removed mapping form can fail with a migration
//! message instead of a generic type error. Keeping spans through to
//! semantic validation is what lets an error point at the exact value or key
//! that was rejected.

use std::fmt;
use std::path::PathBuf;

use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_saphyr::Spanned;

/// Shown when a removed setting or the removed mapping form of `providers`
/// is used; points at the list form that replaces it.
pub(crate) const PROVIDERS_LIST_FORM: &str =
    "providers:\n  - id: claude\n    enabled: true\n    model: haiku";

/// The configuration file before defaults and validation.
///
/// Every field is optional; an empty or absent document means "everything
/// defaults" — which since 0.2 includes no providers at all.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawConfig {
    pub port: Option<Spanned<u16>>,
    /// Removed in 0.2; parsed only so the error can point at the key and
    /// show the list form that replaced it.
    #[serde(default, deserialize_with = "removed_key")]
    pub default_provider: Option<Spanned<serde::de::IgnoredAny>>,
    /// Removed in 0.2 (owner decision 2026-10-07): the client's `model`
    /// field picks the provider and a request without a model returns the
    /// original text. Parsed only so the error can point at the key.
    #[serde(default, deserialize_with = "removed_key")]
    pub default: Option<Spanned<serde::de::IgnoredAny>>,
    pub total_timeout_secs: Option<Spanned<u64>>,
    /// How many provider calls may run at the same time.
    pub max_parallel: Option<Spanned<u64>>,
    #[serde(default, deserialize_with = "provider_list")]
    pub providers: Vec<Spanned<RawProviderEntry>>,
    /// Removed in 0.2; parsed only so the error can point at the key.
    #[serde(default, deserialize_with = "removed_key")]
    pub fallback_order: Option<Spanned<serde::de::IgnoredAny>>,
    /// Removed (owner decision 2026-10-08): the client app sends its own
    /// prompt and Pumice adds none. Parsed only so the error can point at
    /// the key.
    #[serde(default, deserialize_with = "removed_key")]
    pub prompts: Option<Spanned<serde::de::IgnoredAny>>,
    pub debug_log: Option<RawDebugLog>,
}

/// One entry of the `providers:` list. `id` and `enabled` are required by
/// semantic validation (so the error can name the entry and point at it);
/// `model` is required when `enabled` is `true`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawProviderEntry {
    pub id: Option<Spanned<String>>,
    pub enabled: Option<Spanned<bool>>,
    /// `binary: null` (or omitted) keeps the command-name default.
    pub binary: Option<Spanned<PathBuf>>,
    pub model: Option<Spanned<String>>,
    pub timeout_secs: Option<Spanned<u64>>,
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
    pub enabled: Option<Spanned<bool>>,
    pub path: Option<Spanned<PathBuf>>,
}

/// Records that a removed key is present, whatever its value. A plain
/// `Option` would read `default: null` as absent and silently accept it.
fn removed_key<'de, D>(deserializer: D) -> Result<Option<Spanned<serde::de::IgnoredAny>>, D::Error>
where
    D: Deserializer<'de>,
{
    Spanned::<serde::de::IgnoredAny>::deserialize(deserializer).map(Some)
}

/// Deserializes the `providers:` sequence into located entries. The mapping
/// form (`providers: {claude: ...}` or `providers: {}`) was removed in 0.2:
/// it fails here with the list form that replaces it. An explicit `null`
/// (or an omitted field, through `default`) yields an empty list.
fn provider_list<'de, D>(deserializer: D) -> Result<Vec<Spanned<RawProviderEntry>>, D::Error>
where
    D: Deserializer<'de>,
{
    struct List;

    impl<'de> Visitor<'de> for List {
        type Value = Vec<Spanned<RawProviderEntry>>;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a sequence of provider entries (- id: <provider>)")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut entries = Vec::new();
            while let Some(entry) = seq.next_element::<Spanned<RawProviderEntry>>()? {
                entries.push(entry);
            }
            Ok(entries)
        }

        fn visit_map<A: MapAccess<'de>>(self, _map: A) -> Result<Self::Value, A::Error> {
            // One-line example: serde renders custom errors with escaped
            // newlines, unlike the semantic errors below.
            Err(serde::de::Error::custom(
                "the mapping form of \"providers\" was removed in 0.2; use an ordered list instead, e.g. providers: [{id: claude, enabled: true, model: haiku}]",
            ))
        }

        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
    }

    deserializer.deserialize_any(List)
}
