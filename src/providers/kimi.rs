//! Kimi Code CLI adapter (`kimi -p`).
//!
//! Verified against the installed 2.1.1 in S2.4 (real calls, see
//! docs/research/S2.4-kimi-adapter.md). Print mode is the only usable
//! transport: the prompt is one `-p` argv value (no stdin sentinel, no
//! prompt file — verified in source and by running `-p -`), and the reply
//! is a JSONL stream (`--output-format stream-json`):
//!
//! ```text
//! {"role":"meta","type":"system.version","version":"2.1.1"}
//! {"role":"assistant","content":"…","tool_calls":[…]?}
//! {"role":"tool","tool_call_id":"…","content":"…"}
//! {"role":"meta","type":"session.resume_hint",…}
//! ```
//!
//! Restrictions: a custom agent file (`--agent-file`) carries the system
//! prompt as its Markdown body with `tools: []` and `subagents: []` in the
//! front matter, which the upstream agents documentation defines as
//! disabling every tool (including MCP globs) and all delegation. Print
//! mode otherwise runs tools under an auto permission policy, so the empty
//! allowlist is the load-bearing restriction and any tool activity seen in
//! the stream is rejected. `--skills-dir .` (the empty workspace) replaces
//! the user's and project's skill directories, and
//! `KIMI_CODE_BUILTIN_PRODUCT_SKILLS=false` turns off built-in product
//! skills. `KIMI_CODE_BACKGROUND_PRINT_BACKGROUND_MODE=exit` makes the run
//! return right after the main turn (the built-in default, `steer`, waits
//! for background tasks), `KIMI_DISABLE_TELEMETRY=1` keeps the run off the
//! telemetry intake, and `KIMI_CODE_NO_AUTO_UPDATE=1` blocks update checks.
//!
//! Residual unverifiables (why this adapter ships disabled by default with
//! a risk warning): user-configured hooks and plugin MCP servers have no
//! per-launch off switch (agent tool removal does not stop hook scripts or
//! server startup; MCP servers are trust-gated and the fresh empty
//! workspace is untrusted, which is what keeps them from starting); print
//! mode persists a resumable session under the user's own `~/.kimi-code`;
//! the agent body is rendered as a `${var}` template (unknown variables
//! stay verbatim, so only the documented variable names in configured
//! instructions would expand); and the argv transport caps the user
//! message at [`MAX_USER_MESSAGE_BYTES`].

use std::collections::BTreeMap;
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
use crate::process::{Argument, CliInvocation, ControlFile, ProcessOutput, ProcessRunner, ProgramSpec};

pub const ID: &str = "kimi";
pub const DEFAULT_BINARY: &str = "kimi";
/// Cheapest model alias that works in the owner's account (verified with
/// `kimi provider list` and a real call, S2.4).
pub const DEFAULT_MODEL: &str = "kimi-k2.7-code-highspeed";
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Fixed notice shown by `check-config` and startup while enabled; cites
/// the S0.3 subscription-terms rating and the residual unverifiables.
pub const RISK_WARNING: &str = "Disabled by default: Moonshot subscription terms rate non-interactive automation as risky (docs/research/S0.3-provider-terms.md). Dictation travels in argv (size-capped); user-configured hooks, plugins and CLI-owned session retention have no per-launch off switch.";

/// Adapter-owned child environment (2.1.1 bundled source; the background
/// mode, the skills switches and the telemetry call were also verified in
/// real runs).
const DISABLE_ENV_VARS: [(&str, &str); 4] = [
    // Return after the main turn instead of the built-in `steer` default,
    // which waits for background task completions.
    ("KIMI_CODE_BACKGROUND_PRINT_BACKGROUND_MODE", "exit"),
    // --skills-dir replaces user/project skill directories; this turns off
    // the built-in product skills on top.
    ("KIMI_CODE_BUILTIN_PRODUCT_SKILLS", "false"),
    // No telemetry from runs Pumice starts (AGENTS.md: no telemetry).
    ("KIMI_DISABLE_TELEMETRY", "1"),
    // No update checks from runs Pumice starts.
    ("KIMI_CODE_NO_AUTO_UPDATE", "1"),
];

