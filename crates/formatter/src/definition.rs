//! Formatter definition files: one formatter per YAML file.
//!
//! A definition is one document with four keys:
//!
//! ```yaml
//! schema_version: 1
//! id: clean            # [a-z0-9-]+, never the reserved "builtin:" namespace
//! description: …       # short, human-readable
//! system: …            # the rules, injected as one system message (≤ 16 KiB)
//! ```
//!
//! Validation reports `file:line:column` through
//! [`crate::config::ConfigError`], mirroring the transcription module's
//! config style; every semantic error points into the definition file itself.

use std::path::Path;

use serde::Deserialize;
use serde_saphyr::Spanned;

use crate::builtins::BUILTIN_PREFIX;
use crate::config::{self, ConfigError};

/// The maximum size of a definition's `system` text: 16 KiB.
pub const MAX_SYSTEM_BYTES: usize = 16 * 1024;

/// A validated formatter definition.
///
/// Holds the `system` rules text, so it deliberately has no `Debug`: dictated
/// content must never reach a log by accident. The `id` and `description`
/// are safe.
#[derive(Clone, PartialEq, Eq)]
pub struct FormatterDefinition {
    pub id: String,
    pub description: String,
    pub system: String,
}

impl std::fmt::Debug for FormatterDefinition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FormatterDefinition")
            .field("id", &self.id)
            .field("description", &self.description)
            .field("system_len", &self.system.len())
            .finish()
    }
}

/// Loads and validates one definition file.
///
/// Every error carries this file's path; semantic errors carry the
/// line:column of the offending value inside this file.
pub fn load_definition_file(path: &Path) -> Result<FormatterDefinition, ConfigError> {
    let (_, definition) = load_definition_file_located(path)?;
    Ok(definition)
}

/// [`load_definition_file`] plus the source position of the `id` value, so
/// callers (the `formatter.yml` loader) can report duplicate ids with a
/// position inside the definition file.
pub(crate) fn load_definition_file_located(
    path: &Path,
) -> Result<(serde_saphyr::Location, FormatterDefinition), ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        ConfigError::file(path, format!("cannot read formatter definition: {error}"))
    })?;
    let raw = serde_saphyr::from_str_with_options::<Option<RawDefinition>>(
        &text,
        config::saphyr_options(),
    )
    .map_err(|error| config::parse_error(path, &error))?;
    validate(raw.unwrap_or_default()).map_err(|error| error.with_file(path))
}

fn validate(
    raw: RawDefinition,
) -> Result<(serde_saphyr::Location, FormatterDefinition), ConfigError> {
    match &raw.schema_version {
        None => {
            return Err(ConfigError::general(
                "schema_version is required (must be 1)",
            ));
        }
        Some(span) if span.value != 1 => {
            return Err(ConfigError::at(
                span.referenced,
                format!("schema_version must be 1, got {}", span.value),
            ));
        }
        _ => {}
    }

    let id = match &raw.id {
        Some(id) => id,
        None => return Err(ConfigError::general("formatter definition has no \"id\"")),
    };
    if id.value.starts_with(BUILTIN_PREFIX) {
        return Err(ConfigError::at(
            id.referenced,
            format!(
                "formatter id \"{}\" uses the reserved \"builtin:\" namespace",
                id.value
            ),
        ));
    }
    if id.value.is_empty() || !is_valid_id(&id.value) {
        return Err(ConfigError::at(
            id.referenced,
            format!(
                "formatter id \"{}\" is invalid; ids must match [a-z0-9-]+",
                id.value
            ),
        ));
    }
    if id.value.starts_with(BUILTIN_PREFIX) {
        return Err(ConfigError::at(
            id.referenced,
            format!(
                "formatter id \"{}\" uses the reserved \"builtin:\" namespace",
                id.value
            ),
        ));
    }

    let description = match &raw.description {
        Some(description) => description,
        None => {
            return Err(ConfigError::at(
                id.referenced,
                format!("formatter \"{}\" has no \"description\"", id.value),
            ));
        }
    };
    if description.value.trim().is_empty() {
        return Err(ConfigError::at(
            description.referenced,
            format!("formatter \"{}\" description must not be empty", id.value),
        ));
    }

    let system = match &raw.system {
        Some(system) => system,
        None => {
            return Err(ConfigError::at(
                id.referenced,
                format!("formatter \"{}\" has no \"system\" text", id.value),
            ));
        }
    };
    if system.value.trim().is_empty() {
        return Err(ConfigError::at(
            system.referenced,
            format!("formatter \"{}\" system text must not be empty", id.value),
        ));
    }
    if system.value.len() > MAX_SYSTEM_BYTES {
        return Err(ConfigError::at(
            system.referenced,
            format!(
                "formatter \"{}\" system text is {} bytes; the maximum is 16 KiB",
                id.value,
                system.value.len()
            ),
        ));
    }

    Ok((
        raw.id
            .as_ref()
            .map(|id| id.referenced)
            .unwrap_or(serde_saphyr::Location::UNKNOWN),
        FormatterDefinition {
            id: id.value.clone(),
            description: description.value.clone(),
            system: system.value.clone(),
        },
    ))
}

