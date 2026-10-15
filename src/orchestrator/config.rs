//! Validated `orchestrator.yml` configuration for `pumice run`
//! (p12-pumice-run).
//!
//! The orchestrator file refers to the module configurations and declares
//! named flows; it never copies module settings (arch-modules/D7). The shape:
//!
//! ```yaml
//! schema_version: 1
//! modules:
//!   transcription:
//!     config: ./transcribe.yml
//!   formatter:
//!     config: ./formatter.yml
//!   relay:
//!     location: in_process
//!     config: ./relay.yml
//! flows:
//!   - id: clean
//!     enabled: true
//!     transcription_model: turbo
//!     formatter: builtin:clean      # an id, or an explicit null
//!     relay_route: claude
//!     budgets_ms:                   # optional; every key has a default (D5)
//!       total: 180000
//!       transcription: 120000
//!       formatter: 1000
//!       relay: 60000
//! ```
//!
//! Nothing is enabled implicitly: `enabled` defaults to false, `formatter`
//! must be present even as an explicit null, and `relay_route` is always
//! explicit (D4). Validation reports `file:line:column` like the other
//! modules; relative module paths resolve against this file's directory.
//!
//! An optional `serve:` section configures `pumice serve`
//! (p13.1-flow-seam/D3); files without it stay valid for `pumice run`:
//!
//! ```yaml
//! serve:
//!   enabled: true                # default false
//!   port: 17570                  # optional; 7567 is refused, 0 picks a free port
//!   transcription_model: turbo   # required when enabled
//!   startup_timeout_ms: 60000    # optional, 1000..=300000
//! ```
//!
//! The section's own shape is checked at load; the rules that tie it to the
//! flows (every enabled flow uses the served model) are checked only by
//! `pumice serve`, through [`serve_settings`].

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde_saphyr::Spanned;

use crate::orchestrator::error::ConfigError;

/// `budgets_ms.total` must leave room for this teardown/output reserve
/// (p12-pumice-run/D2: 2.5 s are reserved for cleanup and output).
pub const TEARDOWN_RESERVE: Duration = Duration::from_millis(2_500);

/// Upper bound for every budget value: one hour in milliseconds.
const MAX_BUDGET_MS: u64 = 3_600_000;

const DEFAULT_TOTAL_MS: u64 = 180_000;
const DEFAULT_TRANSCRIPTION_MS: u64 = 120_000;
const DEFAULT_FORMATTER_MS: u64 = 1_000;
const DEFAULT_RELAY_MS: u64 = 60_000;

const PER_USER_FILE: &str = "orchestrator.yml";

/// `pumice serve`'s default frontend port (p13.1-flow-seam/D3).
pub const DEFAULT_SERVE_PORT: u16 = 17570;
/// The live relay's port: never accepted for `pumice serve` (D3).
pub const RESERVED_RELAY_PORT: u16 = 7567;
const DEFAULT_STARTUP_TIMEOUT_MS: u64 = 60_000;
const MIN_STARTUP_TIMEOUT_MS: u64 = 1_000;
const MAX_STARTUP_TIMEOUT_MS: u64 = 300_000;
/// The managed transcription child announces model ids of at most this many
/// bytes (crates/transcription/src/managed.rs `MAX_MODEL_ID_BYTES`; kept in
/// sync by hand, `pumice` does not depend on the transcription crate).
const MAX_SERVED_MODEL_BYTES: usize = 512;
/// `pumice serve` publishes at most this many enabled flows, each with an id
/// of at most this many bytes (design §7: no silent truncation).
pub const MAX_SERVED_FLOWS: usize = 128;
pub const MAX_SERVED_FLOW_ID_BYTES: usize = 128;

/// Validated orchestrator configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub modules: Modules,
    /// Every flow in file (list) order, enabled or not.
    pub flows: Vec<Flow>,
    /// The `serve:` section, when the file has one.
    pub serve: Option<Serve>,
}

/// The `serve:` section, with its shape already checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Serve {
    pub enabled: bool,
    pub port: u16,
    /// Required when `enabled`; checked at load.
    pub transcription_model: Option<String>,
    pub startup_timeout: Duration,
    /// Source positions for errors `pumice serve` reports later.
    pub spans: ServeSpans,
}

/// The 1-based (line, column) of the `serve:` section and of its keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ServeSpans {
    pub section: Option<(u64, u64)>,
    pub enabled: Option<(u64, u64)>,
}

/// What `pumice serve` runs with, after [`serve_settings`] checked the
/// section against the flows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServeSettings {
    pub port: u16,
    pub transcription_model: String,
    pub startup_timeout: Duration,
}

/// The module references. `formatter` is optional only when no enabled flow
/// selects a formatter (validated at load).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Modules {
    pub transcription: ModuleRef,
    pub formatter: Option<ModuleRef>,
    pub relay: ModuleRef,
}

/// A module's configuration file, already resolved to an absolute path
/// against the orchestrator file's directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleRef {
    pub config: PathBuf,
}

/// One named flow: which transcription model, formatter and relay route a
/// `pumice run --flow <id>` combines, and its budgets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flow {
    pub id: String,
    pub enabled: bool,
    pub transcription_model: String,
    /// The formatter to apply; `None` is the explicit `formatter: null`
    /// (no system message is added; the route still runs — D4).
    pub formatter: Option<String>,
    pub relay_route: String,
    pub budgets: Budgets,
    /// Source positions of the cross-file references, for errors that name
    /// them (`file:line:column` in the orchestrator file).
    pub references: ReferenceSpans,
}

/// The 1-based (line, column) of each cross-file reference's value in the
/// orchestrator file; `None` when the span was lost in parsing (anchors,
/// serde buffering).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReferenceSpans {
    pub id: Option<(u64, u64)>,
    pub transcription_model: Option<(u64, u64)>,
    pub formatter: Option<(u64, u64)>,
    pub relay_route: Option<(u64, u64)>,
}

/// The position of a parsed value, or `None` when the span is unknown
/// (mirrors [`ConfigError::at`]'s degradation).
fn position(location: serde_saphyr::Location) -> Option<(u64, u64)> {
    (location.line() != 0).then(|| (location.line(), location.column()))
}

/// Per-flow budgets; the stage deadlines can only shorten the overall one
/// (D2). Defaults per D5.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budgets {
    pub total: Duration,
    pub transcription: Duration,
    pub formatter: Duration,
    pub relay: Duration,
}

impl Default for Budgets {
    fn default() -> Budgets {
        Budgets {
            total: Duration::from_millis(DEFAULT_TOTAL_MS),
            transcription: Duration::from_millis(DEFAULT_TRANSCRIPTION_MS),
            formatter: Duration::from_millis(DEFAULT_FORMATTER_MS),
            relay: Duration::from_millis(DEFAULT_RELAY_MS),
        }
    }
}

/// The effective configuration and the file it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedConfig {
    pub config: Config,
    /// The orchestrator file; always a file: unlike the module
    /// configurations, an absent orchestrator file is an error.
    pub path: PathBuf,
}

