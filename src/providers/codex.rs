//! Codex adapter (`codex exec`).
//!
//! The invocation and the JSONL event stream were verified in S0.2 (Codex
//! 0.160.0): `--json` emits one event per line on stdout, the formatted text
//! is the last completed `agent_message` item, and a failed turn ends in
//! `turn.failed` after a handful of retried `error` events.
//!
//! Codex reads the user's own `~/.codex/config.toml` (or `$CODEX_HOME`), so a
//! gateway, provider or profile configured there applies without repeating
//! it in Pumice (owner decision, 2026-10-05). What that file could add to the
//! tool surface is switched off per call: every MCP server it names is
//! disabled by name (Pumice reads only the table names, never values), and
//! plugins, apps and `notify` are off. When the file exists but cannot be
//! read or parsed, or names a server that cannot be addressed safely, the
//! call falls back to `--ignore-user-config` (fail closed).
//!
//! Routing (for example a gateway's `openai_base_url`) comes only from the
//! user's own Codex configuration; Pumice adds none.

use std::collections::BTreeMap;
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
use crate::process::{
    Argument, CliInvocation, ControlFile, ProcessOutput, ProcessRunner, ProgramSpec,
};

pub const ID: &str = "codex";
pub const DEFAULT_BINARY: &str = "codex";
/// Provider default timeout, measured in Phase 0.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);



/// npm package entrypoint the Windows `.cmd` shim translates to (layout from
/// npm; verify on a real Windows install (owner check)).
const NPM_ENTRYPOINT: &str = "@openai/codex/bin/codex.js";

pub const DESCRIPTOR: ProviderDescriptor = ProviderDescriptor {
    id: ID,
    defaults,
    build,
    validate_settings: validate_settings_noop,
    probe: ProbeSpec::Version(&["--version"]),
    default_binary: DEFAULT_BINARY,
    npm_entrypoint: Some(NPM_ENTRYPOINT),
    install_hint: "npm install -g @openai/codex",
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
    let adapter = CodexAdapter::new(binary, settings.model.clone());
    let adapter = match codex_home() {
        Some(home) => adapter.with_codex_home(home),
        None => adapter,
    };
    Ok(Arc::new(CliProvider::new(
        adapter,
        runner,
        settings.timeout,
    )))
}

/// Builds restricted `codex exec` calls.
#[derive(Clone, Debug)]
pub struct CodexAdapter {
    binary: PathBuf,
    model: String,
    /// Codex's home directory, whose `config.toml` the call inherits. `None`
    /// keeps `--ignore-user-config`.
    codex_home: Option<PathBuf>,
}

impl CodexAdapter {
    /// `binary` is a command name looked up on PATH (normally `codex`) or a
    /// path to the official CLI.
    pub fn new(binary: PathBuf, model: String) -> CodexAdapter {
        CodexAdapter {
            binary,
            model,
            codex_home: None,
        }
    }

    /// Lets calls inherit `<codex_home>/config.toml`.
    pub fn with_codex_home(mut self, codex_home: PathBuf) -> CodexAdapter {
        self.codex_home = Some(codex_home);
        self
    }

    /// How this call treats the user's config, read fresh for every call so
    /// edits apply without a restart.
    fn user_config(&self) -> UserConfig {
        let Some(home) = &self.codex_home else {
            return UserConfig::Ignore;
        };
        match std::fs::read_to_string(home.join("config.toml")) {
            Ok(text) => inherit_plan(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                UserConfig::Inherit { mcp_servers: Vec::new() }
            }
            Err(_) => UserConfig::Ignore,
        }
    }
}

/// Whether a call inherits the user's Codex config.
#[derive(Debug, PartialEq, Eq)]
enum UserConfig {
    /// Inherit it, disabling these MCP servers by name.
    Inherit { mcp_servers: Vec<String> },
    /// Pass `--ignore-user-config`.
    Ignore,
}

/// Reads only the names under `[mcp_servers]`. Anything unexpected falls
/// back to ignoring the file.
fn inherit_plan(text: &str) -> UserConfig {
    let Ok(table) = text.parse::<toml::Table>() else {
        return UserConfig::Ignore;
    };
    let mcp_servers = match table.get("mcp_servers") {
        None => Vec::new(),
        Some(toml::Value::Table(servers)) => servers.keys().cloned().collect(),
        Some(_) => return UserConfig::Ignore,
    };
    if !mcp_servers.iter().all(|name| is_bare_key(name)) {
        return UserConfig::Ignore;
    }
    UserConfig::Inherit { mcp_servers }
}

/// A TOML bare key, safe to place unquoted in a `-c` key path.
fn is_bare_key(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// `$CODEX_HOME`, else `~/.codex`, as Codex itself resolves it.
fn codex_home() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("CODEX_HOME").filter(|home| !home.is_empty()) {
        return Some(PathBuf::from(home));
    }
    std::env::home_dir().map(|home| home.join(".codex"))
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

        let user_config = self.user_config();
        let mut args: Vec<Argument> = ["--no-daemon", "exec"]
            .into_iter()
            .map(Argument::literal)
            .collect();
        if user_config == UserConfig::Ignore {
            args.push(Argument::literal("--ignore-user-config"));
        }
        args.extend(
            [
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
        .map(Argument::literal),
        );

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
            // Plugins and apps can bring their own tools; `notify` runs a
            // program after the turn. None of them belongs in a format call.
            "features.plugins=false",
            "features.apps=false",
            "notify=[]",
        ] {
            args.push(Argument::literal("-c"));
            args.push(Argument::literal(value));
        }
        // `-c mcp_servers={}` merges instead of clearing, so each inherited
        // server is disabled by name.
        if let UserConfig::Inherit { mcp_servers } = &user_config {
            for name in mcp_servers {
                args.push(Argument::literal("-c"));
                args.push(Argument::literal(format!("mcp_servers.{name}.enabled=false")));
            }
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
                name: "system.txt".to_owned(),
                contents: input.system_prompt.as_bytes().to_vec(),
            }],
            workspace_files: Vec::new(),
            parser: parse_output,
        })
    }
}

