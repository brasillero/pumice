//! YAML structures retaining source positions for precise validation errors.

use std::path::PathBuf;

use serde::Deserialize;
use serde_saphyr::Spanned;

/// The configuration file before defaults and validation.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawConfig {
    pub schema_version: Option<Spanned<u32>>,
    pub default_model: Option<Spanned<String>>,
    pub language: Option<Spanned<String>>,
    pub threads: Option<Spanned<usize>>,
    #[serde(default)]
    pub models: Vec<Spanned<RawModelEntry>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawModelEntry {
    pub id: Option<Spanned<String>>,
    pub path: Option<Spanned<PathBuf>>,
}
