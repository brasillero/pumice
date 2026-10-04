//! Tests for the formatting pipeline: provider selection, deadlines, the
//! busy guard and the exact raw-text fallback. Everything runs through the
//! portable fake CLI; no real AI CLI is ever invoked.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use pumice::config::{self, Config, DebugLogSettings, PromptSettings};
use pumice::pipeline::{FormatOutcome, OutcomeKind, Pipeline, ProviderErrorKind, RawReason};
use pumice::process::ProcessRunner;
use pumice::providers::{self, ProviderErrorCode};
use pumice::request::{ChatCompletionRequest, Content, ExtractedRequest, Message, extract_request};
use serde_json::{Value, json};
use support::FakeCli;
use tempfile::TempDir;
use tokio::time::Instant;

/// Minimal YAML configuring `claude` with its binary at the fake CLI.
/// `{binary}` is replaced by the fake's path.
const CLAUDE_AT_FAKE: &str = "providers:\n  claude:\n    binary: '{binary}'\n";

/// Builds a pipeline from `yaml` (`{binary}` replaced by the fake's path)
/// together with the fake CLI its provider will run.
fn pipeline(yaml: &str, scenario: Value) -> (Pipeline, FakeCli) {
    let fake = FakeCli::new(scenario);
    let yaml = yaml.replace("{binary}", &fake.path().display().to_string());
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("pumice.yaml");
    fs::write(&path, yaml).expect("write config");
    let config = config::load_with_env(Some(&path), |_| None)
        .expect("config loads")
        .config;
    let runner = Arc::new(ProcessRunner::new());
    let built = providers::build_from_config(&config, runner).expect("provider builds");
    (Pipeline::new(&config, built), fake)
}

/// Extracts `text` wrapped in Handy's envelope, so `raw_text` is `text` byte
/// for byte.
fn handy_request(model: Option<&str>, text: &str) -> ExtractedRequest {
    extract_request(ChatCompletionRequest {
        model: model.map(str::to_owned),
        messages: vec![Message {
            role: "user".to_owned(),
            content: Content::Text(format!("<transcript>\n{text}\n</transcript>")),
        }],
        stream: None,
    })
    .expect("request extracts")
}

/// The stdout of a fake CLI replying with a Claude success envelope holding
/// `result`.
fn success_envelope(result: &str) -> String {
    json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "result": result,
    })
    .to_string()
}

/// A fake CLI replying successfully with `result`.
fn success_scenario(result: &str) -> Value {
    json!({"stdout": success_envelope(result), "exit_code": 0})
}

fn assert_raw(outcome: &FormatOutcome, reason: RawReason, text: &str) {
    assert_eq!(outcome.kind, OutcomeKind::Raw(reason), "kind mismatch");
    assert_eq!(outcome.text, text, "raw text mismatch");
    assert_eq!(outcome.provider, None, "raw outcomes name no provider");
}

#[tokio::test]
async fn formats_cleaned_text_and_reports_the_provider() {
    let (pipeline, fake) = pipeline(
        CLAUDE_AT_FAKE,
        success_scenario("Here is the formatted text:\nOlá mundo."),
    );
    let outcome = pipeline
        .format(&handy_request(Some("claude"), "ola mundo"), Instant::now())
        .await;
    assert_eq!(outcome.kind, OutcomeKind::Formatted);
    assert_eq!(outcome.provider, Some("claude"));
    assert_eq!(outcome.text, "Olá mundo.");
    assert!(fake.report_path().exists(), "the fake ran");
}

#[tokio::test]
async fn empty_transcript_never_invokes_a_cli() {
    let (pipeline, fake) = pipeline(CLAUDE_AT_FAKE, success_scenario("unused"));
    let outcome = pipeline
        .format(&handy_request(None, "  \n\t "), Instant::now())
        .await;
    assert_eq!(outcome.kind, OutcomeKind::Empty);
    assert_eq!(outcome.text, "");
    assert_eq!(outcome.provider, None);
    assert!(!fake.report_path().exists(), "the fake must not run");
}

#[tokio::test]
async fn no_model_selects_the_default_provider() {
    let (pipeline, _fake) = pipeline(CLAUDE_AT_FAKE, success_scenario("Feito."));
    let outcome = pipeline
        .format(&handy_request(None, "dita"), Instant::now())
        .await;
    assert_eq!(outcome.kind, OutcomeKind::Formatted);
    assert_eq!(outcome.provider, Some("claude"));
}