impl LoadedConfig {
    /// Looks up a flow by its exact, case-sensitive id.
    pub fn flow(&self, id: &str) -> Option<&Flow> {
        self.config.flows.iter().find(|flow| flow.id == id)
    }
}

/// Loads the orchestrator configuration: the explicit `--config` path when
/// given, else the per-user `orchestrator.yml`. A missing file — explicit or
/// per-user — is an error (D6).
pub fn load(explicit: Option<&Path>) -> Result<LoadedConfig, ConfigError> {
    load_with_env(explicit, |key| std::env::var_os(key))
}

/// [`load`] with an injectable environment reader, so tests can point the
/// per-user lookup at a temporary directory without touching the process
/// environment.
pub fn load_with_env(
    explicit: Option<&Path>,
    get_env: impl Fn(&str) -> Option<OsString>,
) -> Result<LoadedConfig, ConfigError> {
    let path = match explicit {
        Some(path) => {
            if !path.exists() {
                return Err(ConfigError::file(
                    path,
                    "the configuration file does not exist",
                ));
            }
            path.to_path_buf()
        }
        None => match per_user_config_path(&get_env) {
            Some(candidate) if candidate.exists() => candidate,
            Some(candidate) => {
                return Err(ConfigError::file(
                    candidate,
                    "the orchestrator configuration file does not exist; \
                     pass --config <FILE> or create it",
                ));
            }
            None => {
                return Err(ConfigError::general(
                    "no per-user configuration directory found \
                     (XDG_CONFIG_HOME, HOME or APPDATA must be a nonempty \
                     absolute path); pass --config <FILE>",
                ));
            }
        },
    };
    let text = std::fs::read_to_string(&path).map_err(|error| {
        ConfigError::file(&path, format!("cannot read configuration file: {error}"))
    })?;
    let config_dir = config_dir_of(&path);
    let raw = serde_saphyr::from_str_with_options::<Option<RawConfig>>(&text, saphyr_options())
        .map_err(|error| parse_error(&path, &error))?;
    let config =
        validate(raw.unwrap_or_default(), &config_dir).map_err(|error| error.with_file(&path))?;
    Ok(LoadedConfig {
        config,
        path: std::path::absolute(&path).unwrap_or(path),
    })
}

/// Shared YAML parsing options: strict, with positions for error reporting.
fn saphyr_options() -> serde_saphyr::Options {
    serde_saphyr::options! {
        reject_unsupported_tags: true,
        with_snippet: false,
    }
}

/// Directory that relative module paths in the file resolve against. Made
/// absolute so a later working-directory change cannot break stored paths.
fn config_dir_of(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    match absolute.parent() {
        Some(parent) => parent.to_path_buf(),
        None => PathBuf::from("."),
    }
}

/// Resolves a relative module path against the orchestrator file's
/// directory; inner `.` components are normalized away so error locations
/// and child arguments name real paths.
fn resolve_against(path: &Path, config_dir: &Path) -> PathBuf {
    let joined = if path.is_relative() {
        config_dir.join(path)
    } else {
        path.to_path_buf()
    };
    joined.components().collect()
}

fn validate(raw: RawConfig, config_dir: &Path) -> Result<Config, ConfigError> {
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

    let modules = validate_modules(raw.modules, config_dir)?;

    let mut flows: Vec<Flow> = Vec::new();
    for entry in &raw.flows {
        let flow = validate_flow(entry)?;
        if flows.iter().any(|existing| existing.id == flow.id) {
            let duplicate = entry
                .value
                .id
                .as_ref()
                .map_or(entry.referenced, |id| id.referenced);
            return Err(ConfigError::at(
                duplicate,
                format!("duplicate flow id \"{}\" in \"flows\"", flow.id),
            ));
        }
        flows.push(flow);
    }

    // A formatter config is required once an enabled flow selects one — even
    // a built-in (an empty registry file is enough). Disabled flows and
    // explicit nulls need no formatter module (design §2).
    if modules.formatter.is_none()
        && let Some((entry, flow)) = raw
            .flows
            .iter()
            .zip(&flows)
            .find(|(_, flow)| flow.enabled && flow.formatter.is_some())
    {
        let span = entry.value.formatter.location().unwrap_or(entry.referenced);
        return Err(ConfigError::at(
            span,
            format!(
                "flow \"{}\" selects a formatter but \"modules.formatter\" is not configured",
                flow.id
            ),
        ));
    }

    let serve = raw.serve.map(validate_serve).transpose()?;

    Ok(Config {
        modules,
        flows,
        serve,
    })
}

fn validate_serve(raw: Spanned<RawServe>) -> Result<Serve, ConfigError> {
    let section = raw.referenced;
    let serve = raw.value;
    let enabled = serve.enabled.as_ref().is_some_and(|span| span.value);
    let port = match &serve.port {
        None => DEFAULT_SERVE_PORT,
        Some(span) => check_serve_port(span.value)
            .map_err(|message| ConfigError::at(span.referenced, message))?,
    };
    let transcription_model = match &serve.transcription_model {
        None if enabled => {
            return Err(ConfigError::at(
                section,
                "\"serve.transcription_model\" is required when serve is enabled; \
                 name the model every enabled flow uses",
            ));
        }
        None => None,
        Some(span) if span.value.trim().is_empty() => {
            return Err(ConfigError::at(
                span.referenced,
                "serve.transcription_model must not be empty",
            ));
        }
        Some(span) if span.value.len() > MAX_SERVED_MODEL_BYTES => {
            return Err(ConfigError::at(
                span.referenced,
                format!("serve.transcription_model must be at most {MAX_SERVED_MODEL_BYTES} bytes"),
            ));
        }
        Some(span) => Some(span.value.clone()),
    };
    let startup_timeout = match &serve.startup_timeout_ms {
        None => Duration::from_millis(DEFAULT_STARTUP_TIMEOUT_MS),
        Some(span) if !(MIN_STARTUP_TIMEOUT_MS..=MAX_STARTUP_TIMEOUT_MS).contains(&span.value) => {
            return Err(ConfigError::at(
                span.referenced,
                format!(
                    "serve.startup_timeout_ms must be between {MIN_STARTUP_TIMEOUT_MS} and {MAX_STARTUP_TIMEOUT_MS}"
                ),
            ));
        }
        Some(span) => Duration::from_millis(span.value),
    };
    Ok(Serve {
        enabled,
        port,
        transcription_model,
        startup_timeout,
        spans: ServeSpans {
            section: position(section),
            enabled: serve
                .enabled
                .as_ref()
                .and_then(|span| position(span.referenced)),
        },
    })
}

/// The frontend port rule shared by `serve.port` and `pumice serve --port`:
/// any port but the live relay's (D3). Port 0 is accepted when given
/// explicitly; the chosen port is announced.
pub fn check_serve_port(port: u16) -> Result<u16, String> {
    if port == RESERVED_RELAY_PORT {
        return Err(format!(
            "port {RESERVED_RELAY_PORT} is reserved for the relay service; pumice serve \
             uses another port (default {DEFAULT_SERVE_PORT})"
        ));
    }
    Ok(port)
}