/// Inherited variables removed from the child: it would mark the fresh
/// workspace trusted, relaxing the MCP trust gate that otherwise keeps
/// configured MCP servers from starting in it.
const REMOVE_ENV_VARS: [&str; 1] = ["KIMI_CODE_TRUST_WORKSPACE"];

/// Maximum accepted byte length of the composed user message. The prompt
/// travels as one argv element, whose limit is 128 KiB on Linux and whose
/// whole command line is capped at 32 KiB on Windows; long dictations fail
/// with a clean `InputTooLarge` instead of a platform spawn error.
pub const MAX_USER_MESSAGE_BYTES: usize = 24 * 1024;

/// Agent file name inside the per-call control directory.
const AGENT_FILE_NAME: &str = "pumice-agent.md";

pub const DESCRIPTOR: ProviderDescriptor = ProviderDescriptor {
    id: ID,
    defaults,
    allowed_env: &[],
    validate_options,
    build,
    disabled_by_default: true,
    risk_warning: Some(RISK_WARNING),
    validate_settings,
    probe: ProbeSpec::Version(&["--version"]),
    default_binary: DEFAULT_BINARY,
    npm_entrypoint: None,
    install_hint: "curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash",
};

fn defaults() -> ProviderSettings {
    ProviderSettings {
        enabled: true,
        binary: None,
        model: DEFAULT_MODEL.to_owned(),
        timeout: DEFAULT_TIMEOUT,
        env: BTreeMap::new(),
        options: BTreeMap::new(),
    }
}

/// Kimi accepts no adapter options in this version.
fn validate_options(options: &[RawOption<'_>]) -> Result<(), ConfigError> {
    if let Some(option) = options.first() {
        return Err(ConfigError::at(
            option.key_at,
            format!("providers.{ID}.options.{} is not supported", option.key),
        ));
    }
    Ok(())
}

/// When enabled, the model must be a plausible alias: nonempty, not
/// flag-like, and free of whitespace and control characters (it lands in
/// argv after `--model`).
fn validate_settings(
    settings: &ProviderSettings,
    locations: &ProviderLocations,
) -> Result<(), ConfigError> {
    if !settings.enabled {
        return Ok(());
    }
    if !is_valid_model(&settings.model) {
        return Err(ConfigError::at(
            locations.model.unwrap_or(serde_saphyr::Location::UNKNOWN),
            format!(
                "providers.{ID}.model must be a nonempty model alias without whitespace, control characters or a leading '-'"
            ),
        ));
    }
    Ok(())
}

fn is_valid_model(model: &str) -> bool {
    !model.is_empty()
        && !model.starts_with('-')
        && !model.chars().any(|c| c.is_whitespace() || c.is_control())
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
    Ok(Arc::new(CliProvider::new(
        KimiAdapter::new(binary, settings.model.clone()),
        runner,
        settings.timeout,
    )))
}

/// Builds restricted `kimi -p` calls.
#[derive(Clone, Debug)]
pub struct KimiAdapter {
    binary: PathBuf,
    model: String,
}

impl KimiAdapter {
    /// `binary` is a command name looked up on PATH (normally `kimi`) or a
    /// path to the official CLI.
    pub fn new(binary: PathBuf, model: String) -> KimiAdapter {
        KimiAdapter { binary, model }
    }
}

impl CliAdapter for KimiAdapter {
    fn id(&self) -> &'static str {
        ID
    }

    fn invocation(&self, input: FormatInput<'_>) -> Result<CliInvocation, ProviderError> {
        // Guards direct library construction; the configuration loader runs
        // the same rules with file positions.
        if !is_valid_model(&self.model) {
            return Err(ProviderError::other(ProviderErrorCode::InvalidConfiguration));
        }

        let user = input.user_prompt;
        let message = [user.before_text, input.text, user.after_text].concat();
        if message.len() > MAX_USER_MESSAGE_BYTES {
            return Err(ProviderError::other(ProviderErrorCode::InputTooLarge));
        }

        let mut args: Vec<Argument> = ["--agent-file"]
            .into_iter()
            .map(Argument::literal)
            .collect();
        // The agent file's absolute path is materialized by the runner from
        // the control file, outside the call's workspace.
        args.push(Argument::ControlPath { file: 0 });
        args.extend(
            [
                "--skills-dir",
                ".",
                "--output-format",
                "stream-json",
                "--model",
            ]
            .into_iter()
            .map(Argument::literal),
        );
        args.push(Argument::literal(&self.model));
        args.push(Argument::literal("-p"));
        args.push(Argument::literal(&message));

        let env: BTreeMap<OsString, OsString> = DISABLE_ENV_VARS
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect();

        Ok(CliInvocation {
            program: ProgramSpec {
                binary: self.binary.clone(),
                // Single-binary install; no npm launcher exists to translate.
                npm_entrypoint: None,
            },
            args,
            // Print mode takes the prompt from argv and never reads stdin;
            // it is closed immediately.
            stdin: Vec::new(),
            env,
            remove_env: REMOVE_ENV_VARS.into_iter().map(OsString::from).collect(),
            control_files: vec![ControlFile {
                name: AGENT_FILE_NAME,
                contents: agent_file(input.system_prompt).into_bytes(),
            }],
            workspace_files: Vec::new(),
            parser: parse_output,
        })
    }
}

