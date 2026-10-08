//! Claude Code adapter (`claude -p`).
//!
//! The invocation and the error envelope were verified in S0.2 (Claude Code
//! 2.1.288): tools are disabled with `--tools ""`, the system prompt is a
//! control file, the user message goes on stdin, and the reply is one JSON
//! envelope whose `result` holds the text.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::cli::{CliAdapter, CliProvider};
use super::{
    FormatInput, ProbeSpec, Provider, ProviderDescriptor, ProviderError, ProviderErrorCode,
    ProviderSettings, validate_settings_noop,
};
use crate::config::ConfigError;
use crate::process::{Argument, CliInvocation, ControlFile, ProcessOutput, ProcessRunner, ProgramSpec};

pub const ID: &str = "claude";
pub const DEFAULT_BINARY: &str = "claude";
/// Provider default timeout, measured in Phase 0.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Result text of the verified "not logged in" envelope is
/// `Not logged in · Please run /login`; only its stable start is matched.
const NOT_LOGGED_IN_PREFIX: &str = "Not logged in";


/// npm package entrypoint the Windows `.cmd` shim translates to (layout from
/// npm; verify on a real Windows install (owner check)).
const NPM_ENTRYPOINT: &str = "@anthropic-ai/claude-code/cli.js";

pub const DESCRIPTOR: ProviderDescriptor = ProviderDescriptor {
    id: ID,
    defaults,
    build,
    validate_settings: validate_settings_noop,
    probe: ProbeSpec::Version(&["--version"]),
    default_binary: DEFAULT_BINARY,
    npm_entrypoint: Some(NPM_ENTRYPOINT),
    install_hint: "npm install -g @anthropic-ai/claude-code",
};

fn defaults() -> ProviderSettings {
    ProviderSettings {
        enabled: false,
        binary: None,
        // The configuration file sets the model on every enabled entry;
        // nothing is built in.
        model: String::new(),
        timeout: DEFAULT_TIMEOUT,
    }
}


fn build(
    settings: &ProviderSettings,
    runner: Arc<ProcessRunner>,
) -> Result<Arc<dyn Provider>, ConfigError> {
    let binary = settings
        .binary
        .clone()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_BINARY));
    Ok(Arc::new(CliProvider::new(
        ClaudeAdapter::new(binary, settings.model.clone()),
        runner,
        settings.timeout,
    )))
}

/// Builds restricted `claude -p` calls.
#[derive(Clone, Debug)]
pub struct ClaudeAdapter {
    binary: PathBuf,
    model: String,
}

impl ClaudeAdapter {
    /// `binary` is a command name looked up on PATH (normally `claude`) or a
    /// path to the official CLI.
    pub fn new(binary: PathBuf, model: String) -> ClaudeAdapter {
        ClaudeAdapter { binary, model }
    }
}

impl CliAdapter for ClaudeAdapter {
    fn id(&self) -> &'static str {
        ID
    }

    fn invocation(&self, input: FormatInput<'_>) -> Result<CliInvocation, ProviderError> {
        // A model that looks like a flag would change the command's meaning.
        if self.model.is_empty() || self.model.starts_with('-') {
            return Err(ProviderError::other(ProviderErrorCode::InvalidConfiguration));
        }

        // No `--restricted`: it also ignores the user's settings files,
        // including the `env` block where a gateway URL and token usually
        // live, so Claude silently fell back to the subscription login.
        // `--tools ""` already removes every tool and `--safe-mode` every
        // customization (hooks, MCP servers, plugins, skills, CLAUDE.md).
        let flags = [
            "-p",
            "--safe-mode",
            "--tools",
            "",
            "--strict-mcp-config",
            "--disable-slash-commands",
            "--no-chrome",
            "--no-session-persistence",
            "--model",
        ];
        let mut args: Vec<Argument> = flags.into_iter().map(Argument::literal).collect();
        args.push(Argument::literal(&self.model));
        args.extend([
            Argument::literal("--output-format"),
            Argument::literal("json"),
            Argument::literal("--system-prompt-file"),
            Argument::ControlPath { file: 0 },
        ]);

        let user = input.user_prompt;
        let stdin = [user.before_text, input.text, user.after_text].concat();

        // Thinking off: the cheapest setting, and it dominated latency in
        // S0.2 (owner decision). Everything else the CLI inherits from the
        // user's own configuration.
        let env = BTreeMap::from([(
            OsString::from("MAX_THINKING_TOKENS"),
            OsString::from("0"),
        )]);

        Ok(CliInvocation {
            program: ProgramSpec {
                binary: self.binary.clone(),
                npm_entrypoint: Some(NPM_ENTRYPOINT),
            },
            args,
            stdin: stdin.into_bytes(),
            env,
            remove_env: Vec::new(),
            control_files: vec![ControlFile {
                name: "system.txt".to_owned(),
                contents: input.system_prompt.as_bytes().to_vec(),
            }],
            workspace_files: Vec::new(),
            parser: parse_output,
        })
    }
}