/// The settings `pumice serve` runs with: the section must be present and
/// enabled, and every enabled flow must use the served transcription model
/// (p13.1-flow-seam/D4), with at most [`MAX_SERVED_FLOWS`] enabled flows
/// whose ids fit [`MAX_SERVED_FLOW_ID_BYTES`]. Errors carry the
/// orchestrator file and, when known, the offending value's position.
pub fn serve_settings(loaded: &LoadedConfig) -> Result<ServeSettings, ConfigError> {
    let path = &loaded.path;
    let located = |at: Option<(u64, u64)>, message: String| {
        ConfigError::at_file(
            path,
            at.map(|(line, _)| line),
            at.map(|(_, column)| column),
            message,
        )
    };
    let Some(serve) = &loaded.config.serve else {
        return Err(ConfigError::file(
            path,
            "\"serve\" is not configured; add a \"serve:\" section with \"enabled: true\" \
             and \"transcription_model\" to run pumice serve",
        ));
    };
    if !serve.enabled {
        return Err(located(
            serve.spans.enabled.or(serve.spans.section),
            "serve is not enabled; set \"serve.enabled: true\" to run pumice serve".to_owned(),
        ));
    }
    let Some(model) = &serve.transcription_model else {
        // `validate_serve` requires the model once enabled.
        return Err(located(
            serve.spans.section,
            "\"serve.transcription_model\" is required when serve is enabled".to_owned(),
        ));
    };
    let mut enabled_flows = 0usize;
    for flow in loaded.config.flows.iter().filter(|flow| flow.enabled) {
        enabled_flows += 1;
        if enabled_flows > MAX_SERVED_FLOWS {
            return Err(located(
                flow.references.id,
                format!("pumice serve publishes at most {MAX_SERVED_FLOWS} enabled flows"),
            ));
        }
        if flow.id.len() > MAX_SERVED_FLOW_ID_BYTES {
            return Err(located(
                flow.references.id,
                format!(
                    "pumice serve needs enabled flow ids of at most {MAX_SERVED_FLOW_ID_BYTES} bytes"
                ),
            ));
        }
        if &flow.transcription_model != model {
            return Err(located(
                flow.references.transcription_model,
                format!(
                    "flow \"{}\" uses a different transcription model than \
                     \"serve.transcription_model\"; every enabled flow must use the served model",
                    flow.id
                ),
            ));
        }
    }
    Ok(ServeSettings {
        port: serve.port,
        transcription_model: model.clone(),
        startup_timeout: serve.startup_timeout,
    })
}

fn validate_modules(
    raw: Option<Spanned<RawModules>>,
    config_dir: &Path,
) -> Result<Modules, ConfigError> {
    let Some(modules) = raw else {
        return Err(ConfigError::general(
            "\"modules\" is required: it names the transcription, formatter and relay configuration files",
        ));
    };
    let modules_span = modules.referenced;
    let modules = modules.value;

    let transcription = match modules.transcription {
        Some(entry) => module_ref(
            entry.referenced,
            entry.value.config,
            "transcription",
            config_dir,
        )?,
        None => {
            return Err(ConfigError::at(
                modules_span,
                "\"modules.transcription\" is required, with its \"config\" file",
            ));
        }
    };
    let formatter = match modules.formatter {
        Some(entry) => Some(module_ref(
            entry.referenced,
            entry.value.config,
            "formatter",
            config_dir,
        )?),
        None => None,
    };
    let relay = match modules.relay {
        Some(entry) => {
            let entry_span = entry.referenced;
            let relay = entry.value;
            match &relay.location {
                None => {
                    return Err(ConfigError::at(
                        entry_span,
                        "\"modules.relay.location\" is required (only \"in_process\" is supported)",
                    ));
                }
                Some(location) if location.value != "in_process" => {
                    return Err(ConfigError::at(
                        location.referenced,
                        "modules.relay.location must be \"in_process\"; external relays are not supported yet",
                    ));
                }
                _ => {}
            }
            module_ref(entry_span, relay.config, "relay", config_dir)?
        }
        None => {
            return Err(ConfigError::at(
                modules_span,
                "\"modules.relay\" is required, with \"location: in_process\" and its \"config\" file",
            ));
        }
    };

    Ok(Modules {
        transcription,
        formatter,
        relay,
    })
}

/// A module reference: its `config` path, resolved against the orchestrator
/// file's directory.
fn module_ref(
    entry_span: serde_saphyr::Location,
    config: Option<Spanned<PathBuf>>,
    name: &str,
    config_dir: &Path,
) -> Result<ModuleRef, ConfigError> {
    let Some(config) = config else {
        return Err(ConfigError::at(
            entry_span,
            format!("\"modules.{name}.config\" is required"),
        ));
    };
    if config.value.as_os_str().is_empty() {
        return Err(ConfigError::at(
            config.referenced,
            format!("modules.{name}.config must not be empty"),
        ));
    }
    Ok(ModuleRef {
        config: resolve_against(&config.value, config_dir),
    })
}

fn validate_flow(entry: &Spanned<RawFlow>) -> Result<Flow, ConfigError> {
    let entry_location = entry.referenced;
    let flow = &entry.value;

    let Some(id) = &flow.id else {
        return Err(ConfigError::at(
            entry_location,
            "a flows entry has no \"id\"; every entry starts with \"- id: <name>\"",
        ));
    };
    if id.value.trim().is_empty() {
        return Err(ConfigError::at(id.referenced, "flow id must not be empty"));
    }
    let id_span = id.referenced;
    let id = &id.value;

    let enabled = flow
        .enabled
        .as_ref()
        .map(|span| span.value)
        .unwrap_or(false);

    let Some(transcription_model) = &flow.transcription_model else {
        return Err(ConfigError::at(
            id_span,
            format!("flow \"{id}\" has no \"transcription_model\""),
        ));
    };
    if transcription_model.value.trim().is_empty() {
        return Err(ConfigError::at(
            transcription_model.referenced,
            format!("flow \"{id}\" transcription_model must not be empty"),
        ));
    }

    // `formatter` must be written explicitly: an id or YAML null. A missing
    // key is an error — never an implicit choice (D4).
    let formatter_span = flow.formatter.location();
    let formatter = match &flow.formatter {
        FormatterField::Missing => {
            return Err(ConfigError::at(
                id_span,
                format!(
                    "flow \"{id}\" has no \"formatter\"; set a formatter id or \"formatter: null\" explicitly"
                ),
            ));
        }
        FormatterField::Null(_) => None,
        FormatterField::Id(value, location) if value.trim().is_empty() => {
            return Err(ConfigError::at(
                *location,
                format!("flow \"{id}\" formatter id must not be empty (or null)"),
            ));
        }
        FormatterField::Id(value, _) => Some(value.clone()),
    };

    let Some(relay_route) = &flow.relay_route else {
        return Err(ConfigError::at(
            id_span,
            format!("flow \"{id}\" has no \"relay_route\""),
        ));
    };
    if relay_route.value.trim().is_empty() {
        return Err(ConfigError::at(
            relay_route.referenced,
            format!("flow \"{id}\" relay_route must not be empty"),
        ));
    }
    if relay_route.value.eq_ignore_ascii_case("inspect") {
        return Err(ConfigError::at(
            relay_route.referenced,
            "the \"inspect\" route cannot be used in a flow",
        ));
    }
    if relay_route.value.eq_ignore_ascii_case("passthrough") && formatter.is_some() {
        // The passthrough route never calls a provider, so the formatter's
        // rules would never execute: the combination is a config mistake (D4).
        return Err(ConfigError::at(
            relay_route.referenced,
            format!(
                "flow \"{id}\" combines a formatter with the \"passthrough\" route; \
                 use \"formatter: null\" for a raw-only flow"
            ),
        ));
    }

    let budgets = validate_budgets(flow.budgets_ms.as_ref())?;

    Ok(Flow {
        id: id.clone(),
        enabled,
        transcription_model: transcription_model.value.clone(),
        formatter,
        relay_route: relay_route.value.clone(),
        budgets,
        references: ReferenceSpans {
            id: position(id_span),
            transcription_model: position(transcription_model.referenced),
            formatter: formatter_span.and_then(position),
            relay_route: position(relay_route.referenced),
        },
    })
}

