//! Kiro adapter (`kiro-cli chat`).
//!
//! This adapter is **documentation-derived and unverified by runtime testing**:
//! the owner has `kiro-cli` installed but it is not set up for use, so no real
//! formatting call was made during implementation.
//!
//! Documented invocation (Kiro CLI headless mode, V3 engine):
//!
//! ```text
//! kiro-cli chat --v3 --no-interactive --agent pumice-<random hex> --model <model> \
//!     --output-format stream-json --trust-tools=
//! ```
//!
//! The instruction is supplied through stdin (no positional argument), so the
//! dictated text never reaches argv.
//!
//! Tool lockdown uses the most restrictive documented mode:
//!
//! * `--trust-tools=` (empty) trusts no individual tools.
//! * No `--trust-all-tools`.
//! * A workspace-local custom agent (`.kiro/agents/pumice-<random hex>.json`)
//!   supplies the system prompt in its `prompt` field, exposes no tools
//!   (`tools: []`), excludes the `knowledge` tool, disables MCP JSON / Powers
//!   inclusion, and clears per-agent `mcpServers` and `hooks` so the user's
//!   global MCP servers and hooks stay out of the call. The agent name is
//!   unique per call so a user agent named `pumice` cannot collide with or
//!   override the workspace-local file.
//!
//! The system prompt travels in the agent file's `prompt` field; the user
//! message (the wrapped dictation) travels on stdin. The user's own Kiro
//! profile (`~/.kiro`) is left untouched: the adapter does not set `KIRO_HOME`
//! or read any credential or token file.
//!
//! Output parsing: `--output-format stream-json` emits ACP v2
//! `session/update` events as JSON Lines. The parser collects
//! `agent_message`/`agent_message_chunk` text, flags any
//! `tool_call_update`/`tool_call_content_chunk` as unexpected tool activity,
//! and waits for a final `state_update` with `state: idle` and
//! `stopReason: end_turn` before accepting the result.

use std::collections::hash_map::RandomState;
use std::collections::{BTreeMap, HashMap};
use std::hash::{BuildHasher, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::cli::{CliAdapter, CliProvider};
use super::{
    FormatInput, ProbeSpec, Provider, ProviderDescriptor, ProviderError, ProviderErrorCode,
    ProviderLocations, ProviderSettings, RawOption,
};
use crate::config::ConfigError;
use crate::process::{
    Argument, CliInvocation, ControlFile, ProcessOutput, ProcessRunner, ProgramSpec,
};

pub const ID: &str = "kiro";
pub const DEFAULT_BINARY: &str = "kiro-cli";
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Counter mixed into every per-call agent name so two invocations made in
/// the same nanosecond cannot collide.
static AGENT_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Returns a fresh agent name for each call: `pumice-<64-bit hex>`.
///
/// The name is derived from OS-random hash keys (`RandomState`), the process
/// id, a per-call atomic counter and the current time. It is unique enough
/// that no user-created global agent can plausibly share it, preventing the
/// workspace-local agent file from being shadowed or merged with a global
/// `~/.kiro/agents/pumice.json`.
fn unique_agent_name() -> String {
    let mut entropy = Vec::with_capacity(32);
    entropy.extend_from_slice(&std::process::id().to_le_bytes());
    entropy.extend_from_slice(&AGENT_COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    entropy.extend_from_slice(
        &SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes(),
    );
    let mut hasher = RandomState::new().build_hasher();
    hasher.write(&entropy);
    format!("pumice-{:016x}", hasher.finish())
}

// Residual risk (recorded here; not printed, owner decision 2026-10-07):
// Enabling requires an explicit model; the invocation, output parser and
// tool-lockdown recipe are derived from documentation only.

const ALLOWED_ENV: &[&str] = &[];

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
    install_hint: "curl -fsSL https://cli.kiro.dev/install | bash",
};

fn defaults() -> ProviderSettings {
    ProviderSettings {
        enabled: false,
        binary: None,
        model: String::new(),
        timeout: DEFAULT_TIMEOUT,
        env: BTreeMap::new(),
        options: BTreeMap::new(),
    }
}

fn validate_options(options: &[RawOption<'_>]) -> Result<(), ConfigError> {
    if let Some(option) = options.first() {
        return Err(ConfigError::at(
            option.key_at,
            format!("providers.{ID}.options.{} is not supported", option.key),
        ));
    }
    Ok(())
}

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
                "providers.{ID}.model must be a nonempty token without whitespace, control characters or a leading '-'"
            ),
        ));
    }
    Ok(())
}

fn is_valid_model(model: &str) -> bool {
    if model.is_empty() || model.starts_with('-') {
        return false;
    }
    model.chars().all(|c| !c.is_whitespace() && !c.is_control())
}

