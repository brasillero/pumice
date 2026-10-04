//! Codex adapter (`codex exec`).
//!
//! The invocation and the JSONL event stream were verified in S0.2 (Codex
//! 0.160.0): `--json` emits one event per line on stdout, the formatted text
//! is the last completed `agent_message` item, and a failed turn ends in
//! `turn.failed` after a handful of retried `error` events.
//!
//! Codex accepts exactly one adapter option in Phase 1:
//! `options.openai_base_url`, a non-secret `http://`/`https://` routing
//! override forwarded as `-c openai_base_url="<url>"`. `--ignore-user-config`
//! also drops the user's own `openai_base_url` (the S0.2 config trap, which
//! made Codex fall back to an environment API key against api.openai.com), so
//! an account that needs a gateway must set it here explicitly.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::cli::{CliAdapter, CliProvider};
use super::{
    FormatInput, ProbeSpec, Provider, ProviderDescriptor, ProviderError, ProviderErrorCode,
    ProviderSettings, RawOption, validate_settings_noop,
};
use crate::config::ConfigError;
use crate::process::{
    Argument, CliInvocation, ControlFile, ProcessOutput, ProcessRunner, ProgramSpec,
    toml_basic_string,
};

pub const ID: &str = "codex";
pub const DEFAULT_BINARY: &str = "codex";
/// Recommended initial model, measured in Phase 0.
pub const DEFAULT_MODEL: &str = "gpt-6.1-sol";
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Codex accepts no environment overrides: `--ignore-user-config` would drop
/// them anyway, and routing belongs in `options.openai_base_url`.
const ALLOWED_ENV: &[&str] = &[];

const OPENAI_BASE_URL_KEY: &str = "openai_base_url";

/// npm package entrypoint the Windows `.cmd` shim translates to (layout from
/// npm; verify on a real Windows install (owner check)).
const NPM_ENTRYPOINT: &str = "@openai/codex/bin/codex.js";

