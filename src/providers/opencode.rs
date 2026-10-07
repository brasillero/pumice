//! OpenCode adapter (`opencode run`).
//!
//! Verified against the installed 1.18.34 in S0.2 and by inspecting its
//! bundled source: stdin input works, `--pure`, `--agent`, `--format json`,
//! `--title`, `--model provider/model` and `--variant` are advertised, and
//! `--format json` prints one JSON event per line —
//! `{type, timestamp, sessionID, part|error}` — for completed text parts,
//! step starts/finishes, completed tool calls (`tool_use`) and structured
//! errors. Parts carry `{id, messageID, sessionID}`; completed text parts
//! also carry `time.end`; a `step-finish` part carries the provider's
//! finish `reason` (`stop`, `length`, `tool-calls`, `content-filter`,
//! `other`). A failed run prints an `error` event (`error: {name, data?}`)
//! and exits nonzero. There is no explicit success terminal event: the run
//! ends (exit 0) after the final `step_finish`.
//!
//! Unverified (documented restriction recipe): the scalar `permission:
//! "deny"` covers all actions through the permission mechanism, but complete
//! removal of tool definitions is not proven, the disable variables' combined
//! enforcement in the installed binary was not runtime-tested, and OpenCode
//! retains CLI-owned transcripts. The adapter therefore ships disabled by
//! default and requires an explicit `provider/model` (with no configured
//! provider, OpenCode silently answers through a hosted default — S0.2).

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::cli::{CliAdapter, CliProvider};
use super::{
    FormatInput, ProbeSpec, Provider, ProviderDescriptor, ProviderError, ProviderErrorCode,
    ProviderLocations, ProviderSettings, RawOption,
};
use crate::config::ConfigError;
use crate::process::{
    Argument, CliInvocation, ProcessOutput, ProcessRunner, ProgramSpec,
};

pub const ID: &str = "opencode";
pub const DEFAULT_BINARY: &str = "opencode";
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

// Residual risk (recorded here; not printed, owner decision 2026-10-07):
// Configure an explicit provider/model. Upstream terms vary; startup
// customizations and CLI-owned transcript retention remain unverified.

/// OpenCode accepts no environment overrides: routing belongs in the
/// explicit `model` (and upstream `opencode` auth/config), and the adapter
/// owns the child environment below.
const ALLOWED_ENV: &[&str] = &[];

/// Adapter-owned child environment (1.18.34 tagged source; combined
/// enforcement unverified by runtime testing).
const DISABLE_ENV_VARS: [&str; 4] = [
    "OPENCODE_DISABLE_PROJECT_CONFIG",
    "OPENCODE_DISABLE_AUTOUPDATE",
    "OPENCODE_DISABLE_MODELS_FETCH",
    "OPENCODE_DISABLE_AUTOCOMPACT",
];

/// Inherited variables removed from the child: they would redirect
/// configuration, relocate it, or override permissions.
const REMOVE_ENV_VARS: [&str; 3] =
    ["OPENCODE_CONFIG", "OPENCODE_CONFIG_DIR", "OPENCODE_PERMISSION"];

const VARIANT_KEY: &str = "variant";

/// Agent name the inline configuration defines and argv selects. A
/// pre-existing `pumice` agent in merged configuration could contribute
/// settings; the deny-all recipe below is applied at both the global and the
/// agent level to override it (collision review: phase2-architecture §3.1).
const AGENT_NAME: &str = "pumice";

pub const DESCRIPTOR: ProviderDescriptor = ProviderDescriptor {
    id: ID,
    defaults,
    allowed_env: ALLOWED_ENV,
    validate_options,
    build,
    validate_settings,
    probe: ProbeSpec::Version(&["--version"]),
    default_binary: DEFAULT_BINARY,
    npm_entrypoint: None,
    install_hint: "npm install -g opencode-ai",
};