fn build(
    settings: &ProviderSettings,
    runner: Arc<ProcessRunner>,
) -> Result<Arc<dyn Provider>, ConfigError> {
    validate_settings(settings, &ProviderLocations::default())?;
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

#[derive(Clone, Debug)]
pub struct KiroAdapter {
    binary: PathBuf,
    model: String,
}

impl KiroAdapter {
    pub fn new(binary: PathBuf, model: String) -> KiroAdapter {
        KiroAdapter { binary, model }
    }
}

impl CliAdapter for KiroAdapter {
    fn id(&self) -> &'static str {
        ID
    }

    fn invocation(&self, input: FormatInput<'_>) -> Result<CliInvocation, ProviderError> {
        if !is_valid_model(&self.model) {
            return Err(ProviderError::other(
                ProviderErrorCode::InvalidConfiguration,
            ));
        }

        let agent_name = unique_agent_name();
        let agent_path = format!(".kiro/agents/{agent_name}.json");

        let mut args: Vec<Argument> = ["chat", "--v3", "--no-interactive", "--agent"]
            .into_iter()
            .map(Argument::literal)
            .collect();
        args.push(Argument::literal(agent_name.as_str()));
        args.push(Argument::literal("--model"));
        args.push(Argument::literal(&self.model));
        args.push(Argument::literal("--output-format"));
        args.push(Argument::literal("stream-json"));
        args.push(Argument::literal("--trust-tools="));

        let user = input.user_prompt;
        let stdin = [user.before_text, input.text, user.after_text].concat();

        Ok(CliInvocation {
            program: ProgramSpec {
                binary: self.binary.clone(),
                npm_entrypoint: None,
            },
            args,
            stdin: stdin.into_bytes(),
            env: BTreeMap::new(),
            remove_env: Vec::new(),
            control_files: Vec::new(),
            workspace_files: vec![ControlFile {
                name: agent_path,
                contents: agent_config(input.system_prompt, &agent_name).into_bytes(),
            }],
            parser: parse_output,
        })
    }
}

/// Workspace-local custom agent configuration that carries the system prompt
/// and denies tools.
fn agent_config(system_prompt: &str, agent_name: &str) -> String {
    serde_json::json!({
        "name": agent_name,
        "description": "Formats dictation without taking actions.",
        "tools": [],
        "excludedTools": ["knowledge"],
        "includeMcpJson": false,
        "includePowers": false,
        "mcpServers": {},
        "hooks": {},
        "prompt": system_prompt,
    })
    .to_string()
}

/// Parses the ACP v2 `stream-json` event stream.
///
/// Success requires exit code 0, at least one `agent_message` or
/// `agent_message_chunk` carrying text, and a final `state_update` with
/// `state: idle` / `stopReason: end_turn` as the last state: text or any
/// other state that arrives after it means the turn did not end cleanly. Any `tool_call_update` or
/// `tool_call_content_chunk` is rejected as unexpected tool activity.
pub fn parse_output(output: &ProcessOutput) -> Result<String, ProviderError> {
    let stdout = super::cli::stdout_text(output)?;

    let mut texts: Vec<String> = Vec::new();
    let mut text_indexes: HashMap<String, usize> = HashMap::new();
    let mut saw_end_turn = false;
    let mut tool_activity = false;
    let mut not_logged_in = false;
    let mut stop_error = false;

    for line in stdout.lines() {
        if line.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
        };

        // The CLI may emit either the raw `params.update` object or the full
        // JSON-RPC notification wrapper.
        let update = if event.get("sessionUpdate").is_some() {
            &event
        } else if let Some(update) = event.pointer("/params/update") {
            update
        } else {
            return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
        };

        let Some(session_update) = update.get("sessionUpdate").and_then(Value::as_str) else {
            return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
        };

        match session_update {
            "agent_message" => {
                saw_end_turn = false;
                let text = text_from_content(update.get("content"))?;
                // Like a chunk, a message without an id cannot be updated
                // later and is kept once.
                match update.get("messageId").and_then(Value::as_str) {
                    Some(message_id) if !message_id.is_empty() => {
                        upsert_text(message_id, text, &mut texts, &mut text_indexes);
                    }
                    _ => texts.push(text),
                }
            }
            "agent_message_chunk" => {
                saw_end_turn = false;
                let message_id = update
                    .get("messageId")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let text = text_from_chunk(update.get("content"))?;
                if message_id.is_empty() {
                    texts.push(text);
                } else {
                    append_text(&message_id, text, &mut texts, &mut text_indexes);
                }
            }
            "tool_call_update" | "tool_call_content_chunk" => {
                tool_activity = true;
            }
            "state_update" => {
                // Only the last state counts: any later state (running,
                // requires_action, another idle) reopens the turn.
                saw_end_turn = false;
                if update.get("state").and_then(Value::as_str) != Some("idle") {
                    continue;
                }
                match update.get("stopReason").and_then(Value::as_str) {
                    Some("end_turn") => saw_end_turn = true,
                    Some("error") => {
                        stop_error = true;
                        if is_auth_error(update.get("error")) {
                            not_logged_in = true;
                        }
                    }
                    Some(_) | None => {}
                }
            }
            _ => {}
        }
    }

    if tool_activity {
        return Err(ProviderError::other(
            ProviderErrorCode::UnexpectedToolActivity,
        ));
    }
    if not_logged_in {
        return Err(ProviderError::NotLoggedIn);
    }
    if stop_error {
        return Err(ProviderError::other(ProviderErrorCode::NonzeroExit));
    }
    if !output.status.success() {
        return Err(ProviderError::other(ProviderErrorCode::NonzeroExit));
    }
    if !saw_end_turn || texts.is_empty() {
        return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
    }
    Ok(texts.concat())
}

