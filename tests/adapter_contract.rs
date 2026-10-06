//! The shared adapter contract suite (Phase 2, S8.2 follow-up). Each
//! `adapter_contract!` line runs every applicable contract case against one
//! adapter through the portable fake CLI; no real AI CLI is ever invoked. A
//! new adapter plugs in by implementing
//! `support::adapter_contract::ContractAdapter` and adding one line here.

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pumice::process::ProcessRunner;
use pumice::providers::Provider;
use pumice::providers::antigravity::AntigravityAdapter;
use pumice::providers::claude::ClaudeAdapter;
use pumice::providers::cli::CliProvider;
use pumice::providers::codex::CodexAdapter;
use pumice::providers::opencode::OpenCodeAdapter;
use serde_json::{Value, json};
use support::adapter_contract::{
    AFTER, BEFORE, ContractAdapter, HOSTILE_TEXT, SYSTEM_PROMPT, has_pair, report_argv,
};
use support::fixture;

adapter_contract!(claude, ClaudeContract);
adapter_contract!(codex, CodexContract);
adapter_contract!(opencode, OpenCodeContract);
adapter_contract!(antigravity, AntigravityContract);

/// Claude's protocol fixtures and adapter-specific assertions.
struct ClaudeContract;

impl ContractAdapter for ClaudeContract {
    const ID: &'static str = "claude";

