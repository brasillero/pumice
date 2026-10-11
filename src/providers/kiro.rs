//! Kiro CLI plugin (`kiro-cli chat`).
//!
//! Verified against the installed 2.29.0 in S2.14 (real calls, see
//! docs/research/S2.14-kiro-plugin.md). Headless mode on the pinned V2
//! engine reads the user message from stdin and writes the run's ACP events
//! as JSON Lines (`--output-format stream-json`):
//!
//! ```text
//! {"type":"runStarted","data":{"payloadSchema":"acp","acpProtocolVersion":1,"engine":"v2"}}
//! {"type":"metadata","data":{…}}
//! {"type":"sessionUpdate","data":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"…"}}}}
//! {"type":"runFinished","data":{"status":"success","stopReason":"end_turn","finalText":"…","finalTextTruncated":false}}
//! ```
//!
//! A failed run ends with `{"type":"runError","data":{"stage":…,"message":…}}`
//! and exit code 1.
//!
//! Kiro has no system-prompt flag. The client's system prompt goes into a
//! one-off custom agent, `.kiro/agents/pumice.json`, written into the call's
//! fresh working directory, the only place Kiro discovers a local agent (an
//! owner-approved exception to the empty-workspace rule). A workspace agent
//! wins over a global one with the same name. The agent has no tools, no MCP
//! servers, no resources and no hooks, and `--trust-tools=` trusts none. The
//! user's own Kiro login and settings are inherited; `~/.kiro` is never
//! touched.
//!
//! Residual risks (recorded here and in the research note; Pumice prints no
//! provider warnings): each run is saved in the user's own `~/.kiro/sessions`,
//! with the dictated text; `runError` carries no structured cause, so login,
//! quota and rate-limit failures are all `NonzeroExit`; a CLI that is not
//! logged in starts a device-code login and waits, which ends in `Timeout`.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use super::cli::{CliAdapter, CliProvider};
use super::{
    FormatInput, ProbeSpec, Provider, ProviderDescriptor, ProviderError, ProviderErrorCode,
    ProviderSettings, validate_settings_noop,
};
use crate::config::ConfigError;
use crate::process::{
    Argument, CliInvocation, ControlFile, ProcessOutput, ProcessRunner, ProgramSpec,
};

pub const ID: &str = "kiro";
pub const DEFAULT_BINARY: &str = "kiro-cli";
/// Provider default timeout (real calls took 2.4–5.3 s, S2.14).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Name of the one-off agent, passed to `--agent`.
pub const AGENT_NAME: &str = "pumice";
/// The agent file inside the call's working directory.
pub const AGENT_FILE: &str = ".kiro/agents/pumice.json";

/// The line a CLI that is not logged in prints to stdout when it starts a
/// device-code login (verified with an empty scratch `HOME`, S2.14).
const DEVICE_LOGIN_PROMPT: &str = "Confirm the following code in the browser";

/// Plugin-owned child environment: no telemetry (AGENTS.md) and no update
/// checks from runs Pumice starts. Both names are read by the 2.29.0 binary.
const CHILD_ENV: [(&str, &str); 2] = [
    ("KIRO_DISABLE_TELEMETRY", "1"),
    ("KIRO_NO_AUTO_UPDATE", "1"),
];

pub const DESCRIPTOR: ProviderDescriptor = ProviderDescriptor {
    id: ID,
    defaults,
    build,
    validate_settings: validate_settings_noop,
    probe: ProbeSpec::Version(&["--version"]),
    default_binary: DEFAULT_BINARY,
    npm_entrypoint: None,
    install_hint: "curl -fsSL https://cli.kiro.dev/install | bash",
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
        KiroAdapter::new(binary, settings.model.clone()),
        runner,
        settings.timeout,
    )))
}

/// Builds restricted `kiro-cli chat` calls.
#[derive(Clone, Debug)]
pub struct KiroAdapter {
    binary: PathBuf,
    model: String,
}

impl KiroAdapter {
    /// `binary` is a command name looked up on PATH (normally `kiro-cli`) or
    /// a path to the official CLI.
    pub fn new(binary: PathBuf, model: String) -> KiroAdapter {
        KiroAdapter { binary, model }
    }
}

impl CliAdapter for KiroAdapter {
    fn id(&self) -> &'static str {
        ID
    }

    fn invocation(&self, input: FormatInput<'_>) -> Result<CliInvocation, ProviderError> {
        // A model that looks like a flag would change the command's meaning.
        if self.model.is_empty() || self.model.starts_with('-') {
            return Err(ProviderError::other(ProviderErrorCode::InvalidConfiguration));
        }
        // Kiro reads a `file://` agent prompt from disk; passing one through
        // would make the CLI load a local file chosen by the request.
        if is_file_uri(input.system_prompt) {
            return Err(ProviderError::other(
                ProviderErrorCode::UnsupportedSystemPrompt,
            ));
        }

        let mut args: Vec<Argument> = [
            "chat",
            // Pinned: the CLI's default engine may change (V2 is "the
            // pre-3.0 default").
            "--agent-engine",
            "v2",
            "--no-interactive",
            "--output-format",
            "stream-json",
            // Trust no tools: an untrusted call is denied in headless mode.
            "--trust-tools=",
            "--agent",
            AGENT_NAME,
            "--model",
        ]
        .into_iter()
        .map(Argument::literal)
        .collect();
        args.push(Argument::literal(&self.model));
        // Cheapest settings (owner rule): the lowest level every reasoning
        // model accepts. Models without levels warn on stderr and run.
        args.push(Argument::literal("--effort"));
        args.push(Argument::literal("low"));

        let user = input.user_prompt;
        let stdin = [user.before_text, input.text, user.after_text].concat();

        let env: BTreeMap<OsString, OsString> = CHILD_ENV
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect();

        Ok(CliInvocation {
            program: ProgramSpec {
                binary: self.binary.clone(),
                // Native binary install; no npm launcher exists to translate.
                npm_entrypoint: None,
            },
            args,
            // With no positional argument, headless mode reads the whole of
            // stdin as the instruction.
            stdin: stdin.into_bytes(),
            env,
            remove_env: Vec::new(),
            control_files: Vec::new(),
            workspace_files: vec![ControlFile {
                name: AGENT_FILE.to_owned(),
                contents: agent_file(input.system_prompt).into_bytes(),
            }],
            parser: parse_output,
        })
    }
}