fn text_from_content(content: Option<&Value>) -> Result<String, ProviderError> {
    let Some(content) = content else {
        return Ok(String::new());
    };
    if let Some(array) = content.as_array() {
        let mut text = String::new();
        for block in array {
            if block.get("type").and_then(Value::as_str) == Some("text") {
                let Some(part) = block.get("text").and_then(Value::as_str) else {
                    return Err(ProviderError::other(ProviderErrorCode::InvalidOutput));
                };
                text.push_str(part);
            }
        }
        Ok(text)
    } else if content.get("type").and_then(Value::as_str) == Some("text") {
        content
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| ProviderError::other(ProviderErrorCode::InvalidOutput))
    } else {
        Ok(String::new())
    }
}

fn text_from_chunk(content: Option<&Value>) -> Result<String, ProviderError> {
    let Some(content) = content else {
        return Ok(String::new());
    };
    if content.get("type").and_then(Value::as_str) == Some("text") {
        content
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| ProviderError::other(ProviderErrorCode::InvalidOutput))
    } else {
        Ok(String::new())
    }
}

fn upsert_text(
    id: &str,
    text: String,
    texts: &mut Vec<String>,
    indexes: &mut HashMap<String, usize>,
) {
    match indexes.get(id) {
        Some(&index) => texts[index] = text,
        None => {
            indexes.insert(id.to_owned(), texts.len());
            texts.push(text);
        }
    }
}

fn append_text(
    id: &str,
    text: String,
    texts: &mut Vec<String>,
    indexes: &mut HashMap<String, usize>,
) {
    match indexes.get(id) {
        Some(&index) => texts[index].push_str(&text),
        None => {
            indexes.insert(id.to_owned(), texts.len());
            texts.push(text);
        }
    }
}

