//! Claude Code adapter (`claude -p`).
//!
//! The invocation and the error envelope were verified in S0.2 (Claude Code
//! 2.1.288): tools are disabled with `--tools ""`, the system prompt is a
//! control file, the user message goes on stdin, and the reply is one JSON
//! envelope whose `result` holds the text.
//!
//! Effort and thinking come from `relay.yml` (p16-provider-effort/D4, D5);
//! both were checked against 2.1.306 and re-checked against the installed
//! 2.1.310 (docs/providers.md). `--effort` takes the level, and the same level
//! is pinned in `CLAUDE_CODE_EFFORT_LEVEL`, which overrides `--effort` when
//! inherited. `thinking: false` sets `MAX_THINKING_TOKENS=0`; `thinking:
//! true` clears it and sets `alwaysThinkingEnabled`, so the CLI uses its own
//! thinking mode and budget (adaptive where the model has it).
//! Models the CLI runs without effort, or with thinking always on, are
//! refused at startup, matched the way the CLI canonicalizes model names
//! (`models`).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use serde_saphyr::Location;

use super::cli::{CliAdapter, CliProvider};
use super::{
    Effort, FormatInput, ProbeSpec, Provider, ProviderDescriptor, ProviderError,
    ProviderErrorCode, ProviderLocations, ProviderSettings, effort_unsupported,
};
use crate::config::ConfigError;
use crate::process::{Argument, CliInvocation, ControlFile, ProcessOutput, ProcessRunner, ProgramSpec};

mod models;

pub const ID: &str = "claude";
pub const DEFAULT_BINARY: &str = "claude";

/// Result text of the verified "not logged in" envelope is
/// `Not logged in · Please run /login`; only its stable start is matched.
const NOT_LOGGED_IN_PREFIX: &str = "Not logged in";

/// npm package entrypoint the Windows `.cmd` shim translates to (layout from
/// npm; verify on a real Windows install (owner check)).
const NPM_ENTRYPOINT: &str = "@anthropic-ai/claude-code/cli.js";

pub const DESCRIPTOR: ProviderDescriptor = ProviderDescriptor {
    id: ID,
    build,
    thinking_switch: true,
    validate_settings,
    probe: ProbeSpec::Version(&["--version"]),
    default_binary: DEFAULT_BINARY,
    npm_entrypoint: Some(NPM_ENTRYPOINT),
    install_hint: "npm install -g @anthropic-ai/claude-code",
};

fn validate_settings(
    settings: &ProviderSettings,
    locations: &ProviderLocations,
) -> Result<(), ConfigError> {
    if models::has_no_effort(&settings.model) {
        return Err(effort_unsupported(
            ID,
            settings,
            locations,
            "Claude Code 2.1.310 drops effort for it",
        ));
    }
    if settings.thinking == Some(false) && models::keeps_thinking_on(&settings.model) {
        return Err(ConfigError::at(
            locations
                .thinking
                .or(locations.provider)
                .unwrap_or(Location::UNKNOWN),
            format!(
                "providers entry \"{ID}\".thinking: model \"{}\" cannot turn thinking off (Claude Code 2.1.310 sends adaptive thinking for it even with thinking off), so \"false\" cannot be honoured; write \"thinking: true\" or choose a model that can turn thinking off",
                settings.model
            ),
        ));
    }
    Ok(())
}

fn build(
    settings: &ProviderSettings,
    runner: Arc<ProcessRunner>,
) -> Result<Arc<dyn Provider>, ConfigError> {
    // The loader always sets `thinking` for a provider with the switch.
    let thinking = settings
        .thinking
        .ok_or_else(|| ConfigError::general("providers entry \"claude\" has no thinking"))?;
    Ok(Arc::new(CliProvider::new(
        ClaudeAdapter::new(
            settings.binary.clone(),
            settings.model.clone(),
            settings.effort,
            thinking,
        ),
        runner,
        settings.timeout,
    )))
}

/// The `--effort` and `CLAUDE_CODE_EFFORT_LEVEL` value for each level; the
/// installed CLI accepts low, medium, high, xhigh and max.
fn cli_effort(effort: Effort) -> &'static str {
    match effort {
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
    }
}

/// Environment pins for one call, in the order the settings layer writes
/// them: the thinking budget (`0` turns thinking off where the model allows
/// it; an empty value clears an inherited one, so the CLI's own default
/// applies), the effort level, no fast mode, and the thinking compatibility
/// switch cleared, because an inherited `1` drops the thinking parameter
/// altogether ("off" would become "upstream default", "on" would become
/// off). Bedrock's service tier is pinned to standard too: the fast-mode
/// switch does not cover an inherited Bedrock `priority` tier.
fn pinned_env(effort: Effort, thinking: bool) -> [(&'static str, &'static str); 5] {
    [
        ("MAX_THINKING_TOKENS", if thinking { "" } else { "0" }),
        ("CLAUDE_CODE_EFFORT_LEVEL", cli_effort(effort)),
        ("CLAUDE_CODE_DISABLE_FAST_MODE", "1"),
        ("CLAUDE_CODE_DISABLE_THINKING", ""),
        ("ANTHROPIC_BEDROCK_SERVICE_TIER", "default"),
    ]
}

/// The per-call `--settings` layer: fast mode off plus [`pinned_env`]. With
/// thinking allowed it also overrides a user's `alwaysThinkingEnabled:
/// false`, which would otherwise keep thinking off.
fn settings_json(effort: Effort, thinking: bool) -> String {
    let env: serde_json::Map<String, serde_json::Value> = pinned_env(effort, thinking)
        .into_iter()
        .map(|(key, value)| (key.to_owned(), serde_json::Value::from(value)))
        .collect();
    let mut layer = serde_json::json!({"fastMode": false});
    if thinking {
        layer["alwaysThinkingEnabled"] = serde_json::Value::Bool(true);
    }
    layer["env"] = serde_json::Value::Object(env);
    layer.to_string()
}

/// Builds restricted `claude -p` calls.
#[derive(Clone, Debug)]
pub struct ClaudeAdapter {
    binary: PathBuf,
    model: String,
    effort: Effort,
    thinking: bool,
}

impl ClaudeAdapter {
    /// `binary` is a command name looked up on PATH (normally `claude`) or a
    /// path to the official CLI.
    pub fn new(binary: PathBuf, model: String, effort: Effort, thinking: bool) -> ClaudeAdapter {
        ClaudeAdapter {
            binary,
            model,
            effort,
            thinking,
        }
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
        // The configured effort, and the same pins as a per-call settings
        // layer, because a user's settings `env` block would otherwise
        // overwrite the child environment. The layer only sets these keys;
        // the user's routing and login stay.
        args.extend([
            Argument::literal("--effort"),
            Argument::literal(cli_effort(self.effort)),
            Argument::literal("--settings"),
            Argument::literal(settings_json(self.effort, self.thinking)),
        ]);
        args.extend([
            Argument::literal("--output-format"),
            Argument::literal("json"),
            Argument::literal("--system-prompt-file"),
            Argument::ControlPath { file: 0 },
        ]);

        let user = input.user_prompt;
        let stdin = [user.before_text, input.text, user.after_text].concat();

        // The same pins in the child environment. Everything else the CLI
        // inherits from the user's own configuration.
        let env: BTreeMap<OsString, OsString> = pinned_env(self.effort, self.thinking)
            .into_iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect();

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
