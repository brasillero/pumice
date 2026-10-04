//! The shared adapter contract suite (Phase 2, S8.2 follow-up), run for the
//! two existing adapters. Each `adapter_contract!` line runs every
//! applicable contract case against one adapter through the portable fake
//! CLI; no real AI CLI is ever invoked. A new adapter plugs in by
//! implementing `support::adapter_contract::ContractAdapter` and adding one
//! line here.

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pumice::process::ProcessRunner;
use pumice::providers::Provider;
use pumice::providers::claude::ClaudeAdapter;
use pumice::providers::cli::CliProvider;
use pumice::providers::codex::CodexAdapter;
use serde_json::{Value, json};
use support::adapter_contract::{ContractAdapter, has_pair};
use support::fixture;

adapter_contract!(claude, ClaudeContract);
adapter_contract!(codex, CodexContract);

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
            "--restricted",
            "--disable-slash-commands",
            "--no-session-persistence",
        ] {
            assert!(argv.contains(&flag), "missing {flag}");
        }
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