fn defaults() -> ProviderSettings {
    ProviderSettings {
        enabled: false,
        binary: None,
        // No default model: an explicit provider/model is required to
        // enable the adapter, so a hosted default is never selected.
        model: String::new(),
        timeout: DEFAULT_TIMEOUT,
        env: BTreeMap::new(),
        options: BTreeMap::new(),
    }
}

/// Only `variant` is allowed: a nonempty token of letters, digits, dots,
/// underscores and dashes, not starting with `-` (it lands in argv after
/// `--variant`, so it must not be able to change the command's meaning).
fn validate_options(options: &[RawOption<'_>]) -> Result<(), ConfigError> {
    for option in options {
        if option.key != VARIANT_KEY {
            return Err(ConfigError::at(
                option.key_at,
                format!("providers.{ID}.options.{} is not supported", option.key),
            ));
        }
        if !is_valid_variant(option.value) {
            return Err(ConfigError::at(
                option.value_at,
                format!(
                    "providers.{ID}.options.{VARIANT_KEY} must be a nonempty token of letters, digits, dots, underscores and dashes, not starting with '-'"
                ),
            ));
        }
    }
    Ok(())
}

/// When enabled, an explicit well-formed `provider/model` is required.
/// A missing model points at `enabled:` (or the file, with no entry);
/// a malformed model points at its own value.
fn validate_settings(
    settings: &ProviderSettings,
    locations: &ProviderLocations,
) -> Result<(), ConfigError> {
    if !settings.enabled {
        return Ok(());
    }
    if settings.model.is_empty() {
        return Err(ConfigError::at(
            locations.enabled.unwrap_or(serde_saphyr::Location::UNKNOWN),
            format!("providers.{ID}.model is required when the provider is enabled"),
        ));
    }
    if !is_valid_model(&settings.model) {
        return Err(ConfigError::at(
            locations.model.unwrap_or(serde_saphyr::Location::UNKNOWN),
            format!(
                "providers.{ID}.model must be a nonempty provider/model pair without whitespace, control characters or a leading '-'"
            ),
        ));
    }
    Ok(())
}

/// A nonempty provider prefix and model name around the first `/` (the name
/// keeps any further slashes), no whitespace or control characters, and no
/// leading `-` (which would read as an option).
fn is_valid_model(model: &str) -> bool {
    if model.starts_with('-') {
        return false;
    }
    if model.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let Some((provider, name)) = model.split_once('/') else {
        return false;
    };
    !provider.is_empty() && !name.is_empty()
}

fn is_valid_variant(variant: &str) -> bool {
    !variant.starts_with('-')
        && !variant.is_empty()
        && variant
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn build(
    settings: &ProviderSettings,
    runner: Arc<ProcessRunner>,
) -> Result<Arc<dyn Provider>, ConfigError> {
    // Validate the constructor again to protect direct library use (the
    // loader already ran full-settings validation).
    validate_settings(settings, &ProviderLocations::default())?;
    let binary = settings
        .binary
        .clone()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_BINARY));
    let variant = settings.options.get(VARIANT_KEY).cloned();
    Ok(Arc::new(CliProvider::new(
        OpenCodeAdapter::new(binary, settings.model.clone(), variant),
        runner,
        settings.timeout,
    )))
}

/// Builds restricted `opencode run` calls.
#[derive(Clone, Debug)]
pub struct OpenCodeAdapter {
    binary: PathBuf,
    model: String,
    variant: Option<String>,
}

impl OpenCodeAdapter {
    /// `binary` is a command name looked up on PATH (normally `opencode`) or
    /// a path to the official CLI.
    pub fn new(binary: PathBuf, model: String, variant: Option<String>) -> OpenCodeAdapter {
        OpenCodeAdapter {
            binary,
            model,
            variant,
        }
    }
}

