//! The shared adapter contract suite (Phase 2, S8.2 follow-up). Each
//! `adapter_contract!` line runs every applicable contract case against one
//! adapter through the portable fake CLI; no real AI CLI is ever invoked. A
//! new adapter plugs in by implementing
//! `support::adapter_contract::ContractAdapter` and adding one line here.

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pumice::process::ProcessRunner;
use pumice::providers::Provider;
use pumice::providers::antigravity::AntigravityAdapter;
use pumice::providers::claude::ClaudeAdapter;
use pumice::providers::cli::CliProvider;
use pumice::providers::codex::CodexAdapter;
use pumice::providers::kimi::KimiAdapter;
use serde_json::{Value, json};
use support::adapter_contract::{
    AFTER, BEFORE, ContractAdapter, HOSTILE_TEXT, SYSTEM_PROMPT, has_pair, report_argv,
};
use support::fixture;

adapter_contract!(claude, ClaudeContract);
adapter_contract!(codex, CodexContract);
adapter_contract!(antigravity, AntigravityContract);

/// Kimi's contract cases. The shared `transport` case drives a 200 KiB
/// dictation, which the argv transport rejects by design with
/// `InputTooLarge`; every other shared case runs unchanged, and the
/// transport expectations live in `kimi::transport` below.
mod kimi {
    use std::time::Duration;

    use pumice::providers::kimi::MAX_USER_MESSAGE_BYTES;
    use pumice::providers::{ProviderError, ProviderErrorCode};
    use support::adapter_contract::{UNICODE_TEXT, format_with};

    use super::*;

    /// A fake CLI replying successfully with the agent file captured,
    /// mirroring the suite's private `success_fake` helper.
    fn fake_cli() -> support::FakeCli {
        let (stdout, exit_code) = KimiContract::success("Texto formatado.");
        let mut scenario = json!({"stdout": stdout, "exit_code": exit_code});
        KimiContract::capture_system_prompt(&mut scenario);
        support::FakeCli::new(scenario)
    }

    #[tokio::test]
    async fn final_text_only() {
        support::adapter_contract::final_text_only::<KimiContract>().await;
    }

    #[tokio::test]
    async fn restricted() {
        support::adapter_contract::restricted::<KimiContract>().await;
    }

    #[tokio::test]
    async fn empty_workspace() {
        support::adapter_contract::empty_workspace::<KimiContract>().await;
    }

    #[tokio::test]
    async fn system_separation() {
        support::adapter_contract::system_separation::<KimiContract>().await;
    }

    #[tokio::test]
    async fn not_installed() {
        support::adapter_contract::not_installed::<KimiContract>().await;
    }

    #[tokio::test]
    async fn not_logged_in() {
        support::adapter_contract::not_logged_in::<KimiContract>().await;
    }

    #[tokio::test]
    async fn timeout() {
        support::adapter_contract::timeout::<KimiContract>().await;
    }

    #[tokio::test]
    async fn invalid_output() {
        support::adapter_contract::invalid_output::<KimiContract>().await;
    }

    #[tokio::test]
    async fn tool_activity() {
        support::adapter_contract::tool_activity::<KimiContract>().await;
    }

    #[tokio::test]
    async fn privacy() {
        support::adapter_contract::privacy::<KimiContract>().await;
    }

    #[tokio::test]
    async fn failure_is_terminal() {
        support::adapter_contract::failure_is_terminal::<KimiContract>().await;
    }