/// The custom agent file: the front matter disables every tool and all
/// delegation; the body is the composed system prompt verbatim.
///
/// The body is rendered by the CLI as a `${var}` template: unknown
/// variables stay verbatim, but the documented context variables
/// (`${cwd}`, `${now}`, `${base_prompt}`, …) in configured instructions
/// would expand. Pumice's own prompt carries none; the residual is
/// documented on the module and in the research note.
fn agent_file(system_prompt: &str) -> String {
    let mut file = String::from(
        "---\nname: pumice\ndescription: Formats dictation without taking actions.\ntools: []\nsubagents: []\n---\n\n",
    );
    file.push_str(system_prompt);
    file.push('\n');
    file
}

/// Parses Kimi's `--output-format stream-json` JSONL stream (one message
/// per line).
///
/// A run opens with the `system.version` meta message and, on success,
/// carries one or more `assistant` messages whose `content` strings
/// concatenate into the result; the `session.resume_hint` meta message
/// closes it. A `tool` message, or an `assistant` message with
/// `tool_calls`, means the model tried to use a tool and is rejected even
/// with a clean exit. `turn.step.retrying` meta messages carry the
/// failure's `status_code`, `error_name` and `error_message`: those
/// structured fields (never message text) classify authentication, quota
/// and rate-limit failures of a run that ultimately failed; a retry the
/// run recovered from (assistant text, clean exit) is not a failure.
/// Anything outside this vocabulary, or an
/// assistant `content` that is not a string, is invalid output.
pub fn parse_output(output: &ProcessOutput) -> Result<String, ProviderError> {
    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut texts: Vec<String> = Vec::new();
    let mut saw_version = false;
    let mut tool_activity = false;
    let mut failure: Option<ProviderError> = None;

    for line in stdout.lines() {
        if line.is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
        };
        match message.get("role").and_then(Value::as_str) {
            Some("meta") => {
                if message.get("type").and_then(Value::as_str) == Some("system.version") {
                    saw_version = true;
                    continue;
                }
                // The retrying message is the protocol's structured failure
                // evidence; only its fields are read, never its text.
                if message.get("type").and_then(Value::as_str) == Some("turn.step.retrying")
                    && failure.is_none()
                {
                    failure = Some(classify_retry(&message));
                }
                // session.resume_hint and future meta messages carry no
                // result text.
            }
            Some("assistant") => {
                if let Some(content) = message.get("content") {
                    let Some(text) = content.as_str() else {
                        return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
                    };
                    texts.push(text.to_owned());
                }
                if message.get("tool_calls").is_some() {
                    tool_activity = true;
                }
            }
            Some("tool") => tool_activity = true,
            _ => return Err(ProviderError::other(ProviderErrorCode::InvalidOutput)),
        }
    }

    if tool_activity {
        return Err(ProviderError::other(
            ProviderErrorCode::UnexpectedToolActivity,
        ));
    }
    // The structured failure evidence only classifies a run that ultimately
    // failed; a retry the run recovered from (assistant text, clean exit)
    // is not a failure.
    if !output.status.success() || texts.is_empty() {
        if let Some(err) = failure {
            return Err(err);
        }
        if !output.status.success() {
            return Err(ProviderError::other(ProviderErrorCode::NonzeroExit));
        }
        return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
    }
    if !saw_version {
        return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
    }
    Ok(texts.concat())
}

