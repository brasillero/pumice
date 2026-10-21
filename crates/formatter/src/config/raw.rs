//! YAML structures retaining source positions for precise validation errors.
//!
//! Every setting is a [`Setting`], which keeps an omitted key apart from an
//! explicit `null` and carries the value's position, so validation can
//! require each key (p17-serve-model-names/D3) while `default: null` keeps
//! its meaning (no default formatter).

use std::path::PathBuf;

use serde::{Deserialize, Deserializer};
use serde_saphyr::{Location, Spanned};

use super::ConfigError;

/// The commented reference configuration every missing-setting error points
/// at.
const EXAMPLE_FILE: &str = "formatter.example.yml";

/// The configuration file before validation. Every setting is required.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawConfig {
    #[serde(default)]
    pub schema_version: Setting<u32>,
    #[serde(default)]
    pub formatters: Setting<Vec<Spanned<RawFormatterEntry>>>,
    #[serde(default)]
    pub default: Setting<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawFormatterEntry {
    pub path: Option<Spanned<PathBuf>>,
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

impl<T> Setting<T> {
    /// The written value; an omitted key or an explicit `null` is an error.
    /// `name` is the top-level setting as error messages show it, e.g.
    /// `"formatters"`.
    pub(crate) fn required(&self, name: &str) -> Result<&Spanned<T>, ConfigError> {
        match self {
            Setting::Value(value) => Ok(value),
            Setting::Null(location) => Err(ConfigError::at(
                *location,
                format!("{name} must not be null; write a value"),
            )),
            Setting::Missing => Err(missing(name)),
        }
    }

    /// The written value, or `None` for an explicit `null`; an omitted key
    /// is an error.
    pub(crate) fn nullable(&self, name: &str) -> Result<Option<&Spanned<T>>, ConfigError> {
        match self {
            Setting::Missing => Err(missing(name)),
            Setting::Null(_) => Ok(None),
            Setting::Value(value) => Ok(Some(value)),
        }
    }
}

/// The error for a top-level setting the file does not write, reported at
/// line 1 (every setting of this file is top-level).
fn missing(name: &str) -> ConfigError {
    ConfigError::at_line(
        1,
        1,
        format!("{name} is missing; every setting must be written (see {EXAMPLE_FILE})"),
    )
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