fn validate_budgets(raw: Option<&Spanned<RawBudgets>>) -> Result<Budgets, ConfigError> {
    let defaults = Budgets::default();
    let Some(raw) = raw else { return Ok(defaults) };
    let budget = |span: &Option<Spanned<u64>>, name: &str, default: Duration| match span {
        None => Ok(default),
        Some(span) => {
            if span.value == 0 {
                return Err(ConfigError::at(
                    span.referenced,
                    format!("budgets_ms.{name} must be greater than zero"),
                ));
            }
            if span.value > MAX_BUDGET_MS {
                return Err(ConfigError::at(
                    span.referenced,
                    format!("budgets_ms.{name} must be at most {MAX_BUDGET_MS}"),
                ));
            }
            Ok(Duration::from_millis(span.value))
        }
    };
    let budgets = Budgets {
        total: budget(&raw.value.total, "total", defaults.total)?,
        transcription: budget(
            &raw.value.transcription,
            "transcription",
            defaults.transcription,
        )?,
        formatter: budget(&raw.value.formatter, "formatter", defaults.formatter)?,
        relay: budget(&raw.value.relay, "relay", defaults.relay)?,
    };
    if budgets.total <= TEARDOWN_RESERVE {
        return Err(ConfigError::at(
            raw.referenced,
            format!(
                "budgets_ms.total must exceed the {} ms teardown and output reserve",
                TEARDOWN_RESERVE.as_millis()
            ),
        ));
    }
    Ok(budgets)
}

/// Maps a saphyr parse error to a [`ConfigError`] with file:line:column,
/// mirroring the other modules.
fn parse_error(path: &Path, error: &serde_saphyr::Error) -> ConfigError {
    use serde_saphyr::Error as SaphyrError;

    let message = match error {
        SaphyrError::MultipleDocuments { .. } => {
            "multiple YAML documents are not allowed; use a single document".to_owned()
        }
        SaphyrError::DuplicateMappingKey { key: Some(key), .. } => format!("duplicate key `{key}`"),
        SaphyrError::DuplicateMappingKey { .. } => "duplicate key".to_owned(),
        SaphyrError::SerdeUnknownField { field, .. } => format!("unknown setting `{field}`"),
        SaphyrError::InvalidScalar { ty, .. } => format!("invalid {ty}"),
        SaphyrError::SerdeInvalidType {
            unexpected,
            expected,
            ..
        } => format!("expected {expected}, found {unexpected}"),
        SaphyrError::SerdeInvalidValue {
            unexpected,
            expected,
            ..
        } => format!("invalid value: expected {expected}, found {unexpected}"),
        SaphyrError::Eof { .. } => "unexpected end of input".to_owned(),
        SaphyrError::Unexpected { expected, .. } => format!("expected {expected}"),
        SaphyrError::UnsupportedTag { tag, .. } => format!("unsupported tag `{tag}`"),
        SaphyrError::UnknownAnchor { .. } => "alias references an unknown anchor".to_owned(),
        _ => strip_location_suffix(&error.to_string()),
    };
    let position = error
        .location()
        .and_then(|location| (location.line() != 0).then(|| (location.line(), location.column())));
    ConfigError::at_file(
        path,
        position.map(|(line, _)| line),
        position.map(|(_, column)| column),
        message,
    )
}

/// The crate appends ` at line N, column M` to its messages; the position is
/// already extracted separately, so strip it to avoid reporting it twice.
fn strip_location_suffix(text: &str) -> String {
    match text.rfind(" at line ") {
        Some(index) => text[..index].to_owned(),
        None => text.to_owned(),
    }
}

/// The per-user orchestrator file, when the platform's conventions place
/// one: `<base>/pumice/orchestrator.yml` under XDG_CONFIG_HOME, ~/.config or
/// %APPDATA%, mirroring the relay and formatter lookup (D6). A base must be
/// a nonempty absolute path: an empty or relative value would turn the
/// lookup into one relative to the working directory, so it makes the
/// per-user lookup unavailable (`--config` is then required).
pub fn per_user_config_path(get_env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    per_user_config_path_for(get_env, cfg!(windows))
}

/// `windows` is a parameter (rather than `cfg!` inside the body) so both
/// platform rules are exercised on every CI platform.
fn per_user_config_path_for(
    get_env: &dyn Fn(&str) -> Option<OsString>,
    windows: bool,
) -> Option<PathBuf> {
    let base = if windows {
        get_env("APPDATA").filter(|value| is_absolute_for(value, true))
    } else {
        // The XDG spec: a relative XDG_CONFIG_HOME is ignored and the
        // lookup falls back to ~/.config.
        get_env("XDG_CONFIG_HOME")
            .filter(|value| is_absolute_for(value, false))
            .or_else(|| {
                get_env("HOME")
                    .filter(|value| is_absolute_for(value, false))
                    .map(|home| PathBuf::from(home).join(".config").into_os_string())
            })
    }?;
    Some(PathBuf::from(base).join("pumice").join(PER_USER_FILE))
}

