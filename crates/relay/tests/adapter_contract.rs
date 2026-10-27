//! The shared adapter contract suite (Phase 2, S8.2 follow-up). Each
//! `adapter_contract!` line runs every applicable contract case against one
//! adapter through the portable fake CLI; no real AI CLI is ever invoked. A
//! new adapter plugs in by implementing
//! `support::adapter_contract::ContractAdapter` and adding one line here.

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pumice_relay::process::ProcessRunner;
use pumice_relay::providers::Effort;
use pumice_relay::providers::Provider;
use pumice_relay::providers::claude::ClaudeAdapter;
use pumice_relay::providers::cli::CliProvider;
use pumice_relay::providers::codex::CodexAdapter;
use pumice_relay::providers::kiro::KiroAdapter;
use serde_json::{Value, json};
use support::adapter_contract::{ContractAdapter, SYSTEM_PROMPT, has_pair};
use support::fixture;

adapter_contract!(claude, ClaudeContract);
adapter_contract!(codex, CodexContract);
adapter_contract!(kiro, KiroContract);

/// Claude's protocol fixtures and adapter-specific assertions.
struct ClaudeContract;

impl ContractAdapter for ClaudeContract {
    const ID: &'static str = "claude";

    fn provider(fake: &Path, timeout: Duration) -> Arc<dyn Provider> {
        Arc::new(CliProvider::new(
            ClaudeAdapter::new(fake.to_path_buf(), "haiku".to_owned(), Effort::Low, false),
            Arc::new(ProcessRunner::new()),
            timeout,
        ))
    }

    fn success(text: &str) -> (String, i32) {
        // The verified result envelope; usage, cost and duration metadata
        // must never leak into the parsed text.
        let envelope = json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "duration_ms": 1854,
            "num_turns": 1,
            "result": text,
            "session_id": "00000000-0000-4000-8000-000000000001",
            "total_cost_usd": 0.0012,
            "usage": {
                "input_tokens": 412,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0,
                "output_tokens": 5,
            },
        });
        (envelope.to_string(), 0)
    }

    fn not_logged_in(marker: &str) -> (String, i32) {
        // `is_error: true` with the verified "Not logged in" result text;
        // the marker rides along in the diagnostic.
        let envelope = json!({
            "type": "result",
            "subtype": "success",
            "is_error": true,
            "result": format!("Not logged in · Please run /login ({marker})"),
        });
        (envelope.to_string(), 1)
    }

    fn invalid_outputs() -> Vec<(String, i32)> {
        let truncated = fixture("claude/success.json")[..40].to_owned();
        vec![
            ("not json".to_owned(), 0),
            (truncated, 0),
            (String::new(), 0),
            ("[]".to_owned(), 0),
            (r#"{"type":"result","is_error":false}"#.to_owned(), 0),
            (
                r#"{"type":"result","is_error":false,"result":42}"#.to_owned(),
                0,
            ),
            (
                r#"{"type":"result","result":"no is_error field"}"#.to_owned(),
                0,
            ),
            (
                r#"{"type":"assistant","is_error":false,"result":"wrong type"}"#.to_owned(),
                0,
            ),
            // A nonzero exit without any JSON is a failure, not parseable text.
            (String::new(), 2),
        ]
    }

    fn tool_activity() -> Option<(String, i32)> {
        // The single-result protocol cannot express tool activity; tools are
        // disabled in argv instead (asserted by `assert_restricted_argv`).
        None
    }

    fn capture_system_prompt(scenario: &mut Value) {
        scenario["report_arg_files"] = json!(["--system-prompt-file"]);
    }

    fn captured_system_prompt(report: &Value) -> (PathBuf, String) {
        let file = &report["arg_files"]["--system-prompt-file"];
        (
            PathBuf::from(file["path"].as_str().unwrap()),
            file["contents"].as_str().unwrap().to_owned(),
        )
    }

    fn assert_restricted_argv(argv: &[String]) {
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        assert!(has_pair(&argv, "--tools", ""));
        for flag in [
            "--strict-mcp-config",
            "--safe-mode",
            "--disable-slash-commands",
            "--no-session-persistence",
        ] {
            assert!(argv.contains(&flag), "missing {flag}");
        }
        // `--restricted` ignores the user's settings files, dropping a
        // gateway configured in their `env` block (2026-10-06 regression).
        assert!(
            !argv.contains(&"--restricted"),
            "--restricted must not return"
        );
    }
}