impl CliAdapter for OpenCodeAdapter {
    fn id(&self) -> &'static str {
        ID
    }

    fn invocation(&self, input: FormatInput<'_>) -> Result<CliInvocation, ProviderError> {
        // Guards direct library construction; the configuration loader runs
        // the same rules with file positions.
        if !is_valid_model(&self.model) {
            return Err(ProviderError::other(ProviderErrorCode::InvalidConfiguration));
        }
        if self.variant.as_deref().is_some_and(|v| !is_valid_variant(v)) {
            return Err(ProviderError::other(ProviderErrorCode::InvalidConfiguration));
        }

        let mut args: Vec<Argument> = [
            "run",
            "--pure",
            "--agent",
            AGENT_NAME,
            "--format",
            "json",
            "--title",
            "Pumice",
            "--model",
        ]
        .into_iter()
        .map(Argument::literal)
        .collect();
        args.push(Argument::literal(&self.model));
        if let Some(variant) = &self.variant {
            args.push(Argument::literal("--variant"));
            args.push(Argument::literal(variant));
        }

        let user = input.user_prompt;
        let stdin = [user.before_text, input.text, user.after_text].concat();

        let mut env: BTreeMap<OsString, OsString> = BTreeMap::new();
        env.insert(
            OsString::from("OPENCODE_CONFIG_CONTENT"),
            OsString::from(inline_config(input.system_prompt, &self.model)),
        );
        for name in DISABLE_ENV_VARS {
            env.insert(OsString::from(name), OsString::from("1"));
        }

        Ok(CliInvocation {
            program: ProgramSpec {
                binary: self.binary.clone(),
                // Windows npm shims are refused until a layout is checked.
                npm_entrypoint: None,
            },
            args,
            stdin: stdin.into_bytes(),
            env,
            remove_env: REMOVE_ENV_VARS.into_iter().map(OsString::from).collect(),
            // The system prompt travels inside the inline configuration, not
            // in a control file.
            control_files: Vec::new(),
            workspace_files: Vec::new(),
            parser: parse_output,
        })
    }
}

