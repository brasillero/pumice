//! YAML structures retaining source positions for precise validation errors.
//!
//! Every setting is a [`Setting`], which keeps an omitted key apart from an
//! explicit `null` and carries the value's position, so validation can
//! require each key (p17-serve-model-names/D3) while `null` keeps its meaning
//! where it has one (`language: null` detects the language).

use std::fmt;
use std::path::PathBuf;

use serde::de::Visitor;
use serde::{Deserialize, Deserializer};
use serde_saphyr::{Location, Spanned};

use super::ConfigError;

/// The commented reference configuration every missing-setting error points
/// at.
const EXAMPLE_FILE: &str = "transcribe.example.yml";

/// The configuration file before validation. Every setting is required.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawConfig {
    #[serde(default)]
    pub schema_version: Setting<u32>,
    #[serde(default)]
    pub default_model: Setting<String>,
    #[serde(default)]
    pub language: Setting<String>,
    #[serde(default)]
    pub threads: Setting<RawThreads>,
    /// `cpu`, `auto` or `gpu` (see `crate::compute`).
    #[serde(default)]
    pub backend: Setting<String>,
    #[serde(default)]
    pub server: Setting<RawServerConfig>,
    /// The models directory for `catalog:` entries; `null` is the per-user
    /// data directory.
    #[serde(default)]
    pub models_dir: Setting<PathBuf>,
    #[serde(default)]
    pub models: Setting<Vec<Spanned<RawModelEntry>>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawServerConfig {
    #[serde(default)]
    pub port: Setting<u16>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawModelEntry {
    pub id: Option<Spanned<String>>,
    /// Local GGUF file; mutually exclusive with `catalog`.
    pub path: Option<Spanned<PathBuf>>,
    /// Built-in catalog id; the entry resolves to the catalog file inside the
    /// models directory. Mutually exclusive with `path`.
    pub catalog: Option<Spanned<String>>,
}

/// `threads:` as written: a number, or text (`auto` is the only valid one).
#[derive(Debug)]
pub(crate) enum RawThreads {
    Count(u64),
    Text(String),
}

impl<'de> Deserialize<'de> for RawThreads {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ThreadsVisitor;

        impl Visitor<'_> for ThreadsVisitor {
            type Value = RawThreads;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("auto or a thread count")
            }

            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<RawThreads, E> {
                Ok(RawThreads::Count(value))
            }

            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<RawThreads, E> {
                Ok(match u64::try_from(value) {
                    Ok(value) => RawThreads::Count(value),
                    Err(_) => RawThreads::Text(value.to_string()),
                })
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<RawThreads, E> {
                Ok(RawThreads::Text(value.to_owned()))
            }
        }

        deserializer.deserialize_any(ThreadsVisitor)
    }
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
    /// A nested mapping, reported at its first key.
    Mapping(Location),
}

impl<T> Setting<T> {
    /// The written value; an omitted key or an explicit `null` is an error.
    /// `name` is the setting as error messages show it, e.g. `"backend"`.
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
}

/// The error for a setting the file does not write.
fn missing(within: Within, name: &str) -> ConfigError {
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