/// Codex's protocol fixtures and adapter-specific assertions.
struct CodexContract;

impl ContractAdapter for CodexContract {
    const ID: &'static str = "codex";

    fn provider(fake: &Path, timeout: Duration) -> Arc<dyn Provider> {
        Arc::new(CliProvider::new(
            CodexAdapter::new(fake.to_path_buf(), "gpt-6.1-sol".to_owned(), Effort::Low),
            Arc::new(ProcessRunner::new()),
            timeout,
        ))
    }

    fn success(text: &str) -> (String, i32) {
        // The verified event stream; reasoning and usage noise must never
        // leak into the final agent message.
        let events = [
            json!({"type":"thread.started","thread_id":"00000000-0000-4000-8000-000000000003"}),
            json!({"type":"turn.started"}),
            json!({"type":"item.completed","item":{"id":"1","type":"reasoning","text":"thinking about formatting"}}),
            json!({"type":"item.completed","item":{"id":"2","type":"agent_message","text":text}}),
            json!({"type":"turn.completed","usage":{"input_tokens":5310,"output_tokens":6}}),
        ];
        let stream = events
            .into_iter()
            .map(|event| event.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        (format!("{stream}\n"), 0)
    }

    fn not_logged_in(marker: &str) -> (String, i32) {
        // The verified unauthorized stream; the 401 evidence classifies the
        // failure and the marker rides along in the diagnostic.
        let events = [
            json!({"type":"thread.started","thread_id":"00000000-0000-4000-8000-000000000004"}),
            json!({"type":"turn.started"}),
            json!({"type":"error","message":format!("unexpected status 401 Unauthorized: Missing bearer or basic authentication in header ({marker})")}),
            json!({"type":"turn.failed","error":{"message":format!("unexpected status 401 Unauthorized: Missing bearer or basic authentication in header ({marker})")}}),
        ];
        let stream = events
            .into_iter()
            .map(|event| event.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        (format!("{stream}\n"), 1)
    }

    fn invalid_outputs() -> Vec<(String, i32)> {
        vec![
            ("not json\n".to_owned(), 0),
            ("{\"type\":\"turn.completed\"}\noops\n".to_owned(), 0),
            ("\n".to_owned(), 0),
            ("[1,2]\n".to_owned(), 0),
            (
                concat!(
                    r#"{"type":"thread.started","thread_id":"t"}"#,
                    "\n",
                    r#"{"type":"item.completed","item":{"id":"1","type":"agent_message","text":"Partial."}}"#,
                    "\n",
                )
                .to_owned(),
                0,
            ),
            ("{\"type\":\"turn.completed\"}\n".to_owned(), 0),
        ]
    }

    fn tool_activity_answer() -> Option<&'static str> {
        Some("Tool output.")
    }

    fn tool_activity() -> Option<(String, i32)> {
        // A completed turn that also completed a tool call: the tool item is
        // ignored and the final agent message is the answer.
        Some((
            concat!(
                r#"{"type":"item.completed","item":{"id":"1","type":"command_execution","command":"ls -la","status":"completed"}}"#,
                "\n",
                r#"{"type":"item.completed","item":{"id":"2","type":"agent_message","text":"Tool output."}}"#,
                "\n",
                r#"{"type":"turn.completed"}"#,
                "\n",
            )
            .to_owned(),
            0,
        ))
    }

    fn capture_system_prompt(scenario: &mut Value) {
        scenario["report_config_files"] = json!(["model_instructions_file"]);
    }

    fn captured_system_prompt(report: &Value) -> (PathBuf, String) {
        let file = &report["config_files"]["model_instructions_file"];
        (
            PathBuf::from(file["path"].as_str().unwrap()),
            file["contents"].as_str().unwrap().to_owned(),
        )
    }

    fn assert_restricted_argv(argv: &[String]) {
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        assert!(has_pair(&argv, "--sandbox", "read-only"));
        for flag in ["--ephemeral", "--ignore-user-config", "--ignore-rules"] {
            assert!(argv.contains(&flag), "missing {flag}");
        }
        assert!(argv.contains(&r#"web_search="disabled""#));
        for feature in [
            "shell_tool",
            "unified_exec",
            "multi_agent",
            "hooks",
            "view_image",
            "remote_plugin",
        ] {
            assert!(
                has_pair(&argv, "-c", &format!("features.{feature}=false")),
                "missing features.{feature}=false"
            );
        }
    }
}

/// Joins records into one newline-terminated NDJSON stream.
fn ndjson(records: &[Value]) -> String {
    let stream = records
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    format!("{stream}\n")
}

/// Kiro's protocol fixtures and adapter-specific assertions. The stream
/// vocabulary (runStarted / metadata / sessionUpdate / runFinished / runError)
/// is verified against the installed 2.29.0 in S2.14 with real calls
/// (docs/research/S2.14-kiro-plugin.md). The system prompt travels in a
/// workspace-local agent file — the approved exception to the outside-cwd
/// rule — so placement assertions override the default.
struct KiroContract;

impl ContractAdapter for KiroContract {
    const ID: &'static str = "kiro";

    fn provider(fake: &Path, timeout: Duration) -> Arc<dyn Provider> {
        Arc::new(CliProvider::new(
            KiroAdapter::new(
                fake.to_path_buf(),
                "claude-haiku-4.5".to_owned(),
                Effort::Low,
            ),
            Arc::new(ProcessRunner::new()),
            timeout,
        ))
    }

    fn success(text: &str) -> (String, i32) {
        let (first, rest) = split_at_char(text);
        let mut records: Vec<Value> = vec![
            serde_json::from_str(r#"{"type":"runStarted","data":{"payloadSchema":"acp","acpProtocolVersion":1,"engine":"v2"}}"#).unwrap(),
            serde_json::from_str(r#"{"type":"metadata","data":{"sessionId":"00000000-0000-4000-8000-000000000001","contextUsagePercentage":0.0,"reasoning":{"support":"unavailable","effortLevels":[]}}}"#).unwrap(),
            serde_json::from_str(r#"{"type":"sessionUpdate","data":{"sessionId":"00000000-0000-4000-8000-000000000001","update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"Thinking."}}}}"#).unwrap(),
        ];
        if !first.is_empty() {
            records.push(json!({"type":"sessionUpdate","data":{"sessionId":"00000000-0000-4000-8000-000000000001","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":first}}}}));
        }
        if !rest.is_empty() {
            records.push(json!({"type":"sessionUpdate","data":{"sessionId":"00000000-0000-4000-8000-000000000001","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":rest}}}}));
        }
        records.push(serde_json::from_str(r#"{"type":"metadata","data":{"sessionId":"00000000-0000-4000-8000-000000000001","contextUsagePercentage":0.0,"meteringUsage":[{"value":0.001,"unit":"credit","unitPlural":"credits"}],"reasoning":{"support":"unavailable","effortLevels":[]}}}"#).unwrap());
        records.push(json!({"type":"runFinished","data":{"sessionId":"00000000-0000-4000-8000-000000000001","status":"success","stopReason":"end_turn","finalText":text,"finalTextTruncated":false}}));
        (ndjson(&records), 0)
    }

    fn not_logged_in(marker: &str) -> (String, i32) {
        (
            format!(
                "\nConfirm the following code in the browser\nCode: ABCD-EFGH\n\nOpen this URL: https://example.invalid/device?code={marker}\n"
            ),
            1,
        )
    }

    fn invalid_outputs() -> Vec<(String, i32)> {
        let success = Self::success("x").0;
        vec![
            ("not json\n".to_owned(), 0),
            (String::new(), 0),
            ("\n".to_owned(), 0),
            // A run that never started.
            (
                ndjson(&[
                    json!({"type":"runFinished","data":{"status":"success","finalText":"x","finalTextTruncated":false}}),
                ]),
                0,
            ),
            // A successful finish without any answer text.
            (
                ndjson(&[
                    json!({"type":"runStarted","data":{"payloadSchema":"acp","acpProtocolVersion":1,"engine":"v2"}}),
                    json!({"type":"runFinished","data":{"status":"success","finalText":"","finalTextTruncated":false}}),
                ]),
                0,
            ),
            // Non-string agent message content.
            (
                ndjson(&[
                    json!({"type":"runStarted","data":{"payloadSchema":"acp","acpProtocolVersion":1,"engine":"v2"}}),
                    json!({"type":"sessionUpdate","data":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":42}}}}),
                    json!({"type":"runFinished","data":{"status":"success","finalText":"x","finalTextTruncated":false}}),
                ]),
                0,
            ),
            // A line without a type field.
            ("{\"data\":{}}\n".to_owned(), 0),
            // Truncated mid-line.
            (success.chars().take(40).collect::<String>(), 0),
        ]
    }

    fn tool_activity_answer() -> Option<&'static str> {
        Some("Texto formatado.")
    }

    fn tool_activity() -> Option<(String, i32)> {
        Some((fixture("kiro/tool.jsonl"), 0))
    }

    fn capture_system_prompt(scenario: &mut Value) {
        scenario["report_files"] = json!([".kiro/agents/pumice.json"]);
    }

    fn expected_workspace_entries() -> Vec<&'static str> {
        vec![".kiro"]
    }

    fn captured_system_prompt(report: &Value) -> (PathBuf, String) {
        let files = report["report_files"]
            .as_object()
            .expect("report_files captured");
        let (path, entry) = files
            .iter()
            .find(|(k, _)| k.replace('\\', "/").ends_with(".kiro/agents/pumice.json"))
            .expect("agent file reported");
        let file = entry.as_object().expect("agent file present");
        (
            PathBuf::from(file["path"].as_str().unwrap_or(path)),
            file["contents"]
                .as_str()
                .and_then(|c| serde_json::from_str::<Value>(c).ok())
                .and_then(|v| v["prompt"].as_str().map(str::to_owned))
                .expect("agent file prompt"),
        )
    }

    fn assert_system_prompt_placement(report: &Value) {
        let cwd = Path::new(report["cwd"].as_str().unwrap());
        let (path, contents) = Self::captured_system_prompt(report);
        assert_eq!(contents, SYSTEM_PROMPT);
        assert!(path.is_absolute(), "{}", path.display());
        assert!(
            path_inside_cwd(&path, cwd),
            "agent file must be inside the cwd: {} vs {}",
            path.display(),
            cwd.display()
        );
        assert!(
            normalize_path(&path).ends_with(".kiro/agents/pumice.json"),
            "unexpected agent file: {}",
            path.display()
        );
    }

    fn assert_restricted_argv(argv: &[String]) {
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        assert!(has_pair(&argv, "--agent-engine", "v2"));
        assert!(has_pair(&argv, "--output-format", "stream-json"));
        assert!(has_pair(&argv, "--agent", "pumice"));
        assert!(has_pair(&argv, "--model", "claude-haiku-4.5"));
        assert!(has_pair(&argv, "--effort", "low"));
        assert!(
            argv.contains(&"--no-interactive"),
            "missing --no-interactive"
        );
        assert!(argv.contains(&"--trust-tools="), "missing --trust-tools=");
        for forbidden in [
            "-a",
            "--trust-all-tools",
            "--resume",
            "-r",
            "--resume-id",
            "--v3",
            "--tui",
            "--cloud",
            "--require-mcp-startup",
        ] {
            assert!(!argv.contains(&forbidden), "forbidden flag {forbidden}");
        }
    }
}

/// Splits `text` after its first UTF-8 character, returning ("", text) when
/// empty and (text, "") when it has only one character.
fn split_at_char(text: &str) -> (&str, &str) {
    match text.chars().next() {
        None => ("", text),
        Some(c) => text.split_at(c.len_utf8()),
    }
}

/// True when `path` is inside `cwd`, robustly on Windows (canonicalized `\\?\`
/// prefixes, case and separator differences).
fn path_inside_cwd(path: &Path, cwd: &Path) -> bool {
    let path = normalize_path(path);
    let cwd = normalize_path(cwd);
    let prefix = format!("{cwd}/");
    path.starts_with(&prefix)
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy()
        .strip_prefix(r"\\?\")
        .map(String::from)
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
        .replace('\\', "/")
        .to_lowercase()
}