pub const DESCRIPTOR: ProviderDescriptor = ProviderDescriptor {
    id: ID,
    defaults,
    allowed_env: ALLOWED_ENV,
    validate_options,
    build,
    disabled_by_default: false,
    risk_warning: None,
    validate_settings: validate_settings_noop,
    probe: ProbeSpec::Version(&["--version"]),
    default_binary: DEFAULT_BINARY,
    npm_entrypoint: Some(NPM_ENTRYPOINT),
    install_hint: "npm install -g @openai/codex",
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

/// Only `openai_base_url` is allowed; it must be an `http://` or `https://`
/// URL without whitespace. The value lands inside a TOML string in a `-c`
/// argument, so the invocation encodes it (see [`toml_basic_string`]).
fn validate_options(options: &[RawOption<'_>]) -> Result<(), ConfigError> {
    for option in options {
        if option.key != OPENAI_BASE_URL_KEY {
            return Err(ConfigError::at(
                option.key_at,
                format!("providers.{ID}.options.{} is not supported", option.key),
            ));
        }
        if !is_valid_base_url(option.value) {
            return Err(ConfigError::at(
                option.value_at,
                format!(
                    "providers.{ID}.options.{OPENAI_BASE_URL_KEY} must be an http:// or https:// URL with a valid host"
                ),
            ));
        }
    }
    Ok(())
}

/// An `http://` or `https://` URL with a real host: `scheme://host[:port][/path]`.
/// No whitespace, userinfo, query or fragment; the host is a DNS name, an IPv4
/// address or a bracketed IPv6 address, and the port (if any) is numeric.
fn is_valid_base_url(url: &str) -> bool {
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let Some(rest) = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
    else {
        return false;
    };
    if rest.contains(['?', '#', '@']) {
        return false;
    }
    let authority = rest.split('/').next().unwrap_or_default();
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let Some((inside, after)) = bracketed.split_once(']') else {
            return false;
        };
        if inside.is_empty() || !inside.chars().all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.') {
            return false;
        }
        match after {
            "" => (inside, None),
            _ => match after.strip_prefix(':') {
                Some(port) => (inside, Some(port)),
                None => return false,
            },
        }
    } else {
        match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    let host_ok = !host.is_empty()
        && (authority.starts_with('[')
            || host
                .split('.')
                .all(|label| !label.is_empty() && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')));
    let port_ok = port.is_none_or(|p| !p.is_empty() && p.len() <= 5 && p.chars().all(|c| c.is_ascii_digit()) && p.parse::<u16>().is_ok_and(|n| n > 0));
    host_ok && port_ok
}

fn build(
    settings: &ProviderSettings,
    runner: Arc<ProcessRunner>,
) -> Result<Arc<dyn Provider>, ConfigError> {
    let binary = settings
        .binary
        .clone()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_BINARY));
    let base_url = settings.options.get(OPENAI_BASE_URL_KEY).cloned();
    Ok(Arc::new(CliProvider::new(
        CodexAdapter::new(binary, settings.model.clone(), base_url),
        runner,
        settings.timeout,
    )))
}

/// Builds restricted `codex exec` calls.
#[derive(Clone, Debug)]
pub struct CodexAdapter {
    binary: PathBuf,
    model: String,
    /// Non-secret routing override, forwarded as a `-c openai_base_url=…`
    /// argument when set.
    base_url: Option<String>,
}

impl CodexAdapter {
    /// `binary` is a command name looked up on PATH (normally `codex`) or a
    /// path to the official CLI.
    pub fn new(binary: PathBuf, model: String, base_url: Option<String>) -> CodexAdapter {
        CodexAdapter {
            binary,
            model,
            base_url,
        }
    }
}

impl CliAdapter for CodexAdapter {
    fn id(&self) -> &'static str {
        ID
    }

    fn invocation(&self, input: FormatInput<'_>) -> Result<CliInvocation, ProviderError> {
        // A model that looks like a flag would change the command's meaning.
        if self.model.is_empty() || self.model.starts_with('-') {
            return Err(ProviderError::other(ProviderErrorCode::InvalidConfiguration));
        }

        let mut args: Vec<Argument> = [
            "--no-daemon",
            "exec",
            "--ignore-user-config",
            "--ignore-rules",
            "--ephemeral",
            "--skip-git-repo-check",
            "--sandbox",
            "read-only",
            "--color",
            "never",
            "--json",
        ]
        .into_iter()
        .map(Argument::literal)
        .collect();

        // The routing override sits after --json, matching the invocation
        // verified in S0.2. The URL is TOML-encoded inside the value.
        if let Some(base_url) = &self.base_url {
            args.push(Argument::literal("-c"));
            args.push(Argument::literal(format!(
                "{OPENAI_BASE_URL_KEY}={}",
                toml_basic_string(base_url)
            )));
        }

        args.push(Argument::literal("-m"));
        args.push(Argument::literal(&self.model));
        for value in [
            r#"web_search="disabled""#,
            "features.shell_tool=false",
            "features.unified_exec=false",
            "features.multi_agent=false",
            "features.hooks=false",
            // `view_image` can read files and its activity never appears in
            // the JSONL stream, so it must be off.
            "features.view_image=false",
            "features.remote_plugin=false",
        ] {
            args.push(Argument::literal("-c"));
            args.push(Argument::literal(value));
        }
        // The system prompt is a control file outside the workspace; the path
        // is embedded in the -c value as a TOML string.
        args.push(Argument::literal("-c"));
        args.push(Argument::ConfigControlPath {
            key: "model_instructions_file",
            file: 0,
        });
        args.extend([
            Argument::literal("-c"),
            Argument::literal(r#"model_reasoning_effort="low""#),
            // `-` reads the user message from stdin.
            Argument::literal("-"),
        ]);

        let user = input.user_prompt;
        let stdin = [user.before_text, input.text, user.after_text].concat();

        Ok(CliInvocation {
            program: ProgramSpec {
                binary: self.binary.clone(),
                npm_entrypoint: Some(NPM_ENTRYPOINT),
            },
            args,
            stdin: stdin.into_bytes(),
            env: BTreeMap::new(),
            remove_env: Vec::new(),
            control_files: vec![ControlFile {
                name: "system.txt",
                contents: input.system_prompt.as_bytes().to_vec(),
            }],
            parser: parse_output,
        })
    }
}

/// Parses Codex's `--json` JSONL event stream (one event per line).
///
/// Success requires a `turn.completed` event and exit code 0; the result is
/// the text of the last completed `agent_message` item. Failure events carry
/// CLI diagnostics, never dictation, so they are the only text that is
/// classified; an agent message is returned only from a run that completed
/// cleanly, and reasoning or progress text is never concatenated into it.
pub fn parse_output(output: &ProcessOutput) -> Result<String, ProviderError> {
    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut last_agent_message: Option<String> = None;
    let mut saw_turn_completed = false;
    let mut saw_turn_failed = false;
    let mut tool_activity = false;
    let mut failure_messages: Vec<String> = Vec::new();

    for line in stdout.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
        };
        let Some(event_type) = event.get("type").and_then(Value::as_str) else {
            return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
        };
        match event_type {
            "item.started" | "item.updated" | "item.completed" => {
                match event.pointer("/item/type").and_then(Value::as_str) {
                    // Only agent output may carry text; anything else is a
                    // tool or activity item and must never reach the result.
                    Some("agent_message") if event_type == "item.completed" => {
                        let text = event
                            .pointer("/item/text")
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                ProviderError::other(ProviderErrorCode::InvalidOutput)
                            })?;
                        last_agent_message = Some(text.to_owned());
                    }
                    // Codex reports non-fatal warnings (config, deprecation,
                    // model rerouting) as `error` items; they carry no tool
                    // activity and no output text.
                    Some("agent_message" | "reasoning" | "error") => {}
                    Some(_) => tool_activity = true,
                    None => return Err(ProviderError::other(ProviderErrorCode::InvalidOutput)),
                }
            }
            "turn.completed" => saw_turn_completed = true,
            "turn.failed" => {
                saw_turn_failed = true;
                if let Some(message) = event.pointer("/error/message").and_then(Value::as_str) {
                    failure_messages.push(message.to_owned());
                }
            }
            "error" => {
                if let Some(message) = event.get("message").and_then(Value::as_str) {
                    failure_messages.push(message.to_owned());
                }
            }
            // thread.started, turn.started and future informational events.
            _ => {}
        }
    }

    if tool_activity {
        return Err(ProviderError::other(
            ProviderErrorCode::UnexpectedToolActivity,
        ));
    }
    // Transient `error` events ("Reconnecting... 1/5") can precede a turn that
    // still completes, so they only matter when the turn did not succeed.
    let succeeded = saw_turn_completed && !saw_turn_failed && output.status.success();
    if !succeeded {
        if !failure_messages.is_empty() {
            return Err(classify_failure(&failure_messages));
        }
        if !output.status.success() {
            return Err(ProviderError::other(ProviderErrorCode::NonzeroExit));
        }
        return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
    }
    last_agent_message.ok_or_else(|| ProviderError::other(ProviderErrorCode::InvalidOutput))
}

