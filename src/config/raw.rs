//! YAML structures retaining source positions for precise validation errors.
//!
//! Every value that can fail semantic validation is wrapped in
//! [`Spanned`], and provider, environment and option *keys* go through
//! [`spanned_string_map`], a small map visitor: serde-saphyr does not
//! deserialize mappings into `Vec<(K, V)>`, and `flatten` or untagged
//! buffering would discard spans. Keeping spans through to semantic
//! validation is what lets an error point at the exact value or key that
//! was rejected.

use std::fmt;
use std::marker::PhantomData;
use std::path::PathBuf;

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_saphyr::Spanned;

/// The configuration file before defaults and validation.
///
/// Every field is optional; an empty or absent document means "everything
/// defaults".
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawConfig {
    pub port: Option<Spanned<u16>>,
    pub default_provider: Option<Spanned<String>>,
    pub total_timeout_secs: Option<Spanned<u64>>,
    #[serde(default, deserialize_with = "spanned_string_map")]
    pub providers: Vec<(Spanned<String>, RawProviderSettings)>,
    pub fallback_order: Option<Vec<Spanned<String>>>,
    pub prompts: Option<RawPrompts>,
    pub debug_log: Option<RawDebugLog>,
}

/// One provider's overrides. Unset fields keep the provider's defaults.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawProviderSettings {
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