/// The one-off agent: the client's system prompt verbatim (empty when it
/// sent none), no tools, no pre-approved tools, no MCP servers (its own or
/// from `mcp.json`), no resources and no hooks.
pub fn agent_file(system_prompt: &str) -> String {
    json!({
        "name": AGENT_NAME,
        "description": "Answers one message without tools.",
        "prompt": system_prompt,
        "tools": [],
        "allowedTools": [],
        "mcpServers": {},
        "includeMcpJson": false,
        "resources": [],
        "hooks": {},
    })
    .to_string()
}

/// True when Kiro could read `prompt` as a `file://` URI instead of text.
/// Leading whitespace and letter case are ignored to stay on the safe side.
fn is_file_uri(prompt: &str) -> bool {
    prompt
        .trim_start()
        .get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("file:"))
}

/// Where a V2 run is, as its events arrive.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RunState {
    NotStarted,
    Running,
    Finished { success: bool },
}

/// Parses Kiro's V2 `--output-format stream-json` events (one JSON object
/// per line, `{"type": …, "data": …}`).
///
/// A run is exactly one `runStarted` (ACP payload, V2 engine) first, then
/// its events, then exactly one `runFinished` last. The answer is the
/// concatenation of the `agent_message_chunk` texts inside the run; it equals
/// `runFinished.finalText`, which is not used because it can be truncated.
/// Every other session update (thoughts, tool calls) and every other event
/// type inside the run (`metadata`, future types) carries no answer text and
/// is ignored: Pumice does not police tool use in the output (owner decision
/// 2026-10-08); the agent file switches tools off instead.
///
/// A `runError` (anywhere) or a nonzero exit is `NonzeroExit`: Kiro's errors
/// carry only a free-text message, which is not matched. Any other event
/// before `runStarted` or after `runFinished`, a second start or finish, a
/// finish whose status is not `success`, or no answer text is invalid output.
/// A line that is not JSON is invalid output, except the device-code login
/// prompt, which means the CLI is not logged in.
pub fn parse_output(output: &ProcessOutput) -> Result<String, ProviderError> {
    let invalid = || ProviderError::other(ProviderErrorCode::InvalidOutput);
    let stdout = super::cli::stdout_text(output)?;

    let mut state = RunState::NotStarted;
    let mut run_error = false;
    let mut texts: Vec<String> = Vec::new();

    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            if line.trim() == DEVICE_LOGIN_PROMPT {
                return Err(ProviderError::NotLoggedIn);
            }
            return Err(invalid());
        };
        let data = event.get("data");
        let field = |name: &str| {
            data.and_then(|data| data.get(name))
                .and_then(Value::as_str)
        };
        match (event.get("type").and_then(Value::as_str), state) {
            // A failure can be reported at any point of the run.
            (Some("runError"), _) => run_error = true,
            (Some("runStarted"), RunState::NotStarted) => {
                if field("payloadSchema") != Some("acp") || field("engine") != Some("v2") {
                    return Err(invalid());
                }
                state = RunState::Running;
            }
            (Some("sessionUpdate"), RunState::Running) => {
                let update = data.and_then(|data| data.get("update")).ok_or_else(invalid)?;
                if update.get("sessionUpdate").and_then(Value::as_str)
                    == Some("agent_message_chunk")
                {
                    let content = update.get("content").ok_or_else(invalid)?;
                    if content.get("type").and_then(Value::as_str) != Some("text") {
                        return Err(invalid());
                    }
                    let text = content.get("text").and_then(Value::as_str).ok_or_else(invalid)?;
                    texts.push(text.to_owned());
                }
            }
            (Some("runFinished"), RunState::Running) => {
                state = RunState::Finished {
                    success: field("status") == Some("success"),
                };
            }
            (Some("runStarted" | "sessionUpdate" | "runFinished"), _) => return Err(invalid()),
            (Some(_), RunState::Running) => {}
            _ => return Err(invalid()),
        }
    }

    if run_error || !output.status.success() {
        return Err(ProviderError::other(ProviderErrorCode::NonzeroExit));
    }
    if state != (RunState::Finished { success: true }) || texts.is_empty() {
        return Err(invalid());
    }
    Ok(texts.concat())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uri_prompts_are_detected() {
        for uri in [
            "file:///etc/hostname",
            "file://prompt.md",
            "  FILE:///x",
            "\nFile:relative",
        ] {
            assert!(is_file_uri(uri), "{uri:?}");
        }
        for text in ["", "file", "You format speech. file:///x", "files: none", "é"] {
            assert!(!is_file_uri(text), "{text:?}");
        }
    }

    #[test]
    fn agent_file_carries_the_prompt_and_no_tools() {
        let file: Value = serde_json::from_str(&agent_file("Formate. Coração 🎤 \"q\"")).unwrap();
        assert_eq!(
            file,
            json!({
                "name": "pumice",
                "description": "Answers one message without tools.",
                "prompt": "Formate. Coração 🎤 \"q\"",
                "tools": [],
                "allowedTools": [],
                "mcpServers": {},
                "includeMcpJson": false,
                "resources": [],
                "hooks": {},
            })
        );
    }
}