/// Maps failure-event messages to a safe error.
///
/// Only clear signatures are matched; anything else is a generic failure.
/// Event text is never copied into the error (which is logged), and these
/// events are CLI diagnostics, so dictated text cannot be misread here.
fn classify_failure(messages: &[impl AsRef<str>]) -> ProviderError {
    let lowered: Vec<String> = messages.iter().map(|m| m.as_ref().to_lowercase()).collect();
    let any_contains = |needles: &[&str]| {
        lowered
            .iter()
            .any(|message| needles.iter().all(|needle| message.contains(needle)))
    };
    if any_contains(&["401", "missing bearer"]) {
        return ProviderError::NotLoggedIn;
    }
    if any_contains(&["401", "incorrect api key"]) {
        return ProviderError::other(ProviderErrorCode::AuthenticationRejected);
    }
    if any_contains(&["429"]) || any_contains(&["rate limit"]) {
        return ProviderError::RateLimited { retry_after: None };
    }
    if any_contains(&["usage limit"]) || any_contains(&["quota exceeded"]) {
        return ProviderError::QuotaExceeded { retry_after: None };
    }
    ProviderError::other(ProviderErrorCode::NonzeroExit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_saphyr::Location;

    fn option(key: &'static str, value: &'static str) -> RawOption<'static> {
        RawOption {
            key,
            value,
            key_at: Location::UNKNOWN,
            value_at: Location::UNKNOWN,
        }
    }

    #[test]
    fn accepts_only_openai_base_url() {
        assert!(validate_options(&[]).is_ok());
        assert!(validate_options(&[option("openai_base_url", "https://gw.example/v1")]).is_ok());
        assert!(validate_options(&[option("openai_base_url", "http://127.0.0.1:9999")]).is_ok());
        assert!(validate_options(&[option("web_search", "disabled")]).is_err());
        assert!(validate_options(&[option("model", "gpt-6.1-sol")]).is_err());
    }

    #[test]
    fn rejects_invalid_base_urls() {
        for value in [
            "ftp://gw.example/v1",
            "gw.example/v1",
            "http://",
            "https://",
            "http://exa mple/v1",
            "https://gw.example/v1\n",
        ] {
            assert!(
                validate_options(&[option("openai_base_url", value)]).is_err(),
                "value: {value:?}"
            );
        }
    }

    #[test]
    fn classifies_failure_messages() {
        assert_eq!(
            classify_failure(&["unexpected status 401 Unauthorized: Missing bearer or basic authentication in header"]),
            ProviderError::NotLoggedIn
        );
        assert_eq!(
            classify_failure(&["unexpected status 401 Unauthorized: Incorrect API key provided"]),
            ProviderError::other(ProviderErrorCode::AuthenticationRejected)
        );
        assert_eq!(
            classify_failure(&["unexpected status 429 Too Many Requests"]),
            ProviderError::RateLimited { retry_after: None }
        );
        assert_eq!(
            classify_failure(&["rate limit reached, retry later"]),
            ProviderError::RateLimited { retry_after: None }
        );
        assert_eq!(
            classify_failure(&["usage limit reached, try again tomorrow"]),
            ProviderError::QuotaExceeded { retry_after: None }
        );
        assert_eq!(
            classify_failure(&["quota exceeded for this account"]),
            ProviderError::QuotaExceeded { retry_after: None }
        );
        // A bare 401 without a known wording is not enough.
        assert_eq!(
            classify_failure(&["unexpected status 401 Unauthorized"]),
            ProviderError::other(ProviderErrorCode::NonzeroExit)
        );
        assert_eq!(
            classify_failure(&["something broke"]),
            ProviderError::other(ProviderErrorCode::NonzeroExit)
        );
        assert_eq!(
            classify_failure(&[] as &[&str]),
            ProviderError::other(ProviderErrorCode::NonzeroExit)
        );
    }

    #[test]
    fn failure_classification_uses_one_message_for_pairs() {
        // "401" and the wording must occur in the same message.
        assert_eq!(
            classify_failure(&["status 401", "missing bearer elsewhere"]),
            ProviderError::other(ProviderErrorCode::NonzeroExit)
        );
    }
}
