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
//!     budgets_ms:
//!       total: 180000
//!       transcription: 120000
//!       formatter: 1000
//!       relay: 60000
//! serve:
//!   enabled: true                # false keeps pumice serve off
//!   port: 17570                  # 7567 is refused, 0 picks a free port
//!   transcription_model: turbo   # null only while serve is disabled
//!   startup_timeout_ms: 60000    # 1000..=300000
//!   transcription_flows: [clean] # may be empty (p15/D5)
//!   allowed_origins: []          # browser origins besides 127.0.0.1, e.g.
//!                                # a tailnet name; may be empty
//! ```
//!
//! Every setting must be written: an omitted one is an error, never a
//! built-in default (p17-serve-model-names/D3), reported at the mapping it
//! belongs in (line 1 for a top-level one). Nothing is enabled implicitly:
//! `enabled` is written on every flow and on `serve`, and `formatter` and
//! `modules.formatter` are written even as an explicit null (D4).
//! Validation reports `file:line:column` like the other modules; relative
//! module paths resolve against this file's directory.
//!
//! The `serve:` section (p13.1-flow-seam/D3) is written even when `pumice
//! serve` is not used: `enabled: false` keeps it off. Its own shape is
//! checked at load (published ids are public
//! model ids: `[A-Za-z0-9][A-Za-z0-9._-]{0,127}`, each listed once); the
//! rules that tie it to the flows (every enabled flow uses the served model,
//! a published flow exists, is enabled and is either off or a complete AI
//! flow) are checked only by `pumice serve`, through [`serve_settings`].

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

const PER_USER_FILE: &str = "orchestrator.yml";
/// The commented reference configuration every missing-setting error points
/// at.
const EXAMPLE_FILE: &str = "orchestrator.example.yml";

/// The live relay's port: never accepted for `pumice serve` (D3).
pub const RESERVED_RELAY_PORT: u16 = 7567;
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
    /// The `serve:` section.
    pub serve: Serve,
}

/// The `serve:` section, with its shape already checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Serve {
    pub enabled: bool,
    pub port: u16,
    /// `None` is `transcription_model: null`, valid only while serve is
    /// disabled; checked at load.
    pub transcription_model: Option<String>,
    pub startup_timeout: Duration,
    /// The flows published as OpenAI transcription models, in file order;
    /// empty unless listed (p15-serve-transcriptions/D5).
    pub transcription_flows: Vec<PublishedFlow>,
    /// Origins accepted besides the loopback one; empty unless listed.
    pub allowed_origins: Vec<AllowedOrigin>,
    /// Source positions for errors `pumice serve` reports later.
    pub spans: ServeSpans,
}

/// One `serve.transcription_flows` entry: a flow id, already checked as a
/// public model id, and where it is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedFlow {
    pub id: String,
    pub at: Option<(u64, u64)>,
}

/// One `serve.allowed_origins` entry: a browser origin whose requests
/// `pumice serve` accepts besides `http://127.0.0.1:<port>`, for a reverse
/// proxy on the same machine such as `tailscale serve` (the service itself
/// still listens on loopback only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllowedOrigin {
    /// Exactly as a browser sends it in `Origin`: `scheme://host[:port]`,
    /// lowercase, without the scheme's default port.
    pub origin: String,
    pub host: String,
    /// The origin's port, the scheme's default when it has none.
    pub port: u16,
    /// Whether the origin names its port (a browser's `Host` then does too).
    pub explicit_port: bool,
}

impl AllowedOrigin {
    /// Parses one entry; the error says what is wrong without quoting it.
    pub fn parse(text: &str) -> Result<AllowedOrigin, &'static str> {
        let (scheme, rest) = text
            .split_once("://")
            .ok_or("must be an origin like https://host.example:17570")?;
        let default_port = match scheme {
            "http" => 80,
            "https" => 443,
            _ => return Err("must start with http:// or https:// (lowercase)"),
        };
        if rest.contains('*') {
            return Err("must not contain wildcards");
        }
        if rest.contains('@') {
            return Err("must not contain a user name or password");
        }
        if rest.contains(['/', '?', '#']) {
            return Err(
                "must be an origin only: no path, query or fragment, not even a trailing /",
            );
        }
        if rest.starts_with('[') {
            return Err("IPv6 addresses are not supported; use a host name or an IPv4 address");
        }
        let (host, port) = match rest.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (rest, None),
        };
        if host.bytes().any(|byte| byte.is_ascii_uppercase()) {
            return Err("must write the host in lowercase, as browsers send it");
        }
        let labels_ok = host
            .split('.')
            .all(|label| !label.is_empty() && !label.starts_with('-') && !label.ends_with('-'));
        if !labels_ok
            || !host.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
            })
        {
            return Err("must name a host made of letters, digits, '-' and '.'");
        }
        let port = match port {
            None => default_port,
            Some(port) => {
                let parsed = (!port.starts_with('0')
                    && port.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| port.parse::<u16>().ok())
                .flatten()
                .filter(|port| *port != 0)
                .ok_or("must have a port from 1 to 65535")?;
                if parsed == default_port {
                    return Err("must leave out the scheme's default port, as browsers do");
                }
                parsed
            }
        };
        Ok(AllowedOrigin {
            origin: text.to_owned(),
            host: host.to_owned(),
            port,
            explicit_port: port != default_port,
        })
    }

    /// Whether a request's `Host` names this origin's host and port: the
    /// host in any case, the port as written or, for an origin on its
    /// scheme's default port, left out.
    pub fn matches_host(&self, host: &[u8]) -> bool {
        let (name, port) = match host.iter().rposition(|byte| *byte == b':') {
            Some(colon) => (&host[..colon], Some(&host[colon + 1..])),
            None => (host, None),
        };
        let port_ok = match port {
            None => !self.explicit_port,
            Some(port) => port == self.port.to_string().as_bytes(),
        };
        port_ok && name.eq_ignore_ascii_case(self.host.as_bytes())
    }
}

/// The 1-based (line, column) of the `serve:` section and of its keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ServeSpans {
    pub section: Option<(u64, u64)>,
    pub enabled: Option<(u64, u64)>,
    pub transcription_model: Option<(u64, u64)>,
}

/// What `pumice serve` runs with, after [`serve_settings`] checked the
/// section against the flows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServeSettings {
    pub port: u16,
    pub transcription_model: String,
    pub startup_timeout: Duration,
    /// Where `serve.transcription_model` is written, for errors about it.
    pub transcription_model_at: Option<(u64, u64)>,
    /// The published flow ids (the transcription endpoint's models), in
    /// file order.
    pub transcription_flows: Vec<String>,
    /// Origins accepted besides the loopback one.
    pub allowed_origins: Vec<AllowedOrigin>,
}