#[tokio::test]
async fn unknown_model_keeps_the_dictation_raw() {
    let (pipeline, fake) = pipeline(CLAUDE_AT_FAKE, success_scenario("unused"));
    let outcome = pipeline
        .format(
            &handy_request(Some("gpt-99"), "olá de novo"),
            Instant::now(),
        )
        .await;
    assert_raw(&outcome, RawReason::UnknownProvider, "olá de novo");
    assert!(!fake.report_path().exists(), "the fake must not run");
}

#[tokio::test]
async fn disabled_provider_keeps_the_dictation_raw() {
    // YAML cannot express this (validation forbids disabling the only
    // registered provider when it is the default), so build the
    // configuration directly: claude is present but disabled.
    let descriptor = providers::descriptor("claude").expect("claude is registered");
    let mut settings = (descriptor.defaults)();
    settings.enabled = false;
    let config = Config {
        port: 7567,
        default_provider: "claude".to_owned(),
        total_timeout: Duration::from_secs(30),
        fallback_order: Vec::new(),
        prompts: PromptSettings::default(),
        debug_log: DebugLogSettings {
            enabled: false,
            path: PathBuf::from("pumice-debug.jsonl"),
        },
        providers: BTreeMap::from([("claude".to_owned(), settings)]),
    };
    let built = providers::build_from_config(&config, Arc::new(ProcessRunner::new()))
        .expect("no enabled provider builds fine");
    assert!(built.is_empty());
    let pipeline = Pipeline::new(&config, built);

    let outcome = pipeline
        .format(&handy_request(Some("claude"), "ditado"), Instant::now())
        .await;
    assert_raw(&outcome, RawReason::ProviderDisabled, "ditado");
}

#[tokio::test]
async fn not_logged_in_keeps_the_dictation_raw() {
    let (pipeline, _fake) = pipeline(
        CLAUDE_AT_FAKE,
        json!({
            "stdout": support::fixture("claude/not-logged-in.json"),
            "exit_code": 1,
        }),
    );
    let outcome = pipeline
        .format(&handy_request(None, "olá"), Instant::now())
        .await;
    assert_raw(
        &outcome,
        RawReason::ProviderFailed(ProviderErrorKind::NotLoggedIn),
        "olá",
    );
}

#[tokio::test]
async fn missing_binary_keeps_the_dictation_raw() {
    // An absolute path that cannot exist, on every OS.
    let dir = TempDir::new().expect("temp dir");
    let missing = dir.path().join("no-such-cli");
    let yaml = format!(
        "providers:\n  claude:\n    binary: '{}'\n",
        missing.display()
    );
    let (pipeline, fake) = pipeline(&yaml, json!({}));
    let outcome = pipeline
        .format(&handy_request(None, "olá"), Instant::now())
        .await;
    assert_raw(
        &outcome,
        RawReason::ProviderFailed(ProviderErrorKind::NotInstalled),
        "olá",
    );
    assert!(!fake.report_path().exists(), "the fake must not run");
}

#[tokio::test]
async fn malformed_output_keeps_the_dictation_raw() {
    let (pipeline, _fake) = pipeline(
        CLAUDE_AT_FAKE,
        json!({"stdout": "not json at all", "exit_code": 0}),
    );
    let outcome = pipeline
        .format(&handy_request(None, "olá"), Instant::now())
        .await;
    assert_raw(
        &outcome,
        RawReason::ProviderFailed(ProviderErrorKind::Other {
            code: ProviderErrorCode::InvalidOutput,
        }),
        "olá",
    );
}

#[tokio::test]
async fn total_timeout_stops_a_slow_cli() {
    let (pipeline, fake) = pipeline(
        "total_timeout_secs: 1\nproviders:\n  claude:\n    binary: '{binary}'\n",
        json!({
            "stdout": success_envelope("too late"),
            "exit_code": 0,
            "sleep_ms": 10_000,
        }),
    );
    let outcome = pipeline
        .format(&handy_request(None, "hello"), Instant::now())
        .await;
    assert_raw(
        &outcome,
        RawReason::ProviderFailed(ProviderErrorKind::Timeout),
        "hello",
    );
    assert!(
        outcome.elapsed < Duration::from_secs(1) + Duration::from_secs(2),
        "took {:?}",
        outcome.elapsed
    );

    // The deadline kills the fake (which would otherwise sleep 10 s).
    let pid = fake.report()["pid"].as_u64().expect("pid") as u32;
    assert!(process_alive(std::process::id()), "liveness check works");
    wait_until_gone(pid);
}