fn is_auth_error(error: Option<&Value>) -> bool {
    let Some(error) = error else {
        return false;
    };
    if error.get("code").and_then(Value::as_i64) == Some(-32000) {
        return true;
    }
    if let Some(message) = error.get("message").and_then(Value::as_str)
        && message.to_lowercase().contains("authentication")
    {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
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
    fn rejects_all_options() {
        assert!(validate_options(&[]).is_ok());
        assert!(validate_options(&[option("effort", "low")]).is_err());
    }

    #[test]
    fn validates_models() {
        for good in ["claude-sonnet-4.6", "amazon.nova-pro-v1:0"] {
            assert!(is_valid_model(good), "model: {good:?}");
        }
        for bad in [
            "",
            "-model",
            "bad model",
            "bad\tmodel",
            "bad\nmodel",
            "\u{7f}",
        ] {
            assert!(!is_valid_model(bad), "model: {bad:?}");
        }
    }

    #[test]
    fn settings_require_model_when_enabled() {
        let disabled = ProviderSettings {
            enabled: false,
            ..defaults()
        };
        assert!(validate_settings(&disabled, &ProviderLocations::default()).is_ok());

        let enabled_no_model = ProviderSettings {
            enabled: true,
            ..defaults()
        };
        let err = validate_settings(&enabled_no_model, &ProviderLocations::default()).unwrap_err();
        assert!(err.to_string().contains("providers.kiro.model is required"));

        let enabled_bad_model = ProviderSettings {
            enabled: true,
            model: "bad model".to_owned(),
            ..defaults()
        };
        let err = validate_settings(&enabled_bad_model, &ProviderLocations::default()).unwrap_err();
        assert!(err.to_string().contains("providers.kiro.model must be"));
    }

    #[test]
    fn agent_config_carries_prompt_and_empty_tools() {
        let config: Value =
            serde_json::from_str(&agent_config("the prompt", "pumice-abc123")).unwrap();
        assert_eq!(config["name"], Value::from("pumice-abc123"));
        assert_eq!(config["prompt"], Value::from("the prompt"));
        assert_eq!(config["tools"], Value::Array(Vec::new()));
        assert_eq!(config["excludedTools"], Value::from(vec!["knowledge"]));
        assert_eq!(config["includeMcpJson"], Value::from(false));
        assert_eq!(config["includePowers"], Value::from(false));
        assert_eq!(config["mcpServers"], serde_json::json!({}));
        assert_eq!(config["hooks"], serde_json::json!({}));
    }

    fn output_with(stdout: &str, exit_code: i32) -> ProcessOutput {
        use std::process::ExitStatus;

        #[cfg(unix)]
        fn status_from_code(code: i32) -> ExitStatus {
            use std::os::unix::process::ExitStatusExt;
            ExitStatus::from_raw(code)
        }
        #[cfg(windows)]
        fn status_from_code(code: i32) -> ExitStatus {
            use std::os::windows::process::ExitStatusExt;
            ExitStatus::from_raw(code as u32)
        }

        ProcessOutput {
            status: status_from_code(exit_code),
            stdout: stdout.as_bytes().to_vec(),
            stderr_tail: Vec::new(),
        }
    }

    #[test]
    fn parser_returns_agent_message_text() {
        let stdout = concat!(
            r#"{"sessionUpdate":"state_update","state":"running"}"#,
            "\n",
            r#"{"sessionUpdate":"agent_message","messageId":"m1","content":[{"type":"text","text":"Hello, world."}]}"#,
            "\n",
            r#"{"sessionUpdate":"state_update","state":"idle","stopReason":"end_turn"}"#,
            "\n",
        );
        assert_eq!(
            parse_output(&output_with(stdout, 0)).unwrap(),
            "Hello, world."
        );
    }

    #[test]
    fn parser_collects_chunks() {
        let stdout = concat!(
            r#"{"sessionUpdate":"agent_message_chunk","messageId":"m1","content":{"type":"text","text":"One "}}"#,
            "\n",
            r#"{"sessionUpdate":"agent_message_chunk","messageId":"m1","content":{"type":"text","text":"two."}}"#,
            "\n",
            r#"{"sessionUpdate":"state_update","state":"idle","stopReason":"end_turn"}"#,
            "\n",
        );
        assert_eq!(parse_output(&output_with(stdout, 0)).unwrap(), "One two.");
    }

    #[test]
    fn parser_accepts_json_rpc_wrapped_events() {
        let stdout = concat!(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"agent_message","messageId":"m1","content":[{"type":"text","text":"Wrapped"}]}}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"state_update","state":"idle","stopReason":"end_turn"}}}"#,
            "\n",
        );
        assert_eq!(parse_output(&output_with(stdout, 0)).unwrap(), "Wrapped");
    }

    #[test]
    fn parser_rejects_tool_activity() {
        let stdout = concat!(
            r#"{"sessionUpdate":"agent_message","messageId":"m1","content":[{"type":"text","text":"I'll run a tool."}]}"#,
            "\n",
            r#"{"sessionUpdate":"tool_call_update","toolCallId":"c1","status":"pending"}"#,
            "\n",
            r#"{"sessionUpdate":"state_update","state":"idle","stopReason":"end_turn"}"#,
            "\n",
        );
        assert_eq!(
            parse_output(&output_with(stdout, 0)).unwrap_err(),
            ProviderError::other(ProviderErrorCode::UnexpectedToolActivity)
        );
    }

    #[test]
    fn parser_classifies_authentication_error() {
        let stdout = concat!(
            r#"{"sessionUpdate":"state_update","state":"idle","stopReason":"error","error":{"code":-32000,"message":"Authentication required"}}"#,
            "\n",
        );
        assert_eq!(
            parse_output(&output_with(stdout, 1)).unwrap_err(),
            ProviderError::NotLoggedIn
        );
    }

    #[test]
    fn parser_rejects_missing_end_turn() {
        let stdout = concat!(
            r#"{"sessionUpdate":"agent_message","messageId":"m1","content":[{"type":"text","text":"No finish."}]}"#,
            "\n",
        );
        assert_eq!(
            parse_output(&output_with(stdout, 0)).unwrap_err(),
            ProviderError::other(ProviderErrorCode::InvalidOutput)
        );
    }

    #[test]
    fn parser_rejects_nonzero_exit_without_idle() {
        let stdout = concat!(
            r#"{"sessionUpdate":"agent_message","messageId":"m1","content":[{"type":"text","text":"Partial."}]}"#,
            "\n",
        );
        assert_eq!(
            parse_output(&output_with(stdout, 1)).unwrap_err(),
            ProviderError::other(ProviderErrorCode::NonzeroExit)
        );
    }
}