/// Maps a `turn.step.retrying` meta message to a safe error using only its
/// structured fields. `status_code` is the provider's HTTP status;
/// `error_name`/`error_message` name the failure kind. Retries are CLI
/// diagnostics (never dictation), so a text match here cannot misread
/// formatted output.
fn classify_retry(message: &Value) -> ProviderError {
    let status = message.get("status_code").and_then(Value::as_u64);
    match status {
        Some(401) => return ProviderError::NotLoggedIn,
        Some(403) => return ProviderError::other(ProviderErrorCode::AuthenticationRejected),
        Some(429) => return ProviderError::RateLimited { retry_after: None },
        _ => {}
    }
    let named = [message.get("error_name"), message.get("error_message")]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|part| part.to_lowercase().contains("quota"));
    if named {
        return ProviderError::QuotaExceeded { retry_after: None };
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
    fn accepts_no_options() {
        assert!(validate_options(&[]).is_ok());
        assert!(validate_options(&[option("effort", "low")]).is_err());
    }

    #[test]
    fn validates_models() {
        for good in ["kimi-k2.7-code-highspeed", "kimi-k2.8-code", "k3-256k"] {
            assert!(is_valid_model(good), "model: {good:?}");
        }
        for bad in ["", "-model", "bad model", "bad\tmodel", "bad\u{7f}model"] {
            assert!(!is_valid_model(bad), "model: {bad:?}");
        }
    }

    #[test]
    fn settings_validation_accepts_the_default_model() {
        assert!(validate_settings(&defaults(), &ProviderLocations::default()).is_ok());
        let mut settings = defaults();
        settings.model = "-x".to_owned();
        assert!(validate_settings(&settings, &ProviderLocations::default()).is_err());
    }

    #[test]
    fn agent_file_disables_tools_and_carries_the_prompt() {
        let file = agent_file("You format speech. Coração 🎤");
        assert!(file.starts_with("---\nname: pumice\n"));
        assert!(file.contains("subagents: []\n---\n"));
        assert!(file.contains("subagents: []\n"));
        assert!(file.ends_with("You format speech. Coração 🎤\n"));
    }

    #[test]
    fn classifies_retry_failures() {
        let retry = |overrides: Value| {
            let mut message = json!({"role":"meta","type":"turn.step.retrying","failed_attempt":1,"next_attempt":2,"max_attempts":3});
            message.as_object_mut().unwrap().extend(
                overrides
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone())),
            );
            message
        };
        assert_eq!(
            classify_retry(&retry(json!({"status_code": 401}))),
            ProviderError::NotLoggedIn
        );
        assert_eq!(
            classify_retry(&retry(json!({"status_code": 403}))),
            ProviderError::other(ProviderErrorCode::AuthenticationRejected)
        );
        assert_eq!(
            classify_retry(&retry(json!({"status_code": 429}))),
            ProviderError::RateLimited { retry_after: None }
        );
        assert_eq!(
            classify_retry(&retry(json!({"error_name": "QuotaExceededError"}))),
            ProviderError::QuotaExceeded { retry_after: None }
        );
        assert_eq!(
            classify_retry(&retry(json!({"status_code": 500}))),
            ProviderError::other(ProviderErrorCode::NonzeroExit)
        );
    }
}