#[tokio::test]
async fn provider_timeout_wins_over_the_total_budget() {
    let (pipeline, _fake) = pipeline(
        "total_timeout_secs: 30\nproviders:\n  claude:\n    binary: '{binary}'\n    timeout_secs: 1\n",
        json!({
            "stdout": success_envelope("too late"),
            "exit_code": 0,
            "sleep_ms": 10_000,
        }),
    );
    let start = Instant::now();
    let outcome = pipeline.format(&handy_request(None, "hello"), start).await;
    assert_raw(
        &outcome,
        RawReason::ProviderFailed(ProviderErrorKind::Timeout),
        "hello",
    );
    // The provider timeout fired, not the 30 s total budget.
    assert!(
        outcome.elapsed < Duration::from_secs(5),
        "took {:?}",
        outcome.elapsed
    );
}

#[tokio::test]
async fn exhausted_budget_never_starts_the_cli() {
    let (pipeline, fake) = pipeline(
        "total_timeout_secs: 2\nproviders:\n  claude:\n    binary: '{binary}'\n",
        success_scenario("unused"),
    );
    // The request arrived long enough ago that its budget is spent.
    let started = Instant::now().checked_sub(Duration::from_secs(5)).unwrap();
    let outcome = pipeline
        .format(&handy_request(None, "ditado"), started)
        .await;
    assert_raw(&outcome, RawReason::BudgetExhausted, "ditado");
    assert!(!fake.report_path().exists(), "the fake must not run");
}

#[tokio::test]
async fn a_second_run_gets_raw_text_while_one_is_active() {
    let (pipeline, fake) = pipeline(
        CLAUDE_AT_FAKE,
        json!({
            "stdout": success_envelope("Olá."),
            "exit_code": 0,
            "sleep_ms": 2_000,
        }),
    );
    let pipeline = Arc::new(pipeline);

    let first = {
        let pipeline = Arc::clone(&pipeline);
        tokio::spawn(async move {
            pipeline
                .format(&handy_request(None, "ola"), Instant::now())
                .await
        })
    };

    // The fake writes its report before sleeping; once it exists the first
    // run surely holds the busy permit.
    let started_by = std::time::Instant::now() + Duration::from_secs(5);
    while !fake.report_path().exists() {
        assert!(std::time::Instant::now() < started_by, "fake never started");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let outcome = pipeline
        .format(&handy_request(None, "outro ditado"), Instant::now())
        .await;
    assert_raw(&outcome, RawReason::Busy, "outro ditado");
    assert!(
        outcome.elapsed < Duration::from_millis(500),
        "busy fallback took {:?}",
        outcome.elapsed
    );

    let first = first.await.expect("first run completes");
    assert_eq!(first.kind, OutcomeKind::Formatted);
    assert_eq!(first.text, "Olá.");

    // After the first run finished, formatting works again.
    let third = pipeline
        .format(&handy_request(None, "terceiro"), Instant::now())
        .await;
    assert_eq!(third.kind, OutcomeKind::Formatted);
    assert_eq!(third.text, "Olá.");
}

#[tokio::test]
async fn cleanup_failure_keeps_the_dictation_raw() {
    // A preamble-only result cleans down to nothing; the raw dictation is
    // returned whole instead.
    let (pipeline, _fake) = pipeline(
        CLAUDE_AT_FAKE,
        success_scenario("Here is the formatted text:"),
    );
    let outcome = pipeline
        .format(&handy_request(None, "algum texto ditado"), Instant::now())
        .await;
    assert_raw(&outcome, RawReason::CleanupFailed, "algum texto ditado");
}

#[tokio::test]
async fn raw_fallback_preserves_the_dictation_exactly() {
    let (pipeline, _fake) = pipeline(
        CLAUDE_AT_FAKE,
        json!({
            "stdout": support::fixture("claude/not-logged-in.json"),
            "exit_code": 1,
        }),
    );
    let raw = "  Olá, Mundo! Ação número três… fim.  ";
    let outcome = pipeline
        .format(&handy_request(None, raw), Instant::now())
        .await;
    assert!(matches!(outcome.kind, OutcomeKind::Raw(_)));
    assert_eq!(
        outcome.text, raw,
        "leading/trailing spaces and accents stay"
    );
}

/// Waits (with a CI tolerance) until process `pid` is no longer running.
fn wait_until_gone(pid: u32) {
    let gone_by = std::time::Instant::now() + Duration::from_secs(5);
    while process_alive(pid) {
        assert!(
            std::time::Instant::now() < gone_by,
            "process {pid} still running"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Whether a process with this PID is still running (zombies count as gone).
/// Mirrors tests/process.rs.
#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("run ps");
    let stat = String::from_utf8_lossy(&output.stdout);
    let stat = stat.trim();
    !stat.is_empty() && !stat.starts_with('Z')
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .expect("run tasklist");
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}