/// Ids are `[a-z0-9-]+` (non-empty), matching the formatter config contract.
fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDefinition {
    pub schema_version: Option<Spanned<u32>>,
    pub id: Option<Spanned<String>>,
    pub description: Option<Spanned<String>>,
    pub system: Option<Spanned<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_definition(tmp: &tempfile::TempDir, text: &str) -> std::path::PathBuf {
        let path = tmp.path().join("formatter.yml");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(text.as_bytes()).unwrap();
        path
    }

    fn load(tmp: &tempfile::TempDir, text: &str) -> Result<FormatterDefinition, ConfigError> {
        load_definition_file(&write_definition(tmp, text))
    }

    const VALID: &str = "\
schema_version: 1
id: clean
description: Light dictation correction
system: Correct the text.
";

    #[test]
    fn valid_definition_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let definition = load(&tmp, VALID).unwrap();
        assert_eq!(definition.id, "clean");
        assert_eq!(definition.description, "Light dictation correction");
        assert_eq!(definition.system, "Correct the text.");
    }

    #[test]
    fn missing_schema_version_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let err = load(&tmp, "id: clean\n").unwrap_err();
        assert!(err.to_string().contains("schema_version is required"));
    }

    #[test]
    fn wrong_schema_version_reports_line() {
        let tmp = tempfile::tempdir().unwrap();
        let err = load(&tmp, "schema_version: 2\n").unwrap_err();
        let err = err.to_string();
        assert!(err.contains("schema_version must be 1"), "{err}");
        assert!(err.contains(":1:"), "line reported: {err}");
    }

    #[test]
    fn unknown_key_reports_line() {
        let tmp = tempfile::tempdir().unwrap();
        let err = load(&tmp, "schema_version: 1\nbogus: 5\n").unwrap_err();
        let err = err.to_string();
        assert!(err.contains("unknown setting `bogus`"), "{err}");
        assert!(err.contains(":2:"), "line reported: {err}");
    }

    #[test]
    fn missing_id_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let err = load(&tmp, "schema_version: 1\ndescription: x\nsystem: y\n").unwrap_err();
        assert!(err.to_string().contains("has no \"id\""));
    }

    #[test]
    fn invalid_id_reports_the_value() {
        let tmp = tempfile::tempdir().unwrap();
        let err = load(
            &tmp,
            "schema_version: 1\nid: \"Bad Id\"\ndescription: x\nsystem: y\n",
        )
        .unwrap_err();
        let err = err.to_string();
        assert!(err.contains("formatter id \"Bad Id\" is invalid"), "{err}");
        assert!(err.contains(":2:"), "line reported: {err}");
    }

    #[test]
    fn builtin_namespace_is_reserved() {
        let tmp = tempfile::tempdir().unwrap();
        let err = load(
            &tmp,
            "schema_version: 1\nid: builtin:clean\ndescription: x\nsystem: y\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("reserved \"builtin:\""));
    }

    #[test]
    fn empty_description_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let err = load(
            &tmp,
            "schema_version: 1\nid: clean\ndescription: \"  \"\nsystem: y\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("description must not be empty"));
    }

    #[test]
    fn missing_system_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let err = load(&tmp, "schema_version: 1\nid: clean\ndescription: x\n").unwrap_err();
        assert!(err.to_string().contains("has no \"system\" text"));
    }

    #[test]
    fn oversized_system_reports_size() {
        let tmp = tempfile::tempdir().unwrap();
        let big = "x".repeat(MAX_SYSTEM_BYTES + 1);
        let err = load(
            &tmp,
            &format!("schema_version: 1\nid: clean\ndescription: x\nsystem: {big}\n"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("the maximum is 16 KiB"));
    }

    #[test]
    fn missing_file_names_the_path() {
        let path = Path::new("/tmp/does-not-exist-pumice-formatter.yml");
        let err = load_definition_file(path).unwrap_err();
        let err = err.to_string();
        assert!(err.contains(path.display().to_string().as_str()), "{err}");
        assert!(err.contains("cannot read"), "{err}");
    }
}
