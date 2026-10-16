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
    /// `cpu`, `auto` or `gpu` (see `crate::compute`).
    pub backend: Option<Spanned<String>>,
    pub server: Option<Spanned<RawServerConfig>>,
    /// Override for the per-user models directory used by `catalog:` entries.
    pub models_dir: Option<Spanned<PathBuf>>,
    #[serde(default)]
    pub models: Vec<Spanned<RawModelEntry>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawServerConfig {
    pub port: Option<Spanned<u16>>,
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