/// Parses Codex's `--json` JSONL event stream (one event per line).
///
/// Success requires a `turn.completed` event and exit code 0; the result is
/// the text of the last completed `agent_message` item. Every other item
/// (reasoning, warnings, tool or activity items) is ignored: Pumice does not
/// police tool use in the output (owner decision 2026-10-08); tools are
/// switched off in the invocation instead. Failure events carry
/// CLI diagnostics, never dictation, so they are the only text that is
/// classified; an agent message is returned only from a run that completed
/// cleanly, and reasoning or progress text is never concatenated into it.
pub fn parse_output(output: &ProcessOutput) -> Result<String, ProviderError> {
    let stdout = super::cli::stdout_text(output)?;

    let mut last_agent_message: Option<String> = None;
    let mut saw_turn_completed = false;
    let mut saw_turn_failed = false;
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
                    // Only agent output carries the answer.
                    Some("agent_message") if event_type == "item.completed" => {
                        let text = event
                            .pointer("/item/text")
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                ProviderError::other(ProviderErrorCode::InvalidOutput)
                            })?;
                        last_agent_message = Some(text.to_owned());
                    }
                    // Reasoning, warnings (`error` items) and any other
                    // item carry no answer text.
                    Some(_) => {}
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
    // Quota exhaustion usually arrives as HTTP 429 too, so it is checked
    // before the generic rate limit. OpenAI's wording is "You exceeded your
    // current quota".
    if any_contains(&["usage limit"])
        || any_contains(&["quota exceeded"])
        || any_contains(&["exceeded your current quota"])
        || any_contains(&["insufficient_quota"])
    {
        return ProviderError::QuotaExceeded { retry_after: None };
    }
    if any_contains(&["429"]) || any_contains(&["rate limit"]) {
        return ProviderError::RateLimited { retry_after: None };
    }
    ProviderError::other(ProviderErrorCode::NonzeroExit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn literal_args(adapter: &CodexAdapter) -> Vec<String> {
        let input = FormatInput {
            system_prompt: "system",
            user_prompt: Default::default(),
            text: "oi",
        };
        adapter
            .invocation(input)
            .expect("invocation builds")
            .args
            .into_iter()
            .filter_map(|argument| match argument {
                Argument::Literal(value) => Some(value.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect()
    }

    fn adapter_with_config(config: Option<&str>) -> (tempfile::TempDir, CodexAdapter) {
        let home = tempfile::tempdir().expect("temp dir");
        if let Some(config) = config {
            std::fs::write(home.path().join("config.toml"), config).expect("config written");
        }
        let adapter = CodexAdapter::new(PathBuf::from("codex"), "gpt-6.1-sol".to_owned())
            .with_codex_home(home.path().to_path_buf());
        (home, adapter)
    }

    #[test]
    fn inherits_user_config_and_disables_its_mcp_servers() {
        let (_home, adapter) = adapter_with_config(Some(
            "openai_base_url = \"http://gateway:8317/v1\"\n\
             [mcp_servers.files]\ncommand = \"secret-cmd\"\n\
             [mcp_servers.web-search]\ncommand = \"x\"\n",
        ));
        let args = literal_args(&adapter);
        assert!(!args.contains(&"--ignore-user-config".to_owned()), "{args:?}");
        for wanted in [
            "mcp_servers.files.enabled=false",
            "mcp_servers.web-search.enabled=false",
            "features.plugins=false",
            "features.apps=false",
            "notify=[]",
        ] {
            assert!(args.contains(&wanted.to_owned()), "missing {wanted}: {args:?}");
        }
        assert!(
            !args.iter().any(|arg| arg.contains("secret-cmd") || arg.contains("gateway")),
            "config values are never copied: {args:?}"
        );
    }

    #[test]
    fn missing_config_inherits_nothing_to_disable() {
        let (_home, adapter) = adapter_with_config(None);
        let args = literal_args(&adapter);
        assert!(!args.contains(&"--ignore-user-config".to_owned()), "{args:?}");
        assert!(!args.iter().any(|arg| arg.starts_with("mcp_servers.")), "{args:?}");
    }

    #[test]
    fn unreadable_or_odd_config_fails_closed() {
        for config in [
            "this is = = not toml",
            "mcp_servers = 3",
            "[mcp_servers.\"has space\"]\ncommand = \"x\"\n",
        ] {
            let (_home, adapter) = adapter_with_config(Some(config));
            let args = literal_args(&adapter);
            assert!(
                args.contains(&"--ignore-user-config".to_owned()),
                "{config:?} must fall back: {args:?}"
            );
        }
    }

    #[test]
    fn without_a_codex_home_the_user_config_is_ignored() {
        let adapter = CodexAdapter::new(PathBuf::from("codex"), "gpt-6.1-sol".to_owned());
        assert!(literal_args(&adapter).contains(&"--ignore-user-config".to_owned()));
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
            classify_failure(&[
                "unexpected status 429 Too Many Requests: You exceeded your current quota"
            ]),
            ProviderError::QuotaExceeded { retry_after: None }
        );
        assert_eq!(
            classify_failure(&["429: insufficient_quota"]),
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