    /// Kimi's transport: the prompt is one `-p` argv element (print mode
    /// has no stdin transport), so the byte-exactness and no-argv-default
    /// expectations move to the `-p` payload, and an oversized dictation
    /// fails with `InputTooLarge` before anything is spawned.
    #[tokio::test]
    async fn transport() {
        for text in [HOSTILE_TEXT, UNICODE_TEXT] {
            let fake = fake_cli();
            let provider = KimiContract::provider(fake.path(), Duration::from_secs(30));

            format_with(&provider, text).await.unwrap();
            let report = fake.report();

            // The composed user message is exactly one `-p` argv element;
            // no other element carries the dictation, and stdin is unused.
            let argv = report_argv(&report);
            let payload = argv
                .windows(2)
                .find(|pair| pair[0] == "-p")
                .map(|pair| pair[1].clone())
                .expect("the -p payload");
            assert_eq!(payload, format!("{BEFORE}{text}{AFTER}"));
            for arg in &argv {
                if *arg != payload {
                    assert!(
                        !arg.contains(text),
                        "argv element carries dictation: {}",
                        arg.chars().take(80).collect::<String>()
                    );
                }
            }
            assert_eq!(report["stdin"], json!(""), "print mode reads no stdin");
            // The system prompt never reaches argv; it lives in the agent
            // file, outside the workspace, with tools disabled.
            let file = &report["arg_files"]["--agent-file"];
            let agent_path = PathBuf::from(file["path"].as_str().unwrap());
            assert!(agent_path.is_absolute());
            assert!(!agent_path.starts_with(report["cwd"].as_str().unwrap()));
            let contents = file["contents"].as_str().unwrap();
            assert!(
                contents.contains("subagents: []\n---\n"),
                "the agent file must close the front matter and disable tools: {contents}"
            );
            assert!(
                contents.contains("subagents: []\n"),
                "the agent file must disable delegation: {contents}"
            );
        }

        // An oversized dictation is refused before spawn: a clean
        // `InputTooLarge`, no platform argv limit involved, no run at all.
        let oversize = "x".repeat(MAX_USER_MESSAGE_BYTES + 1);
        let fake = fake_cli();
        let provider = KimiContract::provider(fake.path(), Duration::from_secs(30));
        assert_eq!(
            format_with(&provider, &oversize).await.unwrap_err(),
            ProviderError::other(ProviderErrorCode::InputTooLarge)
        );
        assert!(
            !fake.report_path().exists(),
            "an oversized dictation must never spawn the CLI"
        );

        // The largest accepted dictation still formats.
        let room = MAX_USER_MESSAGE_BYTES - BEFORE.len() - AFTER.len();
        let at_limit = "y".repeat(room);
        let fake = fake_cli();
        let provider = KimiContract::provider(fake.path(), Duration::from_secs(30));
        assert_eq!(
            format_with(&provider, &at_limit).await.unwrap(),
            "Texto formatado."
        );
    }
}

/// Claude's protocol fixtures and adapter-specific assertions.
struct ClaudeContract;

impl ContractAdapter for ClaudeContract {
    const ID: &'static str = "claude";