/// OpenCode's inline configuration, serialized into `OPENCODE_CONFIG_CONTENT`.
///
/// The deny-all `pumice` agent carries the composed system prompt and the
/// selected model is set here as well as in argv, so no hosted default can
/// be picked up (S0.2). The model and prompt are the dynamic string values;
/// each is serialized with `serde_json`, then every literal `{` is re-encoded
/// as `\u007b` so OpenCode's `{env:…}`/`{file:…}` substitution pass — which
/// runs on the raw configuration text before JSON parsing (1.18.34 tagged
/// `config/variable.ts`) — cannot interpret prompt text as a variable
/// reference. JSON decoding restores the original string.
fn inline_config(system_prompt: &str, model: &str) -> String {
    let mut config = String::from(
        r#"{"share":"disabled","snapshot":false,"autoupdate":false,"permission":"deny","model":"#,
    );
    config.push_str(&json_string_with_escaped_braces(model));
    config.push_str(
        r#","agent":{"pumice":{"description":"Formats dictation without taking actions.","mode":"primary","prompt":"#,
    );
    config.push_str(&json_string_with_escaped_braces(system_prompt));
    config.push_str(r#","permission":"deny"}}}"#);
    config
}

/// Serializes `value` as one JSON string with every literal `{` escaped as
/// `\u007b`; decoding the JSON string yields the original text.
fn json_string_with_escaped_braces(value: &str) -> String {
    serde_json::to_string(value)
        .expect("a string serializes to JSON")
        .replace('{', "\\u007b")
}

/// Parses OpenCode's `--format json` JSONL event stream (one event per
/// line).
///
/// Success requires exit code 0 and a final `step_finish` whose `reason` is
/// `stop`, with no tool event or tool part. The result is the text parts of
/// that final step's message, concatenated in order and de-duplicated by
/// part id (a repeated id is an updated part: its latest text wins); text
/// from other messages is never mixed in. Completed tool calls print as
/// `tool_use` events, and any `step_finish` with reason `tool-calls` means
/// the model tried to use tools; both are rejected. A final reason other
/// than `stop` (`length`, `content-filter`, `other`, …) means the text is
/// incomplete or withheld and is invalid output. An `error` event fails the
/// run unless a later step finished with `stop` (the CLI recovered).
/// Reasoning parts, token usage and progress never carry result text and
/// are ignored. Error events are CLI diagnostics (never dictation), so only
/// their structured fields are classified.
pub fn parse_output(output: &ProcessOutput) -> Result<String, ProviderError> {
    let stdout = super::cli::stdout_text(output)?;

    // Each text part with the message it belongs to.
    let mut texts: Vec<(Option<String>, String)> = Vec::new();
    let mut text_indexes: HashMap<String, usize> = HashMap::new();
    // The last step_finish: its message and its reason.
    let mut last_finish: Option<(Option<String>, Option<String>)> = None;
    let mut tool_finish = false;
    let mut session: Option<String> = None;
    let mut tool_activity = false;
    let mut failure: Option<ProviderError> = None;

    for line in stdout.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
        };
        let Some(event_type) = event.get("type").and_then(Value::as_str) else {
            return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
        };
        // Every event of one run carries the session id created for it; a
        // second distinct id means this is not one run's stream.
        if let Some(id) = event.get("sessionID").and_then(Value::as_str) {
            if let Some(seen) = &session {
                if seen != id {
                    return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
                }
            } else {
                session = Some(id.to_owned());
            }
        }
        match event_type {
            "step_start" => {}
            "step_finish" => {
                // Verified reason vocabulary: stop, length, tool-calls,
                // content-filter, other (providers may add more).
                let reason = event.pointer("/part/reason").and_then(Value::as_str);
                if reason == Some("tool-calls") {
                    tool_finish = true;
                }
                if reason == Some("stop") {
                    // The CLI recovered from any earlier error.
                    failure = None;
                }
                let message = event
                    .pointer("/part/messageID")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                last_finish = Some((message, reason.map(str::to_owned)));
            }
            "text" => {
                let Some(part) = event.get("part") else {
                    return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
                };
                if part.get("type").and_then(Value::as_str) != Some("text") {
                    return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
                }
                // The CLI only prints parts of the run's own session.
                if let (Some(part_session), Some(run_session)) = (
                    part.get("sessionID").and_then(Value::as_str),
                    session.as_deref(),
                ) && part_session != run_session
                {
                    return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
                }
                let Some(text) = part.get("text").and_then(Value::as_str) else {
                    return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
                };
                let message = part
                    .get("messageID")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                match part.get("id").and_then(Value::as_str) {
                    Some(id) => match text_indexes.get(id) {
                        // A repeated part id is an updated part: keep the
                        // latest text at its original position.
                        Some(&index) => texts[index] = (message, text.to_owned()),
                        None => {
                            text_indexes.insert(id.to_owned(), texts.len());
                            texts.push((message, text.to_owned()));
                        }
                    },
                    None => texts.push((message, text.to_owned())),
                }
            }
            // A completed tool call (deny-all permissions should prevent
            // any; the event is printed for completed or errored tools).
            "tool_use" => tool_activity = true,
            // CLI diagnostic; the first structured error since the last
            // successful step classifies.
            "error" if failure.is_none() => {
                failure = Some(classify_error(&event));
            }
            // Reasoning parts, token usage and progress carry no result
            // text; unknown future informational events are ignored.
            _ => {}
        }
    }

    if tool_activity {
        return Err(ProviderError::other(
            ProviderErrorCode::UnexpectedToolActivity,
        ));
    }
    if tool_finish {
        return Err(ProviderError::other(
            ProviderErrorCode::UnexpectedToolActivity,
        ));
    }
    if let Some(err) = failure {
        return Err(err);
    }
    if !output.status.success() {
        return Err(ProviderError::other(ProviderErrorCode::NonzeroExit));
    }
    let Some((final_message, Some(reason))) = last_finish else {
        return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
    };
    if reason != "stop" {
        return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
    }
    // Only the final message's text: another message's text is not the
    // answer. A part that names no message cannot be told apart and stays.
    let result: String = texts
        .iter()
        .filter(|(message, _)| {
            final_message.is_none() || message.is_none() || *message == final_message
        })
        .map(|(_, text)| text.as_str())
        .collect();
    if result.is_empty() {
        return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
    }
    Ok(result)
}

