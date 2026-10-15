//! YAML structures retaining source positions for precise validation errors.

use std::path::PathBuf;

use serde::Deserialize;
use serde_saphyr::Spanned;

/// The configuration file before defaults and validation.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawConfig {
    pub schema_version: Option<Spanned<u32>>,
    #[serde(default)]
    pub formatters: Vec<Spanned<RawFormatterEntry>>,
    pub default: Option<Spanned<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawFormatterEntry {
    pub path: Option<Spanned<PathBuf>>,
}