    fn provider(fake: &Path, timeout: Duration) -> Arc<dyn Provider> {
        Arc::new(CliProvider::new(
            ClaudeAdapter::new(fake.to_path_buf(), "haiku".to_owned(), BTreeMap::new()),
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
            CodexAdapter::new(fake.to_path_buf(), "gpt-6.1-sol".to_owned(), None),
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

    fn tool_activity() -> Option<(String, i32)> {
        // A completed turn that also completed a tool call must be rejected
        // even with a clean exit; its agent text is never returned.
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

/// OpenCode's protocol fixtures and adapter-specific assertions. The event
/// shapes are documentation-derived from the installed 1.18.34 bundled
/// source (see `tests/fixtures/opencode/README.md`): every event is
/// `{type, timestamp, sessionID, part|error}`; completed text parts carry
/// `time.end`; a failed run prints one structured `error` event and exits
/// nonzero.
struct OpenCodeContract;

impl ContractAdapter for OpenCodeContract {
    const ID: &'static str = "opencode";

    fn provider(fake: &Path, timeout: Duration) -> Arc<dyn Provider> {
        Arc::new(CliProvider::new(
            OpenCodeAdapter::new(fake.to_path_buf(), "opencode/big-pickle".to_owned(), None),
            Arc::new(ProcessRunner::new()),
            timeout,
        ))
    }

    fn success(text: &str) -> (String, i32) {
        // step_start, an ignored reasoning part, one completed text part,
        // step_finish: only the text part may reach the result.
        let events = [
            json!({"type":"step_start","timestamp":1000,"sessionID":"ses_contract","part":{"id":"prt_1","messageID":"msg_1","sessionID":"ses_contract","type":"step-start"}}),
            json!({"type":"reasoning","timestamp":1200,"sessionID":"ses_contract","part":{"id":"prt_r","messageID":"msg_1","sessionID":"ses_contract","type":"reasoning","text":"thinking about formatting","time":{"start":1050,"end":1200}}}),
            json!({"type":"text","timestamp":1500,"sessionID":"ses_contract","part":{"id":"prt_2","messageID":"msg_1","sessionID":"ses_contract","type":"text","text":text,"time":{"start":1100,"end":1500}}}),
            json!({"type":"step_finish","timestamp":1600,"sessionID":"ses_contract","part":{"id":"prt_3","messageID":"msg_1","sessionID":"ses_contract","type":"step-finish","reason":"stop","cost":0,"tokens":{"input":1,"output":1,"reasoning":0,"cache":{"read":0,"write":0}}}}),
        ];
        let stream = events
            .into_iter()
            .map(|event| event.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        (format!("{stream}\n"), 0)
    }

    fn not_logged_in(marker: &str) -> (String, i32) {
        // The structured missing-authentication error; the marker rides in
        // the diagnostic and must never leak into the classified error.
        let event = json!({"type":"error","timestamp":1200,"sessionID":"ses_contract","error":{"name":"ProviderAuthError","data":{"providerID":"opencode","message":format!("missing authentication ({marker})")}}});
        (format!("{event}\n"), 1)
    }

    fn invalid_outputs() -> Vec<(String, i32)> {
        vec![
            ("not json\n".to_owned(), 0),
            ("{}\n".to_owned(), 0),
            ("[1,2]\n".to_owned(), 0),
            (String::new(), 0),
            // A text part without the final step_finish is incomplete.
            (
                concat!(
                    r#"{"type":"text","timestamp":1,"sessionID":"ses_contract","part":{"id":"prt_1","messageID":"m","sessionID":"ses_contract","type":"text","text":"Partial.","time":{"start":1,"end":2}}}"#,
                    "\n",
                )
                .to_owned(),
                0,
            ),
            // A step_finish without any text part produced no answer.
            (
                concat!(
                    r#"{"type":"step_finish","timestamp":1,"sessionID":"ses_contract","part":{"id":"prt_1","messageID":"m","sessionID":"ses_contract","type":"step-finish","reason":"stop","cost":0,"tokens":{"input":1,"output":1,"reasoning":0,"cache":{"read":0,"write":0}}}}"#,
                    "\n",
                )
                .to_owned(),
                0,
            ),
            // Two events from different sessions are not one run.
            (
                concat!(
                    r#"{"type":"step_start","timestamp":1,"sessionID":"ses_one","part":{"id":"prt_1","messageID":"m","sessionID":"ses_one","type":"step-start"}}"#,
                    "\n",
                    r#"{"type":"text","timestamp":2,"sessionID":"ses_two","part":{"id":"prt_2","messageID":"m","sessionID":"ses_two","type":"text","text":"Mixed.","time":{"start":1,"end":2}}}"#,
                    "\n",
                    r#"{"type":"step_finish","timestamp":3,"sessionID":"ses_one","part":{"id":"prt_3","messageID":"m","sessionID":"ses_one","type":"step-finish","reason":"stop","cost":0,"tokens":{"input":1,"output":1,"reasoning":0,"cache":{"read":0,"write":0}}}}"#,
                    "\n",
                )
                .to_owned(),
                0,
            ),
            // A truncated (length) finish reason means no complete text.
            (
                concat!(
                    r#"{"type":"text","timestamp":1,"sessionID":"ses_contract","part":{"id":"prt_1","messageID":"m","sessionID":"ses_contract","type":"text","text":"Partial","time":{"start":1,"end":2}}}"#,
                    "\n",
                    r#"{"type":"step_finish","timestamp":2,"sessionID":"ses_contract","part":{"id":"prt_2","messageID":"m","sessionID":"ses_contract","type":"step-finish","reason":"length","cost":0,"tokens":{"input":1,"output":1,"reasoning":0,"cache":{"read":0,"write":0}}}}"#,
                    "\n",
                )
                .to_owned(),
                0,
            ),
        ]
    }

    fn tool_activity() -> Option<(String, i32)> {
        // A completed tool call prints a tool_use event even before the
        // step_finish; a clean exit must not rescue the run.
        Some((
            concat!(
                r#"{"type":"step_start","timestamp":1,"sessionID":"ses_contract","part":{"id":"prt_1","messageID":"m","sessionID":"ses_contract","type":"step-start"}}"#,
                "\n",
                r#"{"type":"tool_use","timestamp":2,"sessionID":"ses_contract","part":{"id":"prt_2","messageID":"m","sessionID":"ses_contract","type":"tool","callID":"call_1","tool":"read","state":{"status":"completed"}}}"#,
                "\n",
                r#"{"type":"text","timestamp":3,"sessionID":"ses_contract","part":{"id":"prt_3","messageID":"m","sessionID":"ses_contract","type":"text","text":"Tool output.","time":{"start":2,"end":3}}}"#,
                "\n",
                r#"{"type":"step_finish","timestamp":4,"sessionID":"ses_contract","part":{"id":"prt_4","messageID":"m","sessionID":"ses_contract","type":"step-finish","reason":"tool-calls","cost":0,"tokens":{"input":1,"output":1,"reasoning":0,"cache":{"read":0,"write":0}}}}"#,
                "\n",
            )
            .to_owned(),
            0,
        ))
    }

    fn capture_system_prompt(scenario: &mut Value) {
        scenario["report_env"] = json!(["OPENCODE_CONFIG_CONTENT"]);
    }

    fn assert_system_separation(report: &Value) {
        // The system prompt travels in OPENCODE_CONFIG_CONTENT, never in
        // stdin: the user message alone reaches stdin and the decoded inline
        // config carries the prompt exactly.
        let stdin = report["stdin"].as_str().unwrap();
        assert_eq!(stdin, format!("{BEFORE}{HOSTILE_TEXT}{AFTER}"));
        assert!(
            !stdin.contains(SYSTEM_PROMPT),
            "the system prompt must not reach stdin"
        );
        assert_eq!(inline_agent_prompt(report), SYSTEM_PROMPT);
    }

    fn assert_system_prompt_placement(report: &Value) {
        // No control file exists: the inline config env var is the system
        // prompt's only channel, and it holds the prompt exactly.
        assert_eq!(inline_agent_prompt(report), SYSTEM_PROMPT);
    }

    fn assert_restricted_argv(argv: &[String]) {
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        assert_eq!(argv.first(), Some(&"run"));
        assert!(argv.contains(&"--pure"), "missing --pure");
        assert!(has_pair(&argv, "--agent", "pumice"));
        assert!(has_pair(&argv, "--format", "json"));
        assert!(has_pair(&argv, "--title", "Pumice"));
        assert!(has_pair(&argv, "--model", "opencode/big-pickle"));
        // Dictated text and the system prompt never travel in argv.
        assert!(!argv.contains(&"--variant"));
    }
}

/// Decodes the inline configuration carried in `OPENCODE_CONFIG_CONTENT`,
/// returning the `pumice` agent's prompt from it.
fn inline_agent_prompt(report: &Value) -> String {
    let raw = report["env"]["OPENCODE_CONFIG_CONTENT"]
        .as_str()
        .expect("inline configuration env var reported");
    let config: Value = serde_json::from_str(raw).expect("inline configuration is JSON");
    config["agent"]["pumice"]["prompt"]
        .as_str()
        .expect("pumice agent prompt in inline configuration")
        .to_owned()
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

/// Antigravity's protocol fixtures and adapter-specific assertions.
///
/// Everything here is documentation-derived (S0.2's "public documentation
/// only" section) and the provider is built through the test-only
/// constructor; `agy` is never executed, and the protocol coverage below
/// does not establish safe execution. Production enablement stays refused
/// until a supported per-launch tool policy exists (phase2-architecture.md
/// §3.2), which is also why `assert_restricted_argv` can only assert the
/// documented `--sandbox`/fixed-effort recipe.
struct AntigravityContract;

impl ContractAdapter for AntigravityContract {
    const ID: &'static str = "antigravity";

    fn provider(fake: &Path, timeout: Duration) -> Arc<dyn Provider> {
        Arc::new(CliProvider::new(
            AntigravityAdapter::new(fake.to_path_buf(), "test-model".to_owned(), timeout),
            Arc::new(ProcessRunner::new()),
            timeout,
        ))
    }

    fn success(text: &str) -> (String, i32) {
        // Documentation-derived stream: initialization, ignored text deltas,
        // reasoning and checkpoint noise, then the single terminal result.
        let records = [
            json!({"event": "init"}),
            json!({"event": "message", "type": "model", "content": "partial "}),
            json!({"event": "reasoning", "content": "thinking about formatting"}),
            json!({"event": "message", "type": "model", "content": "noise"}),
            json!({"event": "checkpoint", "id": "checkpoint-1"}),
            json!({"event": "result", "status": "SUCCESS", "result": {"response": text, "usage": {"input_tokens": 12, "output_tokens": 4}}}),
        ];
        (ndjson(&records), 0)
    }

    fn not_logged_in(marker: &str) -> (String, i32) {
        // The documented "authentication required" failure evidence; the
        // marker rides along in the diagnostic text and is never copied out.
        let records = [
            json!({"event": "init"}),
            json!({"event": "result", "status": "FAILURE", "result": {"error": format!("authentication required: no cached login ({marker})")}}),
        ];
        (ndjson(&records), 1)
    }

    fn invalid_outputs() -> Vec<(String, i32)> {
        let success = Self::success("x").0;
        vec![
            ("not json\n".to_owned(), 0),
            (String::new(), 0),
            ("\n".to_owned(), 0),
            // A valid result, but the initialization record is missing.
            (
                ndjson(&[
                    json!({"event": "result", "status": "SUCCESS", "result": {"response": "x"}}),
                ]),
                0,
            ),
            // Two contradictory terminal results.
            (
                ndjson(&[
                    json!({"event": "init"}),
                    json!({"event": "result", "status": "SUCCESS", "result": {"response": "first"}}),
                    json!({"event": "result", "status": "SUCCESS", "result": {"response": "second"}}),
                ]),
                0,
            ),
            // SUCCESS without a string response.
            (
                ndjson(&[
                    json!({"event": "init"}),
                    json!({"event": "result", "status": "SUCCESS", "result": {"response": 42}}),
                ]),
                0,
            ),
            // A record without the protocol's "event" key.
            (
                ndjson(&[json!({"status": "SUCCESS", "result": {"response": "x"}})]),
                0,
            ),
            // A truncated stream dies mid-line.
            (success.chars().take(40).collect::<String>(), 0),
        ]
    }

    fn tool_activity() -> Option<(String, i32)> {
        // A run that invokes a tool is rejected even when it completes.
        Some((fixture("antigravity/tool.ndjson"), 0))
    }

    fn assert_restricted_argv(argv: &[String]) {
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        // The documented recipe is `--sandbox` plus the fixed low-effort
        // invocation; there is no per-launch tool policy to assert (that
        // absence is why the adapter is dormant), so this asserts exactly
        // the recipe and nothing stronger.
        assert!(argv.contains(&"--sandbox"), "missing --sandbox");
        assert!(has_pair(&argv, "--input-format", "stream-json"));
        assert!(has_pair(&argv, "--output-format", "stream-json"));
        assert!(has_pair(&argv, "--model", "test-model"));
        assert!(has_pair(&argv, "--effort", "low"));
    }

    fn assert_stdin_payload(report: &Value, text: &str) {
        // Merged-input fallback (E3.3): one newline-terminated `user` event
        // carries "<system prompt>\n\n<user message>"; the dictation is
        // byte-exact inside the JSON string.
        let content = merged_stdin_event(report);
        assert_eq!(
            content,
            format!("{SYSTEM_PROMPT}\n\n{BEFORE}{text}{AFTER}"),
            "dictation travels byte-exact inside the stdin event"
        );
    }

    fn assert_system_separation(report: &Value) {
        // No system-prompt flag or file exists (documentation-derived: the
        // headless transport has none), so the merged event is the only
        // channel: it holds the system prompt and no argv element names a
        // file or carries the prompt.
        let content = merged_stdin_event(report);
        assert_eq!(
            content,
            format!("{SYSTEM_PROMPT}\n\n{BEFORE}{HOSTILE_TEXT}{AFTER}")
        );
        for arg in report_argv(report) {
            assert!(
                !arg.contains(SYSTEM_PROMPT),
                "argv element carries the system prompt"
            );
            assert!(
                !arg.contains(['/', '\\']),
                "argv element names a file: {arg}"
            );
        }
    }

    fn assert_system_prompt_placement(report: &Value) {
        // The system prompt's only channel is stdin; with no control file,
        // no argv element may name one (a path would contain a separator).
        for arg in report_argv(report) {
            assert!(
                !arg.contains(['/', '\\']),
                "argv element names a file: {arg}"
            );
        }
    }
}

/// Decodes the single newline-terminated stream-JSON `user` event the
/// adapter writes to stdin, returning its `message.content`.
fn merged_stdin_event(report: &Value) -> String {
    let stdin = report["stdin"].as_str().expect("stdin in report");
    let line = stdin.strip_suffix('\n').expect("newline-terminated event");
    assert!(!line.contains('\n'), "the event is a single line");
    let event: Value = serde_json::from_str(line).expect("the event is valid JSON");
    assert_eq!(event["event"], json!("user"));
    event["message"]["content"]
        .as_str()
        .expect("string content")
        .to_owned()
}