/// Maps a structured `error` event to a safe error.
///
/// The 1.18.34 bundled core defines `ProviderAuthError` (missing
/// authentication; `LoadAPIKeyError` is the other auth-missing name) and
/// `APIError` (`statusCode`, `isRetryable`). Only those structured fields
/// are matched — event text is never copied into the error, which is
/// logged, and error events are CLI diagnostics so dictated text cannot be
/// misread here.
fn classify_error(event: &Value) -> ProviderError {
    let error = event.get("error").unwrap_or(event);
    let name = error.get("name").and_then(Value::as_str);
    if matches!(name, Some("ProviderAuthError" | "LoadAPIKeyError")) {
        return ProviderError::NotLoggedIn;
    }
    let status = error
        .get("statusCode")
        .or_else(|| error.get("status"))
        .and_then(Value::as_u64);
    if matches!(status, Some(401) | Some(403)) {
        return ProviderError::other(ProviderErrorCode::AuthenticationRejected);
    }
    // Quota exhaustion often arrives as HTTP 429 too: the name decides
    // before the status does.
    let quota_named = [name, error.get("code").and_then(Value::as_str)]
        .into_iter()
        .flatten()
        .any(|part| part.to_lowercase().contains("quota"));
    if quota_named {
        return ProviderError::QuotaExceeded { retry_after: None };
    }
    if status == Some(429) {
        return ProviderError::RateLimited { retry_after: None };
    }
    ProviderError::other(ProviderErrorCode::NonzeroExit)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn option(key: &'static str, value: &'static str) -> RawOption<'static> {
        RawOption {
            key,
            value,
            key_at: serde_saphyr::Location::UNKNOWN,
            value_at: serde_saphyr::Location::UNKNOWN,
        }
    }

    #[test]
    fn accepts_only_variant() {
        assert!(validate_options(&[]).is_ok());
        assert!(validate_options(&[option("variant", "low")]).is_ok());
        assert!(validate_options(&[option("variant", "high-1.0_beta")]).is_ok());
        assert!(validate_options(&[option("model", "opencode/big-pickle")]).is_err());
        assert!(validate_options(&[option("permissions", "deny")]).is_err());
    }

    #[test]
    fn rejects_invalid_variants() {
        for value in ["", "low effort", "high/beta", "--format", "x\t"] {
            assert!(
                validate_options(&[option("variant", value)]).is_err(),
                "value: {value:?}"
            );
        }
    }

    #[test]
    fn validates_models() {
        for good in [
            "opencode/big-pickle",
            "provider/model/with/slashes",
            "anthropic/claude-haiku-4-5",
        ] {
            assert!(is_valid_model(good), "model: {good:?}");
        }
        for bad in [
            "",
            "no-slash",
            "/model",
            "provider/",
            "/",
            "-model/x",
            "bad model/x",
            "provider/bad\tmodel",
            "provider/\u{7f}",
        ] {
            assert!(!is_valid_model(bad), "model: {bad:?}");
        }
    }

    fn enabled_settings(model: &str) -> ProviderSettings {
        ProviderSettings {
            enabled: true,
            model: model.to_owned(),
            ..defaults()
        }
    }

    #[test]
    fn settings_validation_requires_an_explicit_model() {
        // Disabled is valid; an unlisted provider never reaches the loader.
        let disabled = ProviderSettings {
            enabled: false,
            ..defaults()
        };
        assert!(validate_settings(&disabled, &ProviderLocations::default()).is_ok());

        // Enabled without a model: the loader points this at the `enabled:`
        // value's position (exercised with exact line/column in
        // tests/opencode.rs).
        let err = validate_settings(&enabled_settings(""), &ProviderLocations::default())
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "providers.opencode.model is required when the provider is enabled"
        );

        // A malformed model points at the model value.
        let err =
            validate_settings(&enabled_settings("no-slash"), &ProviderLocations::default())
                .unwrap_err();
        assert!(err
            .to_string()
            .contains("providers.opencode.model must be a nonempty provider/model pair"));
    }

    #[test]
    fn substitution_guard_round_trips_hostile_literals() {
        let prompt = "Read {env:HOME} and {file:~/.ssh/id_rsa} stay literal. Unicode: ação 🎤 \\ path C:\\tmp";
        let config = inline_config(prompt, "opencode/big-pickle");

        // The raw text must not carry a substitutable reference…
        assert!(!config.contains("{env:"), "config: {config}");
        assert!(!config.contains("{file:"), "config: {config}");
        // …and it must stay valid JSON that decodes to the original strings.
        let decoded: Value = serde_json::from_str(&config).expect("inline config parses");
        assert_eq!(decoded["permission"], Value::from("deny"));
        assert_eq!(decoded["agent"]["pumice"]["permission"], Value::from("deny"));
        assert_eq!(decoded["agent"]["pumice"]["prompt"], Value::from(prompt));
        assert_eq!(decoded["model"], Value::from("opencode/big-pickle"));
        assert_eq!(
            decoded["agent"]["pumice"]["description"],
            Value::from("Formats dictation without taking actions.")
        );
        assert_eq!(decoded["agent"]["pumice"]["mode"], Value::from("primary"));
        assert_eq!(decoded["share"], Value::from("disabled"));
        assert_eq!(decoded["snapshot"], Value::from(false));
        assert_eq!(decoded["autoupdate"], Value::from(false));
    }

    #[test]
    fn substitution_guard_escapes_braces_only() {
        let prompt = "a{b}c";
        let config = inline_config(prompt, "opencode/big-pickle");
        assert!(config.contains("\\u007b"), "config: {config}");
        let decoded: Value = serde_json::from_str(&config).expect("inline config parses");
        assert_eq!(decoded["agent"]["pumice"]["prompt"], Value::from(prompt));
    }

    #[test]
    fn classifies_structured_errors() {
        let event = |error: Value| json_event(&error);
        assert_eq!(
            classify_error(&event(json!({"name": "ProviderAuthError", "data": {"providerID": "x"}}))),
            ProviderError::NotLoggedIn
        );
        assert_eq!(
            classify_error(&event(json!({"name": "LoadAPIKeyError"}))),
            ProviderError::NotLoggedIn
        );
        assert_eq!(
            classify_error(&event(json!({"name": "APIError", "statusCode": 401, "isRetryable": false}))),
            ProviderError::other(ProviderErrorCode::AuthenticationRejected)
        );
        assert_eq!(
            classify_error(&event(json!({"name": "APIError", "statusCode": 403, "isRetryable": false}))),
            ProviderError::other(ProviderErrorCode::AuthenticationRejected)
        );
        assert_eq!(
            classify_error(&event(json!({"name": "APIError", "statusCode": 429, "isRetryable": true}))),
            ProviderError::RateLimited { retry_after: None }
        );
        assert_eq!(
            classify_error(&event(json!({"name": "InsufficientQuotaError"}))),
            ProviderError::QuotaExceeded { retry_after: None }
        );
        assert_eq!(
            classify_error(&event(json!({"name": "APIError", "statusCode": 500, "isRetryable": true}))),
            ProviderError::other(ProviderErrorCode::NonzeroExit)
        );
        assert_eq!(
            classify_error(&event(json!({"message": "something broke"}))),
            ProviderError::other(ProviderErrorCode::NonzeroExit)
        );
    }

    /// Wraps an error payload in an `error` event like the CLI prints.
    fn json_event(error: &Value) -> Value {
        json!({"type": "error", "timestamp": 0, "sessionID": "ses_1", "error": error})
    }
}