    fn provider(fake: &Path, timeout: Duration) -> Arc<dyn Provider> {
        Arc::new(CliProvider::new(
            ClaudeAdapter::new(fake.to_path_buf(), "haiku".to_owned()),
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
            CodexAdapter::new(fake.to_path_buf(), "gpt-6.1-sol".to_owned()),
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

/// Kimi's protocol fixtures and adapter-specific assertions. The stream
/// vocabulary (system.version / assistant / tool / turn.step.retrying /
/// session.resume_hint) is verified against the installed 2.1.1 in S2.4
/// with real calls (docs/research/S2.4-kimi-adapter.md). The dictation
/// travels as one `-p` argv element — print mode has no stdin transport —
/// so every default stdin expectation is overridden here.
struct KimiContract;

impl ContractAdapter for KimiContract {
    const ID: &'static str = "kimi";

    fn provider(fake: &Path, timeout: Duration) -> Arc<dyn Provider> {
        Arc::new(CliProvider::new(
            KimiAdapter::new(fake.to_path_buf(), "kimi-k2.7-code-highspeed".to_owned()),
            Arc::new(ProcessRunner::new()),
            timeout,
        ))
    }

    fn success(text: &str) -> (String, i32) {
        // The verified stream: version meta, the assistant message, the
        // resume hint. Only the assistant content is the result; the
        // session id and resume command never leak into it.
        let records = [
            json!({"role":"meta","type":"system.version","version":"2.1.1"}),
            json!({"role":"assistant","content":text}),
            json!({"role":"meta","type":"session.resume_hint","session_id":"session_00000000-0000-4000-8000-000000000000","command":"kimi -r session_00000000-0000-4000-8000-000000000000","content":"To resume this session: kimi -r session_00000000-0000-4000-8000-000000000000"}),
        ];
        (ndjson(&records), 0)
    }

    fn not_logged_in(marker: &str) -> (String, i32) {
        // The retrying meta message's status_code is the protocol's own
        // authentication evidence; the marker rides in the diagnostic text
        // and is never copied into the classified error.
        let records = [
            json!({"role":"meta","type":"system.version","version":"2.1.1"}),
            json!({"role":"meta","type":"turn.step.retrying","failed_attempt":1,"next_attempt":2,"max_attempts":3,"delay_ms":1000,"error_name":"UnauthorizedError","error_message":format!("missing bearer authentication ({marker})"),"status_code":401}),
        ];
        (ndjson(&records), 1)
    }

    fn invalid_outputs() -> Vec<(String, i32)> {
        let version = json!({"role":"meta","type":"system.version","version":"2.1.1"}).to_string();
        vec![
            ("not json\n".to_owned(), 0),
            (String::new(), 0),
            ("\n".to_owned(), 0),
            ("[1,2]\n".to_owned(), 0),
            ("{}\n".to_owned(), 0),
            // An assistant message without the version header is not a run.
            (
                format!("{}\n", json!({"role":"assistant","content":"x"})),
                0,
            ),
            // A run that produced no assistant text is incomplete.
            (format!("{version}\n{}\n", json!({"role":"assistant"})), 0),
            // Non-string assistant content.
            (
                format!("{version}\n{}\n", json!({"role":"assistant","content":42})),
                0,
            ),
            // Unknown roles are outside the verified vocabulary.
            (
                format!("{version}\n{}\n", json!({"role":"user","content":"x"})),
                0,
            ),
        ]
    }

    fn tool_activity_answer() -> Option<&'static str> {
        Some("Let me list the files first.")
    }

    fn tool_activity() -> Option<(String, i32)> {
        // The verified tool sequence: an assistant message with tool_calls
        // followed by the tool result. Tool records are ignored; the
        // assistant text is the answer.
        Some((fixture("kimi/tool.jsonl"), 0))
    }

    fn capture_system_prompt(scenario: &mut Value) {
        scenario["report_arg_files"] = json!(["--agent-file"]);
    }

    fn captured_system_prompt(report: &Value) -> (PathBuf, String) {
        let file = &report["arg_files"]["--agent-file"];
        (
            PathBuf::from(file["path"].as_str().unwrap()),
            file["contents"].as_str().unwrap().to_owned(),
        )
    }

    fn assert_system_separation(report: &Value) {
        // The system prompt travels only in the agent file; the `-p`
        // payload holds the user message alone and no argv element carries
        // the system prompt.
        let argv = report_argv(report);
        let payload = argv
            .windows(2)
            .find(|pair| pair[0] == "-p")
            .map(|pair| pair[1].clone())
            .expect("the -p payload");
        assert_eq!(payload, format!("{BEFORE}{HOSTILE_TEXT}{AFTER}"));
        for arg in &argv {
            assert!(
                !arg.contains(SYSTEM_PROMPT),
                "argv element carries the system prompt"
            );
        }
        let (_, contents) = Self::captured_system_prompt(report);
        assert!(
            contents.contains(SYSTEM_PROMPT),
            "the agent file carries the system prompt"
        );
        assert!(
            contents.contains("subagents: []\n---\n"),
            "the agent file must close the front matter and disable tools"
        );
    }

    fn assert_system_prompt_placement(report: &Value) {
        // The agent file is the system prompt's channel: a control file
        // outside the call's cwd, with the prompt in its body.
        let cwd = Path::new(report["cwd"].as_str().unwrap());
        let (path, contents) = Self::captured_system_prompt(report);
        assert!(contents.contains(SYSTEM_PROMPT));
        assert!(path.is_absolute(), "{}", path.display());
        assert!(!path.starts_with(cwd));
    }

    fn assert_restricted_argv(argv: &[String]) {
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        assert!(has_pair(&argv, "--skills-dir", "."));
        assert!(has_pair(&argv, "--output-format", "stream-json"));
        assert!(has_pair(&argv, "--model", "kimi-k2.7-code-highspeed"));
        assert!(argv.contains(&"-p"), "missing -p");
        // The agent file is passed by absolute path after --agent-file.
        let agent = argv
            .windows(2)
            .find(|pair| pair[0] == "--agent-file")
            .map(|pair| pair[1])
            .expect("the agent file path");
        assert!(
            agent.ends_with("pumice-agent.md"),
            "unexpected agent file: {agent}"
        );
        // Print mode auto-approves tools, so no flag may weaken the recipe:
        // --yolo/--auto/--plan are rejected with --prompt upstream, resuming
        // a session or adding directories would leave the fresh empty
        // workspace, and hidden aliases must not smuggle a second model.
        for forbidden in [
            "--yolo",
            "--auto",
            "--plan",
            "--session",
            "--continue",
            "-S",
            "-c",
            "-m",
            "--agent",
            "--add-dir",
        ] {
            assert!(!argv.contains(&forbidden), "forbidden flag {forbidden}");
        }
    }
}
