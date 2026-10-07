//! YAML structures retaining source positions for precise validation errors.
//!
//! Every value that can fail semantic validation is wrapped in
//! [`Spanned`], and environment and option *keys* go through
//! [`spanned_string_map`], a small map visitor: serde-saphyr does not
//! deserialize mappings into `Vec<(K, V)>`, and `flatten` or untagged
//! buffering would discard spans. `providers` is a sequence, parsed by
//! [`provider_list`], so the removed mapping form can fail with a migration
//! message instead of a generic type error. Keeping spans through to
//! semantic validation is what lets an error point at the exact value or key
//! that was rejected.

use std::fmt;
use std::marker::PhantomData;
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
    pub default_provider: Option<Spanned<serde::de::IgnoredAny>>,
    /// The provider a model-less request runs. Optional: when absent, or
    /// when it does not resolve to an enabled provider, startup succeeds
    /// with a warning and model-less requests return the original text. The
    /// inner option distinguishes an absent key (`None`) from an explicit
    /// null (`default:`, inner `None`), which warns as empty.
    #[serde(default, deserialize_with = "present_default_key")]
    pub default: Option<Spanned<Option<String>>>,
    pub total_timeout_secs: Option<Spanned<u64>>,
    #[serde(default, deserialize_with = "provider_list")]
    pub providers: Vec<Spanned<RawProviderEntry>>,
    /// Removed in 0.2; parsed only so the error can point at the key.
    pub fallback_order: Option<Spanned<serde::de::IgnoredAny>>,
    pub prompts: Option<RawPrompts>,
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
    #[serde(default, deserialize_with = "spanned_string_map")]
    pub env: Vec<(Spanned<String>, Spanned<String>)>,
    #[serde(default, deserialize_with = "spanned_string_map")]
    pub options: Vec<(Spanned<String>, Spanned<String>)>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawPrompts {
    pub system: Option<Spanned<String>>,
    pub user: Option<Spanned<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawDebugLog {
    pub enabled: Option<Spanned<bool>>,
    pub path: Option<Spanned<PathBuf>>,
}

/// Deserializes the `default:` key so an explicit null (`default:`) stays
/// distinguishable from an absent key: `deserialize_with` only runs when the
/// key is present, so the outer `Option` is `Some` for both a value and a
/// null, and the inner `Option` carries the null.
fn present_default_key<'de, D>(deserializer: D) -> Result<Option<Spanned<Option<String>>>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Some(Spanned::<Option<String>>::deserialize(deserializer)?))
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

/// Deserializes a YAML mapping into located `(key, value)` pairs, keeping
/// each key's own position so unknown or rejected keys can be reported at
/// the key rather than at the value. An explicit `null` (or an omitted
/// field, through `default`) yields an empty list.
fn spanned_string_map<'de, D, V>(deserializer: D) -> Result<Vec<(Spanned<String>, V)>, D::Error>
where
    D: Deserializer<'de>,
    V: Deserialize<'de>,
{
    struct Pairs<V>(PhantomData<V>);

    impl<'de, V: Deserialize<'de>> Visitor<'de> for Pairs<V> {
        type Value = Vec<(Spanned<String>, V)>;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a mapping with string keys")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut pairs = Vec::new();
            while let Some(key) = map.next_key::<Spanned<String>>()? {
                pairs.push((key, map.next_value::<V>()?));
            }
            Ok(pairs)
        }

        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
    }

    deserializer.deserialize_map(Pairs(PhantomData))
}