/// The module references. `formatter` may be `null` only when no enabled
/// flow selects a formatter (validated at load).
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
/// (D2). Every one is written in the file (the values D5 chose are the
/// example's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budgets {
    pub total: Duration,
    pub transcription: Duration,
    pub formatter: Duration,
    pub relay: Duration,
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
    let schema_version = raw
        .schema_version
        .required(Within::Root, "\"schema_version\"")?;
    if schema_version.value != 1 {
        return Err(ConfigError::at(
            schema_version.referenced,
            format!("schema_version must be 1, got {}", schema_version.value),
        ));
    }

    let modules = validate_modules(&raw.modules, config_dir)?;

    let entries = raw.flows.required(Within::Root, "\"flows\"")?;
    let mut flows: Vec<Flow> = Vec::new();
    for entry in &entries.value {
        let flow = validate_flow(entry)?;
        if flows.iter().any(|existing| existing.id == flow.id) {
            let duplicate = entry.value.id.location().unwrap_or(entry.referenced);
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
        && let Some((entry, flow)) = entries
            .value
            .iter()
            .zip(&flows)
            .find(|(_, flow)| flow.enabled && flow.formatter.is_some())
    {
        let span = entry.value.formatter.location().unwrap_or(entry.referenced);
        return Err(ConfigError::at(
            span,
            format!(
                "flow \"{}\" selects a formatter but \"modules.formatter\" is null",
                flow.id
            ),
        ));
    }

    let serve = validate_serve(raw.serve.required(Within::Root, "\"serve\"")?)?;

    Ok(Config {
        modules,
        flows,
        serve,
    })
}

fn validate_serve(raw: &Spanned<RawServe>) -> Result<Serve, ConfigError> {
    let section = raw.referenced;
    let within = Within::Mapping(section);
    let serve = &raw.value;
    let enabled = serve.enabled.required(within, "\"serve.enabled\"")?;
    let port = serve.port.required(within, "\"serve.port\"")?;
    let port = check_serve_port(port.value)
        .map_err(|message| ConfigError::at(port.referenced, message))?;
    let transcription_model = match serve
        .transcription_model
        .nullable(within, "\"serve.transcription_model\"")?
    {
        None if enabled.value => {
            return Err(ConfigError::at(
                serve.transcription_model.location().unwrap_or(section),
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
    let startup_timeout = serve
        .startup_timeout_ms
        .required(within, "\"serve.startup_timeout_ms\"")?;
    if !(MIN_STARTUP_TIMEOUT_MS..=MAX_STARTUP_TIMEOUT_MS).contains(&startup_timeout.value) {
        return Err(ConfigError::at(
            startup_timeout.referenced,
            format!(
                "serve.startup_timeout_ms must be between {MIN_STARTUP_TIMEOUT_MS} and {MAX_STARTUP_TIMEOUT_MS}"
            ),
        ));
    }
    let startup_timeout = Duration::from_millis(startup_timeout.value);
    let published = serve
        .transcription_flows
        .required(within, "\"serve.transcription_flows\"")?;
    let mut transcription_flows: Vec<PublishedFlow> = Vec::new();
    for entry in &published.value {
        if !is_public_model_id(&entry.value) {
            return Err(ConfigError::at(
                entry.referenced,
                format!(
                    "serve.transcription_flows entries are public model ids: 1 to \
                     {MAX_SERVED_FLOW_ID_BYTES} characters from A-Z, a-z, 0-9, '.', '_' and '-', \
                     starting with a letter or digit"
                ),
            ));
        }
        if transcription_flows
            .iter()
            .any(|published| published.id == entry.value)
        {
            return Err(ConfigError::at(
                entry.referenced,
                format!(
                    "flow \"{}\" is listed twice in serve.transcription_flows",
                    entry.value
                ),
            ));
        }
        transcription_flows.push(PublishedFlow {
            id: entry.value.clone(),
            at: position(entry.referenced),
        });
    }
    let origins = serve
        .allowed_origins
        .required(within, "\"serve.allowed_origins\"")?;
    let mut allowed_origins: Vec<AllowedOrigin> = Vec::new();
    for entry in &origins.value {
        let origin = AllowedOrigin::parse(&entry.value).map_err(|problem| {
            ConfigError::at(
                entry.referenced,
                format!("each serve.allowed_origins entry {problem}"),
            )
        })?;
        if allowed_origins.contains(&origin) {
            return Err(ConfigError::at(
                entry.referenced,
                "this origin is listed twice in serve.allowed_origins",
            ));
        }
        allowed_origins.push(origin);
    }
    Ok(Serve {
        enabled: enabled.value,
        port,
        transcription_model,
        startup_timeout,
        transcription_flows,
        allowed_origins,
        spans: ServeSpans {
            section: position(section),
            enabled: position(enabled.referenced),
            transcription_model: serve.transcription_model.location().and_then(position),
        },
    })
}

/// A published flow id is a public model id: `[A-Za-z0-9][A-Za-z0-9._-]`
/// up to [`MAX_SERVED_FLOW_ID_BYTES`] characters (p15/D5), so it needs no
/// escaping in a URL path, a form field or a header.
pub fn is_public_model_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    (1..=MAX_SERVED_FLOW_ID_BYTES).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// The frontend port rule shared by `serve.port` and `pumice serve --port`:
/// any port but the live relay's (D3). Port 0 is accepted when given
/// explicitly; the chosen port is announced.
pub fn check_serve_port(port: u16) -> Result<u16, String> {
    if port == RESERVED_RELAY_PORT {
        return Err(format!(
            "port {RESERVED_RELAY_PORT} is reserved for the relay service; pumice serve \
             uses another port"
        ));
    }
    Ok(port)
}

/// The settings `pumice serve` runs with: the section must be enabled, and
/// every enabled flow must use the served transcription model
/// (p13.1-flow-seam/D4), with at most [`MAX_SERVED_FLOWS`] enabled flows
/// whose ids fit [`MAX_SERVED_FLOW_ID_BYTES`]. Each published flow must be
/// a configured, enabled flow that is either off (`formatter: null` with
/// `relay_route: passthrough`) or has a formatter for its AI route
/// (p15-serve-transcriptions/D5); being enabled, it uses the served model.
/// Errors carry the orchestrator file and, when known, the offending
/// value's position.
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
    let serve = &loaded.config.serve;
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
    for published in &serve.transcription_flows {
        let Some(flow) = loaded.flow(&published.id) else {
            return Err(located(
                published.at,
                format!(
                    "serve.transcription_flows names flow \"{}\", which is not in \"flows\"",
                    published.id
                ),
            ));
        };
        if !flow.enabled {
            return Err(located(
                published.at,
                format!(
                    "serve.transcription_flows names flow \"{}\", which is disabled; \
                     set \"enabled: true\" on it or remove it from the list",
                    flow.id
                ),
            ));
        }
        // `formatter` + passthrough is refused at load already; what is
        // left is an AI route without rules.
        if flow.formatter.is_none() && !is_passthrough(&flow.relay_route) {
            return Err(located(
                flow.references.formatter.or(flow.references.id),
                format!(
                    "flow \"{}\" is published but sends the transcript to an AI route \
                     without a formatter; give it a formatter, or make it an off flow with \
                     \"formatter: null\" and \"relay_route: passthrough\"",
                    flow.id
                ),
            ));
        }
    }
    Ok(ServeSettings {
        port: serve.port,
        transcription_model: model.clone(),
        startup_timeout: serve.startup_timeout,
        transcription_model_at: serve.spans.transcription_model,
        transcription_flows: serve
            .transcription_flows
            .iter()
            .map(|published| published.id.clone())
            .collect(),
        allowed_origins: serve.allowed_origins.clone(),
    })
}

/// The relay's built-in route that returns the transcript unchanged.
pub fn is_passthrough(route: &str) -> bool {
    route.eq_ignore_ascii_case("passthrough")
}

fn validate_modules(raw: &Setting<RawModules>, config_dir: &Path) -> Result<Modules, ConfigError> {
    let modules = raw.required(Within::Root, "\"modules\"")?;
    let modules_span = modules.referenced;
    let within = Within::Mapping(modules_span);
    let modules = &modules.value;

    let transcription = match &modules.transcription {
        Setting::Missing => {
            return Err(ConfigError::at(
                modules_span,
                "\"modules.transcription\" is required, with its \"config\" file",
            ));
        }
        entry => {
            let entry = entry.required(within, "\"modules.transcription\"")?;
            module_ref(
                entry.referenced,
                &entry.value.config,
                "transcription",
                config_dir,
            )?
        }
    };
    // `null` is "no formatter module", for configurations whose enabled
    // flows select none (checked by the caller).
    let formatter = match modules
        .formatter
        .nullable(within, "\"modules.formatter\"")?
    {
        Some(entry) => Some(module_ref(
            entry.referenced,
            &entry.value.config,
            "formatter",
            config_dir,
        )?),
        None => None,
    };
    let relay = match &modules.relay {
        Setting::Missing => {
            return Err(ConfigError::at(
                modules_span,
                "\"modules.relay\" is required, with \"location: in_process\" and its \"config\" file",
            ));
        }
        entry => {
            let entry = entry.required(within, "\"modules.relay\"")?;
            let entry_span = entry.referenced;
            let relay = &entry.value;
            let location = match &relay.location {
                Setting::Missing => {
                    return Err(ConfigError::at(
                        entry_span,
                        "\"modules.relay.location\" is required (only \"in_process\" is supported)",
                    ));
                }
                location => {
                    location.required(Within::Mapping(entry_span), "\"modules.relay.location\"")?
                }
            };
            if location.value != "in_process" {
                return Err(ConfigError::at(
                    location.referenced,
                    "modules.relay.location must be \"in_process\"; external relays are not supported yet",
                ));
            }
            module_ref(entry_span, &relay.config, "relay", config_dir)?
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
    config: &Setting<PathBuf>,
    name: &str,
    config_dir: &Path,
) -> Result<ModuleRef, ConfigError> {
    let config = match config {
        Setting::Missing => {
            return Err(ConfigError::at(
                entry_span,
                format!("\"modules.{name}.config\" is required"),
            ));
        }
        config => config.required(
            Within::Mapping(entry_span),
            &format!("\"modules.{name}.config\""),
        )?,
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
    let within = Within::Mapping(entry_location);
    let flow = &entry.value;

    let Setting::Value(id) = &flow.id else {
        return Err(ConfigError::at(
            flow.id.location().unwrap_or(entry_location),
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
        .required(within, &format!("flow \"{id}\".enabled"))?
        .value;

    let transcription_model = match &flow.transcription_model {
        Setting::Missing => {
            return Err(ConfigError::at(
                id_span,
                format!("flow \"{id}\" has no \"transcription_model\""),
            ));
        }
        model => model.required(within, &format!("flow \"{id}\".transcription_model"))?,
    };
    if transcription_model.value.trim().is_empty() {
        return Err(ConfigError::at(
            transcription_model.referenced,
            format!("flow \"{id}\" transcription_model must not be empty"),
        ));
    }

    // `formatter` must be written explicitly: an id or YAML null. A missing
    // key is an error — never an implicit choice (D4).
    let formatter = match &flow.formatter {
        Setting::Missing => {
            return Err(ConfigError::at(
                id_span,
                format!(
                    "flow \"{id}\" has no \"formatter\"; set a formatter id or \"formatter: null\" explicitly"
                ),
            ));
        }
        Setting::Null(_) => None,
        Setting::Value(value) if value.value.trim().is_empty() => {
            return Err(ConfigError::at(
                value.referenced,
                format!("flow \"{id}\" formatter id must not be empty (or null)"),
            ));
        }
        Setting::Value(value) => Some(value.value.clone()),
    };

    let relay_route = match &flow.relay_route {
        Setting::Missing => {
            return Err(ConfigError::at(
                id_span,
                format!("flow \"{id}\" has no \"relay_route\""),
            ));
        }
        route => route.required(within, &format!("flow \"{id}\".relay_route"))?,
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

    let budgets = flow
        .budgets_ms
        .required(within, &format!("flow \"{id}\".budgets_ms"))?;
    let budgets = validate_budgets(budgets, id)?;

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
            formatter: flow.formatter.location().and_then(position),
            relay_route: position(relay_route.referenced),
        },
    })
}

fn validate_budgets(raw: &Spanned<RawBudgets>, flow: &str) -> Result<Budgets, ConfigError> {
    let within = Within::Mapping(raw.referenced);
    let budget = |setting: &Setting<u64>, name: &str| {
        let span = setting.required(within, &format!("flow \"{flow}\".budgets_ms.{name}"))?;
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
    };
    let budgets = Budgets {
        total: budget(&raw.value.total, "total")?,
        transcription: budget(&raw.value.transcription, "transcription")?,
        formatter: budget(&raw.value.formatter, "formatter")?,
        relay: budget(&raw.value.relay, "relay")?,
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
    #[serde(default)]
    schema_version: Setting<u32>,
    #[serde(default)]
    modules: Setting<RawModules>,
    #[serde(default)]
    flows: Setting<Vec<Spanned<RawFlow>>>,
    #[serde(default)]
    serve: Setting<RawServe>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawServe {
    #[serde(default)]
    enabled: Setting<bool>,
    #[serde(default)]
    port: Setting<u16>,
    #[serde(default)]
    transcription_model: Setting<String>,
    #[serde(default)]
    startup_timeout_ms: Setting<u64>,
    #[serde(default)]
    transcription_flows: Setting<Vec<Spanned<String>>>,
    #[serde(default)]
    allowed_origins: Setting<Vec<Spanned<String>>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModules {
    #[serde(default)]
    transcription: Setting<RawModuleRef>,
    #[serde(default)]
    formatter: Setting<RawModuleRef>,
    #[serde(default)]
    relay: Setting<RawRelayModule>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModuleRef {
    #[serde(default)]
    config: Setting<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRelayModule {
    #[serde(default)]
    location: Setting<String>,
    #[serde(default)]
    config: Setting<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFlow {
    #[serde(default)]
    id: Setting<String>,
    #[serde(default)]
    enabled: Setting<bool>,
    #[serde(default)]
    transcription_model: Setting<String>,
    /// Present-but-null (`formatter: ~`) selects no formatter; a missing key
    /// is rejected: the choice is never implicit (D4).
    #[serde(default)]
    formatter: Setting<String>,
    #[serde(default)]
    relay_route: Setting<String>,
    #[serde(default)]
    budgets_ms: Setting<RawBudgets>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBudgets {
    #[serde(default)]
    total: Setting<u64>,
    #[serde(default)]
    transcription: Setting<u64>,
    #[serde(default)]
    formatter: Setting<u64>,
    #[serde(default)]
    relay: Setting<u64>,
}

/// One setting as written: omitted, an explicit `null` (also `~` or no
/// value), or a value. `Option<Spanned<T>>` cannot tell the first two apart:
/// serde-saphyr maps an explicit `null` to `None` before the field type runs.
/// Every field of this type needs `#[serde(default)]`.
#[derive(Debug, Default)]
enum Setting<T> {
    #[default]
    Missing,
    Null(serde_saphyr::Location),
    Value(Spanned<T>),
}

/// Where a missing setting is reported: the mapping it belongs in.
#[derive(Clone, Copy, Debug)]
enum Within {
    /// The top level of the file, reported at line 1.
    Root,
    /// A nested mapping or list entry, reported at its first key.
    Mapping(serde_saphyr::Location),
}

impl<T> Setting<T> {
    /// The written value; an omitted key or an explicit `null` is an error.
    /// `name` is the setting as error messages show it, e.g. `"serve.port"`.
    fn required(&self, within: Within, name: &str) -> Result<&Spanned<T>, ConfigError> {
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
    fn nullable(&self, within: Within, name: &str) -> Result<Option<&Spanned<T>>, ConfigError> {
        match self {
            Setting::Missing => Err(missing(within, name)),
            Setting::Null(_) => Ok(None),
            Setting::Value(value) => Ok(Some(value)),
        }
    }

    /// The position of a written value or `null`.
    fn location(&self) -> Option<serde_saphyr::Location> {
        match self {
            Setting::Missing => None,
            Setting::Null(location) => Some(*location),
            Setting::Value(value) => Some(value.referenced),
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
        D: serde::Deserializer<'de>,
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

    /// The budgets every test flow writes (the example's values).
    const BUDGETS: &str = "    budgets_ms:\n      total: 180000\n      transcription: 120000\n      formatter: 1000\n      relay: 60000\n";

    /// Everything but the `serve:` section: lines 1-30.
    const BASE: &str = "\
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
    budgets_ms:
      total: 180000
      transcription: 120000
      formatter: 1000
      relay: 60000
  - id: raw
    enabled: false
    transcription_model: turbo
    formatter: null
    relay_route: passthrough
    budgets_ms:
      total: 180000
      transcription: 120000
      formatter: 1000
      relay: 60000
";

    /// A disabled `serve:` section with every key written.
    const SERVE_OFF: &str = "\
serve:
  enabled: false
  port: 17570
  transcription_model: null
  startup_timeout_ms: 60000
  transcription_flows: []
  allowed_origins: []
";

    /// A complete file: [`BASE`] and [`SERVE_OFF`].
    fn valid() -> String {
        format!("{BASE}{SERVE_OFF}")
    }

    /// The modules and the `flows:` key, for tests that write their own
    /// flow entries (lines 1-10): `modules.formatter` is null.
    const MODULES: &str = "schema_version: 1\nmodules:\n  transcription:\n    config: t.yml\n  formatter: null\n  relay:\n    location: in_process\n    config: relay.yml\nflows:\n";

    /// [`MODULES`] with a formatter module (lines 1-10).
    const WITH_FORMATTER_MODULE: &str = "schema_version: 1\nmodules:\n  transcription:\n    config: t.yml\n  formatter:\n    config: f.yml\n  relay:\n    location: in_process\n    config: relay.yml\nflows:\n";

    #[test]
    fn valid_config_loads_with_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let loaded = load_text(&tmp, &valid()).unwrap();
        assert_eq!(loaded.config.flows.len(), 2);

        let clean = loaded.flow("clean").unwrap();
        assert!(clean.enabled);
        assert_eq!(clean.transcription_model, "turbo");
        assert_eq!(clean.formatter.as_deref(), Some("builtin:clean"));
        assert_eq!(clean.relay_route, "claude");
        // The values that were the built-in defaults (D5), now written.
        assert_eq!(
            clean.budgets,
            Budgets {
                total: Duration::from_millis(180_000),
                transcription: Duration::from_millis(120_000),
                formatter: Duration::from_millis(1_000),
                relay: Duration::from_millis(60_000),
            }
        );

        let raw = loaded.flow("raw").unwrap();
        assert!(!raw.enabled, "enabled: false is kept");
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
        let serve = &loaded.config.serve;
        assert!(!serve.enabled);
        assert_eq!(serve.port, 17570);
        assert_eq!(serve.transcription_model, None);
        assert_eq!(serve.startup_timeout, Duration::from_millis(60_000));
    }

    #[test]
    fn every_top_level_setting_is_required_at_line_1() {
        let tmp = tempfile::tempdir().unwrap();
        let text = valid();
        for (key, start, end) in [
            ("schema_version", 1, 1),
            ("modules", 2, 9),
            ("flows", 10, 30),
            ("serve", 31, 37),
        ] {
            let without: String = text
                .lines()
                .enumerate()
                .filter(|(index, _)| !(start..=end).contains(&(index + 1)))
                .map(|(_, line)| format!("{line}\n"))
                .collect();
            let err = load_text(&tmp, &without).unwrap_err().to_string();
            assert!(
                err.contains(&format!(
                    "orchestrator.yml:1:1: \"{key}\" is missing; every setting must be written (see orchestrator.example.yml)"
                )),
                "{key}: {err}"
            );
        }
    }

    #[test]
    fn missing_flow_settings_are_reported_at_the_flow_or_its_budgets() {
        let tmp = tempfile::tempdir().unwrap();
        // The clean flow starts at line 11, column 5; its budgets at 17:7.
        for (line, expected) in [
            (
                "    enabled: true\n",
                ":11:5: flow \"clean\".enabled is missing",
            ),
            (
                "    budgets_ms:\n      total: 180000\n      transcription: 120000\n      formatter: 1000\n      relay: 60000\n  - id: raw",
                ":11:5: flow \"clean\".budgets_ms is missing",
            ),
            (
                "      total: 180000\n",
                ":17:7: flow \"clean\".budgets_ms.total is missing",
            ),
            (
                "      relay: 60000\n",
                ":17:7: flow \"clean\".budgets_ms.relay is missing",
            ),
        ] {
            let replacement = if line.ends_with("- id: raw") {
                "  - id: raw"
            } else {
                ""
            };
            let text = valid().replacen(line, replacement, 1);
            let err = load_text(&tmp, &text).unwrap_err().to_string();
            assert!(err.contains(expected), "{expected}: {err}");
        }
        // `enabled: null` is not an omission, but it is not a value either.
        let text = valid().replacen("    enabled: true\n", "    enabled: null\n", 1);
        let err = load_text(&tmp, &text).unwrap_err().to_string();
        assert!(
            err.contains(":12:14: flow \"clean\".enabled must not be null"),
            "{err}"
        );
    }

    #[test]
    fn missing_serve_settings_are_reported_at_the_section() {
        let tmp = tempfile::tempdir().unwrap();
        // The section's first key is at line 32, column 3.
        for key in [
            "enabled",
            "port",
            "transcription_model",
            "startup_timeout_ms",
            "transcription_flows",
            "allowed_origins",
        ] {
            let text: String = valid()
                .lines()
                .filter(|line| !line.starts_with(&format!("  {key}:")))
                .map(|line| format!("{line}\n"))
                .collect();
            let err = load_text(&tmp, &text).unwrap_err().to_string();
            // Without `enabled` the section starts at the next key, still
            // line 32.
            assert!(
                err.contains(&format!(":32:3: \"serve.{key}\" is missing")),
                "{key}: {err}"
            );
        }
        for (from, to, at) in [
            (
                "serve:\n  enabled: false\n",
                "serve:\n  enabled: null\n",
                ":32:12:",
            ),
            ("  port: 17570\n", "  port: ~\n", ":33:9:"),
            (
                "  transcription_flows: []\n",
                "  transcription_flows: null\n",
                ":36:24:",
            ),
        ] {
            let err = load_text(&tmp, &valid().replace(from, to))
                .unwrap_err()
                .to_string();
            assert!(err.contains(&format!("{at} \"serve.")), "{to}: {err}");
            assert!(err.contains("must not be null"), "{to}: {err}");
        }
    }

    #[test]
    fn explicit_budgets_are_used_per_key() {
        let tmp = tempfile::tempdir().unwrap();
        let text = valid().replacen(
            "      transcription: 120000\n",
            "      transcription: 5000\n",
            1,
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
        assert!(err.contains(":1:1: \"schema_version\" is missing"), "{err}");

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
        assert!(err.contains(":1:1: \"modules\" is missing"), "{err}");

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
            "schema_version: 1\nmodules:\n  transcription:\n    config: t.yml\n  formatter: null\n",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("\"modules.relay\" is required"), "{err}");

        // `modules.formatter` is written even when no formatter is used.
        let err = load_text(
            &tmp,
            "schema_version: 1\nmodules:\n  transcription:\n    config: t.yml\n  relay:\n    location: in_process\n    config: relay.yml\n",
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains(":3:3: \"modules.formatter\" is missing"),
            "{err}"
        );

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
        let base = "schema_version: 1\nmodules:\n  transcription:\n    config: t.yml\n  formatter: null\n  relay:\n";

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
        assert!(err.contains(":7:"), "the location value's line: {err}");
        // Never quotes the configured value.
        assert!(!err.contains("7567"), "{err}");
    }

    #[test]
    fn flow_id_is_required_nonempty_and_unique() {
        let tmp = tempfile::tempdir().unwrap();
        let modules = MODULES;

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
            "{modules}  - id: a\n    enabled: false\n    transcription_model: t\n    formatter: null\n    relay_route: passthrough\n{BUDGETS}  - id: a\n    enabled: false\n    transcription_model: t\n    formatter: null\n    relay_route: passthrough\n{BUDGETS}"
        );
        let err = load_text(&tmp, &dup).unwrap_err().to_string();
        assert!(err.contains("duplicate flow id \"a\""), "{err}");
        let second_id_line = dup.lines().count() as u64 - 9;
        assert!(
            err.contains(&format!(":{second_id_line}:")),
            "the second entry's line: {err}"
        );
    }

    #[test]
    fn flow_references_and_formatter_key_are_explicit() {
        let tmp = tempfile::tempdir().unwrap();
        let modules = MODULES;

        let err = load_text(
            &tmp,
            &format!(
                "{modules}  - id: a\n    enabled: false\n    formatter: null\n    relay_route: passthrough\n"
            ),
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
                "{modules}  - id: a\n    enabled: false\n    transcription_model: t\n    relay_route: passthrough\n"
            ),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("flow \"a\" has no \"formatter\""), "{err}");
        assert!(err.contains("formatter: null"), "{err}");

        let err = load_text(
            &tmp,
            &format!(
                "{modules}  - id: a\n    enabled: false\n    transcription_model: t\n    formatter: null\n"
            ),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("flow \"a\" has no \"relay_route\""), "{err}");

        let loaded = load_text(
            &tmp,
            &format!("{modules}  - id: a\n    enabled: false\n    transcription_model: t\n    formatter: ~\n    relay_route: passthrough\n{BUDGETS}{SERVE_OFF}"),
        )
        .unwrap();
        assert_eq!(loaded.flow("a").unwrap().formatter, None, "`~` is null too");
    }

    #[test]
    fn inspect_and_formatter_passthrough_routes_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let with_formatter_module = WITH_FORMATTER_MODULE;

        let err = load_text(
            &tmp,
            &format!("{with_formatter_module}  - id: a\n    enabled: false\n    transcription_model: t\n    formatter: null\n    relay_route: inspect\n"),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("\"inspect\" route cannot be used"), "{err}");

        let err = load_text(
            &tmp,
            &format!("{with_formatter_module}  - id: a\n    enabled: false\n    transcription_model: t\n    formatter: builtin:clean\n    relay_route: passthrough\n"),
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
            &format!("{with_formatter_module}  - id: a\n    enabled: false\n    transcription_model: t\n    formatter: null\n    relay_route: passthrough\n{BUDGETS}{SERVE_OFF}"),
        )
        .unwrap();
    }

    #[test]
    fn formatter_module_is_required_only_for_enabled_flows_with_a_formatter() {
        let tmp = tempfile::tempdir().unwrap();
        let no_formatter_module = MODULES;

        // Enabled flow with a formatter but no formatter module: rejected at
        // the flow's formatter line.
        let text = format!(
            "{no_formatter_module}  - id: a\n    enabled: true\n    transcription_model: t\n    formatter: builtin:clean\n    relay_route: claude\n{BUDGETS}{SERVE_OFF}"
        );
        let err = load_text(&tmp, &text).unwrap_err().to_string();
        assert!(
            err.contains("selects a formatter but \"modules.formatter\" is null"),
            "{err}"
        );
        assert!(err.contains(":13:"), "the formatter line: {err}");

        // The same flow disabled is fine, and so is an enabled null flow.
        let text = format!(
            "{no_formatter_module}  - id: a\n    enabled: false\n    transcription_model: t\n    formatter: builtin:clean\n    relay_route: claude\n{BUDGETS}  - id: b\n    enabled: true\n    transcription_model: t\n    formatter: null\n    relay_route: claude\n{BUDGETS}{SERVE_OFF}"
        );
        load_text(&tmp, &text).unwrap();
    }

    #[test]
    fn budgets_are_positive_bounded_and_cover_the_teardown_reserve() {
        let tmp = tempfile::tempdir().unwrap();
        // `budgets` lists the keys under test; the others keep the
        // example's values.
        let flow = |budgets: &str| {
            let mut keys = budgets.to_owned();
            for (key, value) in [
                ("total", 180000),
                ("transcription", 120000),
                ("formatter", 1000),
                ("relay", 60000),
            ] {
                if !budgets.contains(&format!(" {key}:")) {
                    keys.push_str(&format!("      {key}: {value}\n"));
                }
            }
            format!(
                "{MODULES}  - id: a\n    enabled: false\n    transcription_model: t\n    formatter: null\n    relay_route: passthrough\n    budgets_ms:\n{keys}{SERVE_OFF}"
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
        std::fs::write(dir.join("orchestrator.yml"), valid()).unwrap();
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

    /// The `serve:` keys with [`SERVE_OFF`]'s values, keyed by name.
    const SERVE_KEYS: [(&str, &str); 6] = [
        ("enabled", "  enabled: false\n"),
        ("port", "  port: 17570\n"),
        ("transcription_model", "  transcription_model: null\n"),
        ("startup_timeout_ms", "  startup_timeout_ms: 60000\n"),
        ("transcription_flows", "  transcription_flows: []\n"),
        ("allowed_origins", "  allowed_origins: []\n"),
    ];

    /// `serve` (the section's keys under test), then every `serve:` key it
    /// does not write, with [`SERVE_OFF`]'s values. Appending keeps the line
    /// numbers in `serve` unchanged.
    fn serve_section(serve: &str) -> String {
        let mut section = format!("serve:\n{serve}");
        for (key, line) in SERVE_KEYS {
            if !serve
                .lines()
                .any(|written| written.starts_with(&format!("  {key}:")))
            {
                section.push_str(line);
            }
        }
        section
    }

    /// [`BASE`] with a `serve:` section from [`serve_section`]: the
    /// section starts at line 31.
    fn with_serve(serve: &str) -> String {
        format!("{BASE}{}", serve_section(serve))
    }

    #[test]
    fn serve_section_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let loaded = load_text(
            &tmp,
            &with_serve("  enabled: true\n  transcription_model: turbo\n"),
        )
        .unwrap();
        let serve = &loaded.config.serve;
        assert!(serve.enabled);
        assert_eq!(serve.port, 17570);
        assert_eq!(serve.startup_timeout, Duration::from_millis(60_000));
        let settings = serve_settings(&loaded).unwrap();
        assert_eq!(
            settings,
            ServeSettings {
                port: 17570,
                transcription_model: "turbo".to_owned(),
                startup_timeout: Duration::from_millis(60_000),
                transcription_model_at: Some((33, 24)),
                transcription_flows: Vec::new(),
                allowed_origins: Vec::new(),
            }
        );
        assert!(serve.transcription_flows.is_empty(), "nothing is published");

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
    fn serve_disabled_is_a_located_error_for_serve_only() {
        let tmp = tempfile::tempdir().unwrap();
        // A disabled section loads (pumice run keeps working) but cannot serve.
        let loaded = load_text(&tmp, &valid()).unwrap();
        let err = serve_settings(&loaded).unwrap_err().to_string();
        assert!(err.contains("serve is not enabled"), "{err}");
        assert!(err.contains(":32:"), "the enabled line: {err}");

        // A disabled section may still name the model it would serve.
        let loaded = load_text(
            &tmp,
            &with_serve("  enabled: false\n  transcription_model: turbo\n"),
        )
        .unwrap();
        assert_eq!(
            loaded.config.serve.transcription_model.as_deref(),
            Some("turbo")
        );
    }

    #[test]
    fn serve_port_7567_and_bad_values_are_rejected_with_their_line() {
        let tmp = tempfile::tempdir().unwrap();
        let text = with_serve("  enabled: true\n  port: 7567\n  transcription_model: turbo\n");
        let err = load_text(&tmp, &text).unwrap_err().to_string();
        assert!(err.contains("port 7567 is reserved"), "{err}");
        assert!(err.contains(":33:"), "the port line: {err}");
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
        assert!(err.contains(":34:24:"), "the null value: {err}");
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
        let text = BASE.replace(
            "    enabled: false\n    transcription_model: turbo",
            "    enabled: false\n    transcription_model: small",
        );
        let loaded = load_text(
            &tmp,
            &format!(
                "{text}{}",
                serve_section("  enabled: true\n  transcription_model: turbo\n")
            ),
        )
        .unwrap();
        serve_settings(&loaded).unwrap();
    }

    #[test]
    fn served_flow_ids_are_bounded() {
        let tmp = tempfile::tempdir().unwrap();
        let long = "x".repeat(MAX_SERVED_FLOW_ID_BYTES + 1);
        let text = format!(
            "{}{}",
            BASE.replace("- id: clean", &format!("- id: {long}")),
            serve_section("  enabled: true\n  transcription_model: turbo\n")
        );
        let loaded = load_text(&tmp, &text).unwrap();
        let err = serve_settings(&loaded).unwrap_err().to_string();
        assert!(err.contains("ids of at most 128 bytes"), "{err}");
        assert!(err.contains("orchestrator.yml:11:"), "the id line: {err}");
        assert!(!err.contains(&long), "the id is not echoed: {err}");
    }

    /// [`BASE`] with `raw` enabled, an AI flow without a formatter and a
    /// disabled flow, served with `turbo` and publishing `published`.
    fn publishing(published: &str) -> String {
        format!(
            "{}  - id: bare
    enabled: true
    transcription_model: turbo
    formatter: null
    relay_route: claude
{BUDGETS}  - id: later
    enabled: false
    transcription_model: turbo
    formatter: null
    relay_route: passthrough
{BUDGETS}{}",
            BASE.replace("    enabled: false\n", "    enabled: true\n"),
            serve_section(&format!(
                "  enabled: true\n  transcription_model: turbo\n  transcription_flows: {published}\n"
            ))
        )
    }

    /// The 1-based line of the first line of `text` containing `needle`.
    fn line_of(text: &str, needle: &str) -> usize {
        text.lines().position(|line| line.contains(needle)).unwrap() + 1
    }

    #[test]
    fn published_flows_are_listed_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let loaded = load_text(&tmp, &publishing("[raw, clean]")).unwrap();
        let serve = &loaded.config.serve;
        assert_eq!(
            serve
                .transcription_flows
                .iter()
                .map(|flow| flow.id.as_str())
                .collect::<Vec<_>>(),
            ["raw", "clean"]
        );
        assert!(serve.transcription_flows[0].at.is_some());
        let settings = serve_settings(&loaded).unwrap();
        assert_eq!(settings.transcription_flows, ["raw", "clean"]);

        // Block style, and an empty list publishes nothing.
        let text = publishing("\n    - clean\n    - raw");
        let settings = serve_settings(&load_text(&tmp, &text).unwrap()).unwrap();
        assert_eq!(settings.transcription_flows, ["clean", "raw"]);
        let settings = serve_settings(&load_text(&tmp, &publishing("[]")).unwrap()).unwrap();
        assert!(settings.transcription_flows.is_empty());
    }

    #[test]
    fn allowed_origins_are_exact_browser_origins() {
        let origin = AllowedOrigin::parse("https://server-1.tail59f332.ts.net:17570").unwrap();
        assert_eq!(
            origin,
            AllowedOrigin {
                origin: "https://server-1.tail59f332.ts.net:17570".to_owned(),
                host: "server-1.tail59f332.ts.net".to_owned(),
                port: 17570,
                explicit_port: true,
            }
        );
        assert!(origin.matches_host(b"server-1.tail59f332.ts.net:17570"));
        assert!(origin.matches_host(b"Server-1.Tail59f332.ts.net:17570"));
        for host in [
            &b"server-1.tail59f332.ts.net"[..],
            b"server-1.tail59f332.ts.net:443",
            b"server-1.tail59f332.ts.net:17571",
            b"server-1.tail59f332.ts.net.:17570",
            b"evil.example:17570",
            b"127.0.0.1:17570",
        ] {
            assert!(
                !origin.matches_host(host),
                "{}",
                String::from_utf8_lossy(host)
            );
        }
        // On the scheme's default port the origin has no port, and the
        // Host may name it or not.
        let default = AllowedOrigin::parse("https://pumice.example").unwrap();
        assert_eq!((default.port, default.explicit_port), (443, false));
        assert!(default.matches_host(b"pumice.example"));
        assert!(default.matches_host(b"pumice.example:443"));
        assert!(!default.matches_host(b"pumice.example:80"));
        let plain = AllowedOrigin::parse("http://10.0.0.5:8080").unwrap();
        assert!(plain.matches_host(b"10.0.0.5:8080"));

        for (bad, problem) in [
            ("server-1.ts.net:17570", "must be an origin like"),
            ("ftp://server-1.ts.net", "http:// or https://"),
            ("HTTPS://server-1.ts.net", "http:// or https://"),
            (
                "https://server-1.ts.net:17570/",
                "no path, query or fragment",
            ),
            ("https://server-1.ts.net/page", "no path, query or fragment"),
            ("https://server-1.ts.net?x=1", "no path, query or fragment"),
            ("https://server-1.ts.net#top", "no path, query or fragment"),
            ("https://*.ts.net", "wildcards"),
            ("https://user:pass@server-1.ts.net", "user name or password"),
            ("https://Server-1.ts.net", "lowercase"),
            ("https://[::1]:17570", "IPv6"),
            ("https://", "letters, digits"),
            ("https://bad_host.example", "letters, digits"),
            ("https://-bad.example", "letters, digits"),
            ("https://bad..example", "letters, digits"),
            ("https://server-1.ts.net:", "port from 1 to 65535"),
            ("https://server-1.ts.net:0", "port from 1 to 65535"),
            ("https://server-1.ts.net:017570", "port from 1 to 65535"),
            ("https://server-1.ts.net:65536", "port from 1 to 65535"),
            ("https://server-1.ts.net:+1", "port from 1 to 65535"),
            ("https://server-1.ts.net:443", "default port"),
            ("http://server-1.ts.net:80", "default port"),
        ] {
            let error = AllowedOrigin::parse(bad).unwrap_err();
            assert!(error.contains(problem), "{bad}: {error}");
        }
    }

    #[test]
    fn allowed_origins_default_to_none_and_errors_are_located() {
        let tmp = tempfile::tempdir().unwrap();
        let loaded = load_text(
            &tmp,
            &with_serve("  enabled: true\n  transcription_model: turbo\n"),
        )
        .unwrap();
        assert!(serve_settings(&loaded).unwrap().allowed_origins.is_empty());

        let text = with_serve(
            "  enabled: true\n  transcription_model: turbo\n  allowed_origins:\n    - https://server-1.ts.net:17570\n    - http://pumice.example\n",
        );
        let loaded = load_text(&tmp, &text).unwrap();
        let origins: Vec<String> = serve_settings(&loaded)
            .unwrap()
            .allowed_origins
            .into_iter()
            .map(|origin| origin.origin)
            .collect();
        assert_eq!(
            origins,
            ["https://server-1.ts.net:17570", "http://pumice.example"]
        );

        for (entries, problem) in [
            ("    - https://server-1.ts.net:17570/\n", "no path"),
            ("    - '*'\n", "must be an origin like"),
            (
                "    - https://server-1.ts.net:17570\n    - https://server-1.ts.net:17570\n",
                "listed twice",
            ),
        ] {
            let text = with_serve(&format!(
                "  enabled: true\n  transcription_model: turbo\n  allowed_origins:\n{entries}"
            ));
            let err = load_text(&tmp, &text).unwrap_err().to_string();
            assert!(err.contains(problem), "{err}");
            assert!(err.contains("serve.allowed_origins"), "{err}");
            // The last entry written, before the keys `with_serve` appends.
            let lines: Vec<&str> = text.lines().collect();
            let line = lines
                .iter()
                .rposition(|line| line.starts_with("    - "))
                .unwrap()
                + 1;
            assert!(err.contains(&format!("orchestrator.yml:{line}:")), "{err}");
        }
        let err = load_text(
            &tmp,
            &with_serve("  enabled: true\n  transcription_model: turbo\n  allowed_origins: https://a.example\n"),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("orchestrator.yml:"), "{err}");
    }

    #[test]
    fn published_ids_must_be_public_model_ids_and_unique() {
        let tmp = tempfile::tempdir().unwrap();
        let longest = format!("a{}", "-".repeat(MAX_SERVED_FLOW_ID_BYTES - 1));
        assert!(is_public_model_id(&longest));
        for good in ["a", "9", "clean-en", "v1.2_x", "A.b-C_d"] {
            assert!(is_public_model_id(good), "{good}");
        }
        let too_long = format!("{longest}x");
        for bad in [
            "",
            "-clean",
            ".clean",
            "_clean",
            "clean en",
            "clean/en",
            "clean:en",
            "Olá",
            "clean%20",
            too_long.as_str(),
        ] {
            assert!(!is_public_model_id(bad), "{bad}");
        }

        // At load, with the entry's line; the id is quoted in YAML so the
        // shape reaches the check.
        let text = publishing("\n    - clean\n    - \"clean en\"");
        let err = load_text(&tmp, &text).unwrap_err().to_string();
        assert!(err.contains("public model ids"), "{err}");
        let line = line_of(&text, "clean en");
        assert!(err.contains(&format!("orchestrator.yml:{line}:")), "{err}");

        let text = publishing("\n    - clean\n    - raw\n    - clean");
        let err = load_text(&tmp, &text).unwrap_err().to_string();
        assert!(
            err.contains("flow \"clean\" is listed twice in serve.transcription_flows"),
            "{err}"
        );
        let lines: Vec<&str> = text.lines().collect();
        let line = lines
            .iter()
            .rposition(|line| *line == "    - clean")
            .unwrap()
            + 1;
        assert!(err.contains(&format!("orchestrator.yml:{line}:")), "{err}");

        // Not a list of strings.
        let err = load_text(&tmp, &publishing("clean"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("orchestrator.yml:"), "{err}");
        // Even a disabled section checks its shape.
        let err = load_text(
            &tmp,
            &with_serve("  enabled: false\n  transcription_flows: [\"-x\"]\n"),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("public model ids"), "{err}");
    }

    #[test]
    fn published_flows_must_be_enabled_and_off_or_complete() {
        let tmp = tempfile::tempdir().unwrap();
        for (published, needle, message) in [
            (
                "\n    - clean\n    - nowhere",
                "- nowhere",
                "names flow \"nowhere\", which is not in \"flows\"",
            ),
            (
                "\n    - later",
                "- later",
                "names flow \"later\", which is disabled",
            ),
            (
                "\n    - bare",
                "relay_route: claude\n    budgets_ms:\n      total: 180000\n      transcription: 120000\n      formatter: 1000\n      relay: 60000\n  - id: later",
                "flow \"bare\" is published but sends the transcript to an AI route without a formatter",
            ),
        ] {
            let text = publishing(published);
            let err = serve_settings(&load_text(&tmp, &text).unwrap())
                .unwrap_err()
                .to_string();
            assert!(err.contains(message), "{err}");
            // The bare flow's error points at its `formatter: null` line.
            let line = if needle.starts_with("relay_route") {
                let at = text.find(needle).unwrap();
                text[..at].lines().count() - 1
            } else {
                line_of(&text, needle)
            };
            assert!(
                err.contains(&format!("orchestrator.yml:{line}:")),
                "{published}: {err}"
            );
        }

        // A published flow uses the served model like every enabled flow:
        // a different one fails at the flow's transcription_model line.
        let text = publishing("[raw]").replace(
            "serve:\n  enabled: true\n  transcription_model: turbo",
            "serve:\n  enabled: true\n  transcription_model: small",
        );
        let err = serve_settings(&load_text(&tmp, &text).unwrap())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("uses a different transcription model"),
            "{err}"
        );
        assert!(err.contains("orchestrator.yml:13:"), "{err}");
    }

    #[test]
    fn the_example_writes_every_setting_with_the_old_defaults() {
        let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("orchestrator.example.yml");
        let loaded = load(Some(&example)).unwrap();
        let ids: Vec<&str> = loaded
            .config
            .flows
            .iter()
            .map(|flow| flow.id.as_str())
            .collect();
        assert_eq!(ids, ["clean", "raw"]);
        for flow in &loaded.config.flows {
            assert!(flow.enabled, "{}", flow.id);
            assert_eq!(
                flow.budgets,
                Budgets {
                    total: Duration::from_millis(180_000),
                    transcription: Duration::from_millis(120_000),
                    formatter: Duration::from_millis(1_000),
                    relay: Duration::from_millis(60_000),
                },
                "{}",
                flow.id
            );
        }
        assert!(loaded.config.modules.formatter.is_some());
        // `pumice serve` stays off, as it was without a section.
        let serve = &loaded.config.serve;
        assert!(!serve.enabled);
        assert_eq!(serve.port, 17570);
        assert_eq!(serve.startup_timeout, Duration::from_millis(60_000));
        assert!(serve.allowed_origins.is_empty());
    }

    #[test]
    fn the_cli_port_rule_matches_the_config_rule() {
        assert!(check_serve_port(7567).is_err());
        assert_eq!(check_serve_port(0), Ok(0));
        assert_eq!(check_serve_port(17570), Ok(17570));
    }
}