/// Parses Claude's `--output-format json` envelope.
///
/// A nonzero exit or `is_error: true` is a failure, even when `subtype` is
/// `"success"` (that is how "not logged in" is reported).
pub fn parse_output(output: &ProcessOutput) -> Result<String, ProviderError> {
    let exited_ok = output.status.success();
    let envelope = match serde_json::from_slice::<Value>(&output.stdout) {
        Ok(Value::Object(envelope)) => envelope,
        _ if !exited_ok => return Err(ProviderError::other(ProviderErrorCode::NonzeroExit)),
        _ => return Err(ProviderError::other(ProviderErrorCode::InvalidOutput)),
    };
    let is_error = envelope.get("is_error").and_then(Value::as_bool);
    let result = envelope.get("result");

    match (is_error, exited_ok) {
        // Only an error envelope's `result` is diagnostic text. Any other
        // `result` may be formatted dictation and is never classified.
        (Some(true), _) => return Err(classify_failure(result.and_then(Value::as_str))),
        (_, false) => return Err(ProviderError::other(ProviderErrorCode::NonzeroExit)),
        (None, true) => return Err(ProviderError::other(ProviderErrorCode::InvalidOutput)),
        (Some(false), true) => {}
    }
    if envelope.get("type").and_then(Value::as_str) != Some("result") {
        return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
    }
    match result {
        Some(Value::String(text)) => Ok(text.clone()),
        _ => Err(ProviderError::other(ProviderErrorCode::InvalidOutput)),
    }
}

/// Maps a failed envelope's `result` text to a safe error.
///
/// Only the "not logged in" text is verified. Limit messages are matched
/// only when they clearly name one kind of limit; anything else is a generic
/// failure. This runs on `is_error: true` envelopes only, so dictated text
/// that mentions limits or logins cannot be misread as an error.
fn classify_failure(result: Option<&str>) -> ProviderError {
    let Some(text) = result.map(str::trim) else {
        return ProviderError::other(ProviderErrorCode::NonzeroExit);
    };
    if text.starts_with(NOT_LOGGED_IN_PREFIX) {
        return ProviderError::NotLoggedIn;
    }
    let lower = text.to_lowercase();
    // "You've hit your weekly limit · resets …" is the subscription's quota
    // message (verified 2026-10-06).
    let usage = lower.contains("usage limit") || lower.contains("weekly limit");
    let rate = lower.contains("rate limit") || lower.contains("rate_limit");
    match (usage, rate) {
        (true, false) => ProviderError::QuotaExceeded { retry_after: None },
        (false, true) => ProviderError::RateLimited { retry_after: None },
        _ => ProviderError::other(ProviderErrorCode::NonzeroExit),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_failure_texts() {
        assert_eq!(
            classify_failure(Some("Not logged in · Please run /login")),
            ProviderError::NotLoggedIn
        );
        assert_eq!(
            classify_failure(Some("Claude AI usage limit reached|1760000000")),
            ProviderError::QuotaExceeded { retry_after: None }
        );
        assert_eq!(
            classify_failure(Some(
                r#"API Error: 429 {"type":"error","error":{"type":"rate_limit_error"}}"#
            )),
            ProviderError::RateLimited { retry_after: None }
        );
        assert_eq!(
            classify_failure(Some(
                "You've hit your weekly limit · resets Oct 8, 4am (America/Sao_Paulo)"
            )),
            ProviderError::QuotaExceeded { retry_after: None }
        );
        assert_eq!(
            classify_failure(Some("usage limit and rate limit")),
            ProviderError::other(ProviderErrorCode::NonzeroExit)
        );
        assert_eq!(
            classify_failure(Some("API Error: 500")),
            ProviderError::other(ProviderErrorCode::NonzeroExit)
        );
        assert_eq!(
            classify_failure(None),
            ProviderError::other(ProviderErrorCode::NonzeroExit)
        );
    }
}
