//! Claude Code adapter (`claude -p`).
//!
//! The invocation and the error envelope were verified in S0.2 (Claude Code
//! 2.1.288): tools are disabled with `--tools ""`, the system prompt is a
//! control file, the user message goes on stdin, and the reply is one JSON
//! envelope whose `result` holds the text.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::cli::{CliAdapter, CliProvider};
use super::{
    FormatInput, Provider, ProviderDescriptor, ProviderError, ProviderErrorCode, ProviderSettings,
};
use crate::process::{Argument, CliInvocation, ControlFile, ProcessOutput, ProcessRunner, ProgramSpec};

pub const ID: &str = "claude";
pub const DEFAULT_BINARY: &str = "claude";
/// Recommended initial model, measured in Phase 0.
pub const DEFAULT_MODEL: &str = "haiku";
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Result text of the verified "not logged in" envelope is
/// `Not logged in · Please run /login`; only its stable start is matched.
const NOT_LOGGED_IN_PREFIX: &str = "Not logged in";

pub const DESCRIPTOR: ProviderDescriptor = ProviderDescriptor {
    id: ID,
    defaults,
    build,
};

fn defaults() -> ProviderSettings {
    ProviderSettings {
        binary: PathBuf::from(DEFAULT_BINARY),
        model: DEFAULT_MODEL.to_owned(),
        timeout: DEFAULT_TIMEOUT,
    }
}

fn build(settings: &ProviderSettings, runner: Arc<ProcessRunner>) -> Arc<dyn Provider> {
    Arc::new(CliProvider::new(
        ClaudeAdapter::new(settings.binary.clone(), settings.model.clone()),
        runner,
        settings.timeout,
    ))
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

        let flags = [
            "-p",
            "--safe-mode",
            "--restricted",
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

        Ok(CliInvocation {
            program: ProgramSpec {
                binary: self.binary.clone(),
                // The npm package layout on Windows is unverified (S5.1).
                npm_entrypoint: None,
            },
            args,
            stdin: stdin.into_bytes(),
            // Thinking off: it dominated latency in S0.2 (owner decision).
            env: BTreeMap::from([("MAX_THINKING_TOKENS".into(), "0".into())]),
            remove_env: Vec::new(),
            control_files: vec![ControlFile {
                name: "system.txt",
                contents: input.system_prompt.as_bytes().to_vec(),
            }],
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

    if !exited_ok || is_error != Some(false) {
        if is_error.is_none() && exited_ok {
            return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
        }
        return Err(classify_failure(result.and_then(Value::as_str)));
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
/// failure. This runs on failures only, so dictated text that mentions
/// limits cannot be misread as an error.
fn classify_failure(result: Option<&str>) -> ProviderError {
    let Some(text) = result.map(str::trim) else {
        return ProviderError::other(ProviderErrorCode::NonzeroExit);
    };
    if text.starts_with(NOT_LOGGED_IN_PREFIX) {
        return ProviderError::NotLoggedIn;
    }
    let lower = text.to_lowercase();
    let usage = lower.contains("usage limit");
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