/// Whether `value` is a nonempty absolute path under the given platform's
/// rules, checked textually so both rules hold on every host: `/…` on Unix;
/// a drive root (`C:\` or `C:/`) or a UNC path (`\\server\share`) on
/// Windows.
fn is_absolute_for(value: &OsStr, windows: bool) -> bool {
    let text = value.to_string_lossy();
    let bytes = text.as_bytes();
    if windows {
        let drive_root = bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/');
        let unc = bytes.len() > 2 && bytes.starts_with(br"\\") && bytes[2] != b'\\';
        drive_root || unc
    } else {
        bytes.first() == Some(&b'/')
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    schema_version: Option<Spanned<u32>>,
    modules: Option<Spanned<RawModules>>,
    #[serde(default)]
    flows: Vec<Spanned<RawFlow>>,
    serve: Option<Spanned<RawServe>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawServe {
    enabled: Option<Spanned<bool>>,
    port: Option<Spanned<u16>>,
    transcription_model: Option<Spanned<String>>,
    startup_timeout_ms: Option<Spanned<u64>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModules {
    transcription: Option<Spanned<RawModuleRef>>,
    formatter: Option<Spanned<RawModuleRef>>,
    relay: Option<Spanned<RawRelayModule>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModuleRef {
    config: Option<Spanned<PathBuf>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRelayModule {
    location: Option<Spanned<String>>,
    config: Option<Spanned<PathBuf>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFlow {
    id: Option<Spanned<String>>,
    enabled: Option<Spanned<bool>>,
    transcription_model: Option<Spanned<String>>,
    /// Present-but-null (`formatter: ~`) is `Null`; a missing key is
    /// `Missing` and is rejected: the choice is never implicit (D4).
    /// `Option<Spanned<…>>` cannot express this: serde-saphyr maps an
    /// explicit YAML null to `None` before the field type runs.
    #[serde(default)]
    formatter: FormatterField,
    relay_route: Option<Spanned<String>>,
    budgets_ms: Option<Spanned<RawBudgets>>,
}

/// The three states of a flow's `formatter` key.
#[derive(Debug, Default)]
enum FormatterField {
    #[default]
    Missing,
    Null(serde_saphyr::Location),
    Id(String, serde_saphyr::Location),
}

impl FormatterField {
    /// The source position of a present value (id or null).
    fn location(&self) -> Option<serde_saphyr::Location> {
        match self {
            FormatterField::Missing => None,
            FormatterField::Null(location) | FormatterField::Id(_, location) => Some(*location),
        }
    }
}

impl<'de> Deserialize<'de> for FormatterField {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let spanned = Spanned::<FormatterValue>::deserialize(deserializer)?;
        Ok(match spanned.value {
            FormatterValue::Null => FormatterField::Null(spanned.referenced),
            FormatterValue::Id(id) => FormatterField::Id(id, spanned.referenced),
        })
    }
}

/// The value of a present `formatter` key: null or a string id.
enum FormatterValue {
    Null,
    Id(String),
}

impl<'de> Deserialize<'de> for FormatterValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ValueVisitor;
        impl serde::de::Visitor<'_> for ValueVisitor {
            type Value = FormatterValue;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a formatter id or null")
            }

            fn visit_unit<E: serde::de::Error>(self) -> Result<FormatterValue, E> {
                Ok(FormatterValue::Null)
            }

            fn visit_none<E: serde::de::Error>(self) -> Result<FormatterValue, E> {
                Ok(FormatterValue::Null)
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<FormatterValue, E> {
                Ok(FormatterValue::Id(value.to_owned()))
            }

            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<FormatterValue, E> {
                Ok(FormatterValue::Id(value))
            }
        }
        deserializer.deserialize_any(ValueVisitor)
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBudgets {
    total: Option<Spanned<u64>>,
    transcription: Option<Spanned<u64>>,
    formatter: Option<Spanned<u64>>,
    relay: Option<Spanned<u64>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<OsString> {
        None
    }

    /// Writes `text` as `orchestrator.yml` in a fresh directory and loads it.
    fn load_text(tmp: &tempfile::TempDir, text: &str) -> Result<LoadedConfig, ConfigError> {
        let path = tmp.path().join("orchestrator.yml");
        std::fs::write(&path, text).unwrap();
        load(Some(&path))
    }

    const VALID: &str = "\
schema_version: 1
modules:
  transcription:
    config: ./transcribe.yml
  formatter:
    config: ./formatter.yml
  relay:
    location: in_process
    config: ./relay.yml
flows:
  - id: clean
    enabled: true
    transcription_model: turbo
    formatter: builtin:clean
    relay_route: claude
  - id: raw
    enabled: false
    transcription_model: turbo
    formatter: null
    relay_route: passthrough
";

    #[test]
    fn valid_config_loads_with_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let loaded = load_text(&tmp, VALID).unwrap();
        assert_eq!(loaded.config.flows.len(), 2);

        let clean = loaded.flow("clean").unwrap();
        assert!(clean.enabled);
        assert_eq!(clean.transcription_model, "turbo");
        assert_eq!(clean.formatter.as_deref(), Some("builtin:clean"));
        assert_eq!(clean.relay_route, "claude");
        assert_eq!(clean.budgets, Budgets::default());

        let raw = loaded.flow("raw").unwrap();
        assert!(!raw.enabled, "enabled defaults to false");
        assert_eq!(raw.formatter, None, "explicit null selects no formatter");
        assert!(raw.relay_route.eq_ignore_ascii_case("passthrough"));

        // Module paths resolve against the orchestrator file's directory.
        let dir = tmp.path();
        assert_eq!(
            loaded.config.modules.transcription.config,
            dir.join("transcribe.yml")
        );
        assert_eq!(
            loaded.config.modules.formatter.as_ref().unwrap().config,
            dir.join("formatter.yml")
        );
        assert_eq!(loaded.config.modules.relay.config, dir.join("relay.yml"));
        assert!(loaded.path.is_absolute());
        assert!(loaded.flow("missing").is_none());
    }

    #[test]
    fn explicit_budgets_replace_defaults_per_key() {
        let tmp = tempfile::tempdir().unwrap();
        let text = VALID.replace(
            "relay_route: claude\n",
            "relay_route: claude\n    budgets_ms:\n      transcription: 5000\n",
        );
        let loaded = load_text(&tmp, &text).unwrap();
        let budgets = loaded.flow("clean").unwrap().budgets;
        assert_eq!(budgets.transcription, Duration::from_millis(5000));
        assert_eq!(budgets.total, Duration::from_millis(180_000));
        assert_eq!(budgets.formatter, Duration::from_millis(1_000));
        assert_eq!(budgets.relay, Duration::from_millis(60_000));
    }

    #[test]
    fn schema_version_is_required_and_must_be_1() {
        let tmp = tempfile::tempdir().unwrap();
        let err = load_text(&tmp, "modules: {}\n").unwrap_err().to_string();
        assert!(err.contains("schema_version is required"), "{err}");

        let err = load_text(&tmp, "schema_version: 2\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("schema_version must be 1"), "{err}");
        assert!(err.contains(":1:"), "line reported: {err}");
    }

    #[test]
    fn unknown_duplicate_and_multi_document_keys_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let err = load_text(&tmp, "schema_version: 1\nbogus: 5\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown setting `bogus`"), "{err}");
        assert!(err.contains(":2:"), "line reported: {err}");

        let err = load_text(&tmp, "schema_version: 1\nschema_version: 1\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("duplicate key"), "{err}");

        let err = load_text(&tmp, "schema_version: 1\n---\nschema_version: 1\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("multiple YAML documents"), "{err}");

        // Unknown keys inside nested maps are rejected too.
        let err = load_text(
            &tmp,
            "schema_version: 1\nmodules:\n  transcription:\n    config: t.yml\n    bogus: 1\n",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("unknown setting `bogus`"), "{err}");
    }

    #[test]
    fn modules_and_their_config_files_are_required() {
        let tmp = tempfile::tempdir().unwrap();
        let err = load_text(&tmp, "schema_version: 1\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("\"modules\" is required"), "{err}");

        let err = load_text(&tmp, "schema_version: 1\nmodules:\n  relay:\n    location: in_process\n    config: relay.yml\n")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("\"modules.transcription\" is required"),
            "{err}"
        );
        assert!(err.contains(":3:"), "points at the modules map: {err}");

        let err = load_text(
            &tmp,
            "schema_version: 1\nmodules:\n  transcription:\n    config: t.yml\n",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("\"modules.relay\" is required"), "{err}");

        let err = load_text(
            &tmp,
            "schema_version: 1\nmodules:\n  transcription:\n    location: t.yml\n",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("unknown setting `location`"), "{err}");

        let err = load_text(&tmp, "schema_version: 1\nmodules:\n  transcription: {}\n  relay:\n    location: in_process\n    config: relay.yml\n")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("\"modules.transcription.config\" is required"),
            "{err}"
        );
        assert!(err.contains(":3:"), "points at the module entry: {err}");
    }

    #[test]
    fn relay_location_must_be_in_process() {
        let tmp = tempfile::tempdir().unwrap();
        let base = "schema_version: 1\nmodules:\n  transcription:\n    config: t.yml\n  relay:\n";

        let err = load_text(&tmp, &format!("{base}    config: relay.yml\n"))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("\"modules.relay.location\" is required"),
            "{err}"
        );

        let err = load_text(
            &tmp,
            &format!("{base}    location: \"http://127.0.0.1:7567\"\n    config: relay.yml\n"),
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("modules.relay.location must be \"in_process\""),
            "{err}"
        );
        assert!(err.contains(":6:"), "the location value's line: {err}");
        // Never quotes the configured value.
        assert!(!err.contains("7567"), "{err}");
    }

    #[test]
    fn flow_id_is_required_nonempty_and_unique() {
        let tmp = tempfile::tempdir().unwrap();
        let modules = "schema_version: 1\nmodules:\n  transcription:\n    config: t.yml\n  relay:\n    location: in_process\n    config: relay.yml\nflows:\n";

        let err = load_text(
            &tmp,
            &format!("{modules}  - transcription_model: turbo\n    formatter: null\n    relay_route: claude\n"),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("a flows entry has no \"id\""), "{err}");

        let err = load_text(
            &tmp,
            &format!("{modules}  - id: \" \"\n    transcription_model: turbo\n    formatter: null\n    relay_route: claude\n"),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("flow id must not be empty"), "{err}");

        let dup = format!(
            "{modules}  - id: a\n    transcription_model: t\n    formatter: null\n    relay_route: passthrough\n  - id: a\n    transcription_model: t\n    formatter: null\n    relay_route: passthrough\n"
        );
        let err = load_text(&tmp, &dup).unwrap_err().to_string();
        assert!(err.contains("duplicate flow id \"a\""), "{err}");
        let second_id_line = dup.lines().count() as u64 - 3;
        assert!(
            err.contains(&format!(":{second_id_line}:")),
            "the second entry's line: {err}"
        );
    }

    #[test]
    fn flow_references_and_formatter_key_are_explicit() {
        let tmp = tempfile::tempdir().unwrap();
        let modules = "schema_version: 1\nmodules:\n  transcription:\n    config: t.yml\n  relay:\n    location: in_process\n    config: relay.yml\nflows:\n";

        let err = load_text(
            &tmp,
            &format!("{modules}  - id: a\n    formatter: null\n    relay_route: passthrough\n"),
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("flow \"a\" has no \"transcription_model\""),
            "{err}"
        );

        // `formatter` missing entirely is an error; explicit null is not.
        let err = load_text(
            &tmp,
            &format!(
                "{modules}  - id: a\n    transcription_model: t\n    relay_route: passthrough\n"
            ),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("flow \"a\" has no \"formatter\""), "{err}");
        assert!(err.contains("formatter: null"), "{err}");

        let err = load_text(
            &tmp,
            &format!("{modules}  - id: a\n    transcription_model: t\n    formatter: null\n"),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("flow \"a\" has no \"relay_route\""), "{err}");

        let loaded = load_text(
            &tmp,
            &format!("{modules}  - id: a\n    transcription_model: t\n    formatter: ~\n    relay_route: passthrough\n"),
        )
        .unwrap();
        assert_eq!(loaded.flow("a").unwrap().formatter, None, "`~` is null too");
    }

    #[test]
    fn inspect_and_formatter_passthrough_routes_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let with_formatter_module = "schema_version: 1\nmodules:\n  transcription:\n    config: t.yml\n  formatter:\n    config: f.yml\n  relay:\n    location: in_process\n    config: relay.yml\nflows:\n";

        let err = load_text(
            &tmp,
            &format!("{with_formatter_module}  - id: a\n    transcription_model: t\n    formatter: null\n    relay_route: inspect\n"),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("\"inspect\" route cannot be used"), "{err}");

        let err = load_text(
            &tmp,
            &format!("{with_formatter_module}  - id: a\n    transcription_model: t\n    formatter: builtin:clean\n    relay_route: passthrough\n"),
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("combines a formatter with the \"passthrough\" route"),
            "{err}"
        );

        // formatter: null + passthrough is the raw-only flow and loads fine.
        load_text(
            &tmp,
            &format!("{with_formatter_module}  - id: a\n    transcription_model: t\n    formatter: null\n    relay_route: passthrough\n"),
        )
        .unwrap();
    }

    #[test]
    fn formatter_module_is_required_only_for_enabled_flows_with_a_formatter() {
        let tmp = tempfile::tempdir().unwrap();
        let no_formatter_module = "schema_version: 1\nmodules:\n  transcription:\n    config: t.yml\n  relay:\n    location: in_process\n    config: relay.yml\nflows:\n";

        // Enabled flow with a formatter but no formatter module: rejected at
        // the flow's formatter line.
        let text = format!(
            "{no_formatter_module}  - id: a\n    enabled: true\n    transcription_model: t\n    formatter: builtin:clean\n    relay_route: claude\n"
        );
        let err = load_text(&tmp, &text).unwrap_err().to_string();
        assert!(
            err.contains("selects a formatter but \"modules.formatter\" is not configured"),
            "{err}"
        );
        assert!(err.contains(":12:"), "the formatter line: {err}");

        // The same flow disabled is fine, and so is an enabled null flow.
        let text = format!(
            "{no_formatter_module}  - id: a\n    enabled: false\n    transcription_model: t\n    formatter: builtin:clean\n    relay_route: claude\n  - id: b\n    enabled: true\n    transcription_model: t\n    formatter: null\n    relay_route: claude\n"
        );
        load_text(&tmp, &text).unwrap();
    }

    #[test]
    fn budgets_are_positive_bounded_and_cover_the_teardown_reserve() {
        let tmp = tempfile::tempdir().unwrap();
        let flow = |budgets: &str| {
            format!(
                "schema_version: 1\nmodules:\n  transcription:\n    config: t.yml\n  relay:\n    location: in_process\n    config: relay.yml\nflows:\n  - id: a\n    transcription_model: t\n    formatter: null\n    relay_route: passthrough\n    budgets_ms:\n{budgets}"
            )
        };

        let err = load_text(&tmp, &flow("      total: 0\n"))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("budgets_ms.total must be greater than zero"),
            "{err}"
        );

        let err = load_text(&tmp, &flow("      relay: 3600001\n"))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("budgets_ms.relay must be at most 3600000"),
            "{err}"
        );

        let err = load_text(&tmp, &flow("      total: 2500\n"))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("budgets_ms.total must exceed the 2500 ms"),
            "{err}"
        );

        // Stage caps may sum beyond the total: the remaining total wins.
        let loaded = load_text(
            &tmp,
            &flow("      total: 3000\n      transcription: 2900\n      relay: 2900\n"),
        )
        .unwrap();
        let budgets = loaded.flow("a").unwrap().budgets;
        assert_eq!(budgets.total, Duration::from_millis(3000));
        assert_eq!(budgets.transcription, Duration::from_millis(2900));
    }

    #[test]
    fn missing_explicit_file_is_an_error() {
        let path = PathBuf::from("/tmp/does-not-exist-pumice-orchestrator.yml");
        let err = load(Some(&path)).unwrap_err().to_string();
        assert!(err.contains("does not exist"), "{err}");
        assert!(err.contains(path.display().to_string().as_str()), "{err}");
    }

    #[test]
    fn per_user_lookup_and_its_absence_are_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let xdg = tmp.path().join("xdg");
        let xdg_value = xdg.clone().into_os_string();
        // The lookup's environment variable is platform-specific: APPDATA on
        // Windows, XDG_CONFIG_HOME on Linux and macOS.
        let base_key = if cfg!(windows) {
            "APPDATA"
        } else {
            "XDG_CONFIG_HOME"
        };

        // Without a per-user file the lookup names the missing file.
        let err = load_with_env(None, |key: &str| {
            (key == base_key).then(|| xdg_value.clone())
        })
        .unwrap_err()
        .to_string();
        assert!(
            err.contains(
                &xdg.join("pumice")
                    .join("orchestrator.yml")
                    .display()
                    .to_string()
            ),
            "names the per-user path: {err}"
        );

        // With the file present it loads.
        let dir = xdg.join("pumice");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("orchestrator.yml"), VALID).unwrap();
        let loaded = load_with_env(None, |key: &str| {
            (key == base_key).then(|| xdg_value.clone())
        })
        .unwrap();
        assert_eq!(loaded.config.flows.len(), 2);
        assert_eq!(loaded.path, dir.join("orchestrator.yml"));

        // No environment at all: a clear error, not a search.
        let err = load_with_env(None, no_env).unwrap_err().to_string();
        assert!(err.contains("no per-user configuration directory"), "{err}");
    }

    #[test]
    fn per_user_path_platform_rules() {
        // Unix rules are checked on the text, so the base must be a Unix
        // path on every host (a Windows temp dir is not absolute here).
        let xdg = PathBuf::from("/tmp/xdg-orch");
        let xdg_value = xdg.clone().into_os_string();
        let path = per_user_config_path_for(
            &|key| (key == "XDG_CONFIG_HOME").then(|| xdg_value.clone()),
            false,
        );
        assert_eq!(path, Some(xdg.join("pumice").join("orchestrator.yml")));

        let path = per_user_config_path_for(
            &|key| (key == "HOME").then(|| OsString::from("/tmp/home")),
            false,
        );
        assert_eq!(
            path,
            Some(
                PathBuf::from("/tmp/home")
                    .join(".config")
                    .join("pumice")
                    .join("orchestrator.yml")
            )
        );

        // A relative XDG_CONFIG_HOME is ignored (the XDG spec requires an
        // absolute path).
        let path = per_user_config_path_for(
            &|key| match key {
                "XDG_CONFIG_HOME" => Some(OsString::from("relative/dir")),
                "HOME" => Some(OsString::from("/tmp/home")),
                _ => None,
            },
            false,
        );
        assert_eq!(
            path,
            Some(
                PathBuf::from("/tmp/home")
                    .join(".config")
                    .join("pumice")
                    .join("orchestrator.yml")
            )
        );

        let path = per_user_config_path_for(
            &|key| (key == "APPDATA").then(|| OsString::from(r"C:\Users\me\AppData\Roaming")),
            true,
        );
        assert_eq!(
            path,
            Some(
                PathBuf::from(r"C:\Users\me\AppData\Roaming")
                    .join("pumice")
                    .join("orchestrator.yml")
            )
        );

        assert!(per_user_config_path(&no_env).is_none());
    }

    #[test]
    fn empty_or_relative_bases_disable_the_per_user_lookup() {
        let lookup = |pairs: &'static [(&'static str, &'static str)], windows: bool| {
            per_user_config_path_for(
                &|key| {
                    pairs
                        .iter()
                        .find(|(name, _)| *name == key)
                        .map(|(_, value)| OsString::from(value))
                },
                windows,
            )
        };

        // Unix: an empty or relative HOME never yields a path relative to
        // the working directory.
        assert_eq!(lookup(&[("HOME", "")], false), None);
        assert_eq!(lookup(&[("HOME", "home/me")], false), None);
        assert_eq!(lookup(&[("HOME", ".")], false), None);
        // An empty or relative XDG_CONFIG_HOME is ignored (XDG spec) and the
        // lookup falls back to HOME — only when HOME is absolute.
        assert_eq!(
            lookup(&[("XDG_CONFIG_HOME", ""), ("HOME", "")], false),
            None
        );
        assert_eq!(
            lookup(&[("XDG_CONFIG_HOME", "cfg"), ("HOME", "rel")], false),
            None
        );
        assert_eq!(
            lookup(&[("XDG_CONFIG_HOME", ""), ("HOME", "/home/me")], false),
            Some(
                PathBuf::from("/home/me")
                    .join(".config")
                    .join("pumice")
                    .join("orchestrator.yml")
            )
        );

        // Windows: APPDATA must be a drive root or UNC path.
        for appdata in ["", "AppData", r"C:", r"C:AppData", r"\AppData", "/x"] {
            let path = per_user_config_path_for(
                &|key| (key == "APPDATA").then(|| OsString::from(appdata)),
                true,
            );
            assert_eq!(path, None, "APPDATA={appdata:?}");
        }
        for appdata in [r"C:\Users\me", "D:/roaming", r"\\server\share"] {
            let path = per_user_config_path_for(
                &|key| (key == "APPDATA").then(|| OsString::from(appdata)),
                true,
            );
            assert!(path.is_some(), "APPDATA={appdata:?}");
        }

        // With no usable base, loading asks for --config.
        let err = load_with_env(None, |key: &str| match key {
            "HOME" => Some(OsString::from("")),
            "XDG_CONFIG_HOME" | "APPDATA" => Some(OsString::from("relative")),
            _ => None,
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("no per-user configuration directory"), "{err}");
        assert!(err.contains("--config"), "{err}");
    }

    /// [`VALID`] with a `serve:` section appended.
    fn with_serve(serve: &str) -> String {
        format!("{VALID}serve:\n{serve}")
    }

    #[test]
    fn files_without_serve_stay_valid_and_serve_settings_need_the_section() {
        let tmp = tempfile::tempdir().unwrap();
        let loaded = load_text(&tmp, VALID).unwrap();
        assert_eq!(loaded.config.serve, None);
        let err = serve_settings(&loaded).unwrap_err().to_string();
        assert!(err.contains("\"serve\" is not configured"), "{err}");
        assert!(err.contains("orchestrator.yml"), "names the file: {err}");
    }

    #[test]
    fn serve_section_defaults_and_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let loaded = load_text(
            &tmp,
            &with_serve("  enabled: true\n  transcription_model: turbo\n"),
        )
        .unwrap();
        let serve = loaded.config.serve.as_ref().unwrap();
        assert!(serve.enabled);
        assert_eq!(serve.port, DEFAULT_SERVE_PORT);
        assert_eq!(serve.startup_timeout, Duration::from_millis(60_000));
        let settings = serve_settings(&loaded).unwrap();
        assert_eq!(
            settings,
            ServeSettings {
                port: 17570,
                transcription_model: "turbo".to_owned(),
                startup_timeout: Duration::from_millis(60_000),
            }
        );

        let loaded = load_text(
            &tmp,
            &with_serve(
                "  enabled: true\n  port: 0\n  transcription_model: turbo\n  startup_timeout_ms: 1000\n",
            ),
        )
        .unwrap();
        let settings = serve_settings(&loaded).unwrap();
        assert_eq!(settings.port, 0, "port 0 is allowed explicitly");
        assert_eq!(settings.startup_timeout, Duration::from_millis(1000));
    }

    #[test]
    fn serve_disabled_or_unset_is_a_located_error_for_serve_only() {
        let tmp = tempfile::tempdir().unwrap();
        // A disabled section loads (pumice run keeps working) but cannot serve.
        let text = with_serve("  enabled: false\n");
        let loaded = load_text(&tmp, &text).unwrap();
        let err = serve_settings(&loaded).unwrap_err().to_string();
        assert!(err.contains("serve is not enabled"), "{err}");
        let line = text.lines().count();
        assert!(
            err.contains(&format!(":{line}:")),
            "the enabled line: {err}"
        );

        // `enabled` missing: false, located at the section.
        let loaded = load_text(&tmp, &with_serve("  port: 18000\n")).unwrap();
        let err = serve_settings(&loaded).unwrap_err().to_string();
        assert!(err.contains("serve is not enabled"), "{err}");
    }

    #[test]
    fn serve_port_7567_and_bad_values_are_rejected_with_their_line() {
        let tmp = tempfile::tempdir().unwrap();
        let text = with_serve("  enabled: true\n  port: 7567\n  transcription_model: turbo\n");
        let err = load_text(&tmp, &text).unwrap_err().to_string();
        assert!(err.contains("port 7567 is reserved"), "{err}");
        let line = text.lines().count() - 1;
        assert!(err.contains(&format!(":{line}:")), "the port line: {err}");
        // Even a disabled section may not name it.
        let err = load_text(&tmp, &with_serve("  enabled: false\n  port: 7567\n"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("port 7567 is reserved"), "{err}");

        let err = load_text(
            &tmp,
            &with_serve("  enabled: true\n  port: 70000\n  transcription_model: t\n"),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("orchestrator.yml:"), "{err}");

        for bad in ["999", "300001"] {
            let err = load_text(
                &tmp,
                &with_serve(&format!(
                    "  enabled: true\n  transcription_model: turbo\n  startup_timeout_ms: {bad}\n"
                )),
            )
            .unwrap_err()
            .to_string();
            assert!(
                err.contains("startup_timeout_ms must be between 1000 and 300000"),
                "{err}"
            );
        }

        let err = load_text(&tmp, &with_serve("  enabled: true\n  bogus: 1\n"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown setting `bogus`"), "{err}");
        // No `endpoints:` map in P13 (p13.1-flow-seam/D2).
        let err = load_text(&tmp, &with_serve("  enabled: true\n  endpoints: {}\n"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown setting `endpoints`"), "{err}");
    }

    #[test]
    fn serve_model_is_required_when_enabled_and_must_match_enabled_flows() {
        let tmp = tempfile::tempdir().unwrap();
        let err = load_text(&tmp, &with_serve("  enabled: true\n"))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("\"serve.transcription_model\" is required"),
            "{err}"
        );
        let err = load_text(
            &tmp,
            &with_serve("  enabled: true\n  transcription_model: \" \"\n"),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("must not be empty"), "{err}");

        // The enabled flow `clean` uses turbo; serving another model fails
        // at that flow's transcription_model line. (`raw` is disabled.)
        let text = with_serve("  enabled: true\n  transcription_model: small\n");
        let loaded = load_text(&tmp, &text).unwrap();
        let err = serve_settings(&loaded).unwrap_err().to_string();
        assert!(
            err.contains("flow \"clean\" uses a different transcription model"),
            "{err}"
        );
        assert!(
            err.contains("orchestrator.yml:13:"),
            "the flow field: {err}"
        );

        // A disabled flow may use another model.
        let text = VALID.replace(
            "    enabled: false\n    transcription_model: turbo",
            "    enabled: false\n    transcription_model: small",
        );
        let loaded = load_text(
            &tmp,
            &format!("{text}serve:\n  enabled: true\n  transcription_model: turbo\n"),
        )
        .unwrap();
        serve_settings(&loaded).unwrap();
    }

    #[test]
    fn served_flow_ids_are_bounded() {
        let tmp = tempfile::tempdir().unwrap();
        let long = "x".repeat(MAX_SERVED_FLOW_ID_BYTES + 1);
        let text = format!(
            "{}serve:\n  enabled: true\n  transcription_model: turbo\n",
            VALID.replace("- id: clean", &format!("- id: {long}"))
        );
        let loaded = load_text(&tmp, &text).unwrap();
        let err = serve_settings(&loaded).unwrap_err().to_string();
        assert!(err.contains("ids of at most 128 bytes"), "{err}");
        assert!(err.contains("orchestrator.yml:11:"), "the id line: {err}");
        assert!(!err.contains(&long), "the id is not echoed: {err}");
    }

    #[test]
    fn the_cli_port_rule_matches_the_config_rule() {
        assert!(check_serve_port(7567).is_err());
        assert_eq!(check_serve_port(0), Ok(0));
        assert_eq!(check_serve_port(17570), Ok(17570));
    }
}
