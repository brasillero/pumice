//! Tests for the formatting pipeline: provider selection, deadlines, the
//! busy guard and the exact raw-text guarantee. There is no fallback: one
//! provider runs per request, and every failure returns the dictation
//! exactly as extracted. Everything runs through the portable fake CLI; no
//! real AI CLI is ever invoked.

mod support;

use pumice::cleanup::CleanupError;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use pumice::config::{self, Config, DebugLogSettings, ProviderConfig};
use pumice::pipeline::{FormatOutcome, OutcomeKind, Pipeline, ProviderErrorKind, RawReason};
use pumice::process::ProcessRunner;
use pumice::providers::{self, Provider, ProviderError, ProviderErrorCode, ProviderSettings};
use pumice::request::{ChatCompletionRequest, Content, ExtractedRequest, Message, extract_request};
use serde_json::{Value, json};
use support::FakeCli;
use support::test_provider::{Step, TestProvider};
use tempfile::TempDir;
use tokio::time::Instant;

/// Minimal YAML configuring `claude` at the fake CLI. `{binary}` is replaced
/// by the fake's path.
const CLAUDE_AT_FAKE: &str =
    "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: '{binary}'\n";

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

/// Asserts a raw outcome carries the exact dictation and no provider.
fn assert_raw(outcome: &FormatOutcome, reason: RawReason, text: &str) {
    assert_eq!(outcome.kind, OutcomeKind::Raw(reason), "kind mismatch");
    assert_eq!(outcome.text, text, "raw text mismatch");
    assert_eq!(outcome.provider, None, "raw outcomes name no provider");
}

/// Claude settings under a test-only id, for directly built configs (YAML
/// only knows registered provider ids). Enabled settings carry a model, as
/// the configuration file requires.
fn test_settings(enabled: bool) -> ProviderSettings {
    let descriptor = providers::descriptor("claude").expect("claude is registered");
    let mut settings = (descriptor.defaults)();
    settings.enabled = enabled;
    if enabled {
        settings.model = "test-model".to_owned();
    }
    settings
}

/// Settings pointing the real Claude adapter at a fake CLI.
fn claude_settings_at(fake: &FakeCli) -> ProviderSettings {
    let mut settings = test_settings(true);
    settings.binary = Some(fake.path().to_path_buf());
    settings
}

fn build_claude(settings: &ProviderSettings) -> Arc<dyn Provider> {
    let descriptor = providers::descriptor("claude").expect("claude is registered");
    (descriptor.build)(settings, Arc::new(ProcessRunner::new())).expect("claude builds")
}

/// Builds a [`Config`] directly, bypassing YAML validation. `entries` lists
/// every configured provider; which of them are actually built is decided
/// by the caller when constructing the pipeline, so tests can leave
/// disabled providers unbuilt.
fn direct_config(total_timeout: Duration, entries: Vec<(&str, ProviderSettings)>) -> Config {
    Config {
        port: 7567,
        total_timeout,
        debug_log: DebugLogSettings {
            enabled: false,
            path: PathBuf::from("pumice-debug.jsonl"),
        },
        providers: entries
            .into_iter()
            .map(|(id, settings)| ProviderConfig {
                id: id.to_owned(),
                settings,
            })
            .collect(),
    }
}

/// A provider's settings inside a directly built config.
fn settings_of<'a>(config: &'a Config, id: &str) -> &'a ProviderSettings {
    config.provider(id).expect("provider is configured")
}

#[tokio::test]
async fn formats_cleaned_text_and_reports_the_provider() {
    let (pipeline, fake) = pipeline(
        CLAUDE_AT_FAKE,
        success_scenario("<think>plan</think>\n  Olá mundo.\n"),
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
async fn empty_transcript_returns_the_original_whitespace_and_runs_no_cli() {
    let (pipeline, fake) = pipeline(CLAUDE_AT_FAKE, success_scenario("unused"));
    let outcome = pipeline
        .format(&handy_request(Some("claude"), "  \n\t "), Instant::now())
        .await;
    assert_eq!(outcome.kind, OutcomeKind::Empty);
    assert_eq!(outcome.text, "  \n\t ", "original text, byte for byte");
    assert_eq!(outcome.provider, None);
    assert!(!fake.report_path().exists(), "the fake must not run");
}

#[tokio::test]
async fn empty_transcript_still_reports_a_selection_problem() {
    let (pipeline, fake) = pipeline(CLAUDE_AT_FAKE, success_scenario("unused"));
    let outcome = pipeline
        .format(&handy_request(None, "  "), Instant::now())
        .await;
    assert_raw(&outcome, RawReason::NoModel, "  ");
    let outcome = pipeline
        .format(&handy_request(Some("nope"), "  "), Instant::now())
        .await;
    assert_raw(&outcome, RawReason::UnknownProvider, "  ");
    assert!(!fake.report_path().exists(), "the fake must not run");
}

#[tokio::test]
async fn no_model_returns_raw_text_and_runs_nothing() {
    let (pipeline, fake) = pipeline(CLAUDE_AT_FAKE, success_scenario("unused"));
    for model in [None, Some("")] {
        let outcome = pipeline
            .format(&handy_request(model, "dita"), Instant::now())
            .await;
        assert_raw(&outcome, RawReason::NoModel, "dita");
    }
    assert!(
        !fake.report_path().exists(),
        "no provider runs for a model-less request"
    );
}

#[tokio::test]
async fn model_selects_the_named_provider_regardless_of_list_order() {
    // codex is listed first, claude second; naming claude still runs claude.
    let yaml = "providers:\n  - id: codex\n    enabled: true\n    model: gpt-6.1-sol\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: '{binary}'\n";
    let (pipeline, fake) = pipeline(yaml, success_scenario("Feito."));
    let outcome = pipeline
        .format(&handy_request(Some("claude"), "dita"), Instant::now())
        .await;
    assert_eq!(outcome.kind, OutcomeKind::Formatted);
    assert_eq!(outcome.provider, Some("claude"));
    assert_eq!(outcome.text, "Feito.");
    assert!(fake.report_path().exists(), "the fake ran");
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
    // claude is present but disabled: requesting it returns raw text.
    let config = direct_config(
        Duration::from_secs(30),
        vec![("claude", test_settings(false))],
    );
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
async fn removed_default_key_fails_the_config() {
    // Through real YAML: `default:` no longer loads with a warning; it is a
    // removed key and fails at its own line:column.
    let fake = FakeCli::new(success_scenario("unused"));
    let yaml = "default: claude\nproviders:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: '{binary}'\n"
        .replace("{binary}", &fake.path().display().to_string());
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("pumice.yaml");
    fs::write(&path, yaml).expect("write config");
    let error = config::load_with_env(Some(&path), |_| None)
        .expect_err("the removed key fails the config")
        .to_string();
    assert!(error.contains("\"default\" was removed in 0.2:"), "{error}");
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
        .format(&handy_request(Some("claude"), "olá"), Instant::now())
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
        "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: '{}'\n",
        missing.display()
    );
    let (pipeline, fake) = pipeline(&yaml, json!({}));
    let outcome = pipeline
        .format(&handy_request(Some("claude"), "olá"), Instant::now())
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
        .format(&handy_request(Some("claude"), "olá"), Instant::now())
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
        "total_timeout_secs: 1\nproviders:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: '{binary}'\n",
        json!({
            "stdout": success_envelope("too late"),
            "exit_code": 0,
            "sleep_ms": 10_000,
        }),
    );
    let outcome = pipeline
        .format(&handy_request(Some("claude"), "hello"), Instant::now())
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
        "total_timeout_secs: 30\nproviders:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: '{binary}'\n    timeout_secs: 1\n",
        json!({
            "stdout": success_envelope("too late"),
            "exit_code": 0,
            "sleep_ms": 10_000,
        }),
    );
    let start = Instant::now();
    let outcome = pipeline
        .format(&handy_request(Some("claude"), "hello"), start)
        .await;
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
        "total_timeout_secs: 2\nproviders:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: '{binary}'\n",
        success_scenario("unused"),
    );
    // The request arrived long enough ago that its budget is spent.
    let started = Instant::now().checked_sub(Duration::from_secs(5)).unwrap();
    let outcome = pipeline
        .format(&handy_request(Some("claude"), "ditado"), started)
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
                .format(&handy_request(Some("claude"), "ola"), Instant::now())
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
        .format(
            &handy_request(Some("claude"), "outro ditado"),
            Instant::now(),
        )
        .await;
    assert_raw(&outcome, RawReason::Busy, "outro ditado");
    assert!(
        outcome.elapsed < Duration::from_millis(500),
        "busy fallback took {:?}",
        outcome.elapsed
    );

    let passthrough = pipeline
        .format(
            &handy_request(Some("passthrough"), "  raw\ntext  "),
            Instant::now(),
        )
        .await;
    assert_eq!(passthrough.kind, OutcomeKind::Passthrough);
    assert_eq!(passthrough.text, "  raw\ntext  ");
    assert_eq!(passthrough.attempts, 0);

    let first = first.await.expect("first run completes");
    assert_eq!(first.kind, OutcomeKind::Formatted);
    assert_eq!(first.text, "Olá.");

    // After the first run finished, formatting works again.
    let third = pipeline
        .format(&handy_request(Some("claude"), "terceiro"), Instant::now())
        .await;
    assert_eq!(third.kind, OutcomeKind::Formatted);
    assert_eq!(third.text, "Olá.");
}

#[tokio::test]
async fn cleanup_failure_keeps_the_dictation_raw() {
    // A reasoning-only result cleans down to nothing; the raw dictation is
    // returned whole instead, and the reason says why.
    let (pipeline, _fake) = pipeline(
        CLAUDE_AT_FAKE,
        success_scenario("<think>just thinking</think>"),
    );
    let outcome = pipeline
        .format(
            &handy_request(Some("claude"), "algum texto ditado"),
            Instant::now(),
        )
        .await;
    assert_raw(
        &outcome,
        RawReason::CleanupFailed(CleanupError::Empty),
        "algum texto ditado",
    );
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
        .format(&handy_request(Some("claude"), raw), Instant::now())
        .await;
    assert!(matches!(outcome.kind, OutcomeKind::Raw(_)));
    assert_eq!(
        outcome.text, raw,
        "leading/trailing spaces and accents stay"
    );
}

#[tokio::test]
async fn failure_never_runs_a_second_provider() {
    // The owner's no-fallback rule: when the selected provider fails, the
    // original text comes back and no other provider is tried.
    let fake = FakeCli::new(json!({
        "stdout": support::fixture("claude/not-logged-in.json"),
        "exit_code": 1,
    }));
    let backup = TestProvider::new("backup", vec![Step::Ready("unused".to_owned())]);
    let config = direct_config(
        Duration::from_secs(30),
        vec![
            ("claude", claude_settings_at(&fake)),
            ("backup", test_settings(true)),
        ],
    );
    let providers: Vec<Arc<dyn Provider>> =
        vec![build_claude(settings_of(&config, "claude")), backup.clone()];
    let pipeline = Pipeline::new(&config, providers);

    let outcome = pipeline
        .format(&handy_request(Some("claude"), "ola mundo"), Instant::now())
        .await;
    assert_raw(
        &outcome,
        RawReason::ProviderFailed(ProviderErrorKind::NotLoggedIn),
        "ola mundo",
    );
    assert_eq!(outcome.attempts, 1);
    assert!(fake.report_path().exists(), "the fake ran once");
    assert_eq!(backup.calls(), 0, "a failure never tries another provider");
}

#[tokio::test]
async fn another_enabled_provider_never_runs_when_the_selected_succeeds() {
    let selected = TestProvider::new("alpha", vec![Step::Ready("Texto pronto.".to_owned())]);
    let backup = TestProvider::new("backup", vec![Step::Ready("unused".to_owned())]);
    let config = direct_config(
        Duration::from_secs(30),
        vec![
            ("alpha", test_settings(true)),
            ("backup", test_settings(true)),
        ],
    );
    let providers: Vec<Arc<dyn Provider>> = vec![selected.clone(), backup.clone()];
    let pipeline = Pipeline::new(&config, providers);

    let outcome = pipeline
        .format(&handy_request(Some("alpha"), "dita"), Instant::now())
        .await;
    assert_eq!(outcome.kind, OutcomeKind::Formatted);
    assert_eq!(outcome.provider, Some("alpha"));
    assert_eq!(outcome.attempts, 1);
    assert_eq!(selected.calls(), 1);
    assert_eq!(backup.calls(), 0, "the other provider must not run");
}

#[tokio::test]
async fn requested_provider_failure_never_runs_other_enabled_providers() {
    // A request naming a provider that fails returns raw text; other
    // enabled providers are never tried.
    let alpha = TestProvider::new("alpha", vec![Step::Fail(ProviderError::NotLoggedIn)]);
    let beta = TestProvider::new("beta", vec![Step::Ready("unused".to_owned())]);
    let config = direct_config(
        Duration::from_secs(30),
        vec![
            ("alpha", test_settings(true)),
            ("beta", test_settings(true)),
        ],
    );
    let providers: Vec<Arc<dyn Provider>> = vec![alpha.clone(), beta.clone()];
    let pipeline = Pipeline::new(&config, providers);

    let outcome = pipeline
        .format(
            &handy_request(Some("alpha"), "texto ditado"),
            Instant::now(),
        )
        .await;
    assert_raw(
        &outcome,
        RawReason::ProviderFailed(ProviderErrorKind::NotLoggedIn),
        "texto ditado",
    );
    assert_eq!(outcome.attempts, 1);
    assert_eq!(alpha.calls(), 1);
    assert_eq!(beta.calls(), 0);
}

#[tokio::test]
async fn cleanup_failure_never_runs_other_enabled_providers() {
    // An unclosed reasoning block fails cleanup; the raw dictation comes
    // back and no other provider runs.
    let fake = FakeCli::new(success_scenario("<think>never closed"));
    let backup = TestProvider::new("backup", vec![Step::Ready("unused".to_owned())]);
    let config = direct_config(
        Duration::from_secs(30),
        vec![
            ("claude", claude_settings_at(&fake)),
            ("backup", test_settings(true)),
        ],
    );
    let providers: Vec<Arc<dyn Provider>> =
        vec![build_claude(settings_of(&config, "claude")), backup.clone()];
    let pipeline = Pipeline::new(&config, providers);

    let outcome = pipeline
        .format(&handy_request(Some("claude"), "ola mundo"), Instant::now())
        .await;
    assert_raw(
        &outcome,
        RawReason::CleanupFailed(CleanupError::UnclosedReasoning),
        "ola mundo",
    );
    assert_eq!(outcome.attempts, 1);
    assert!(fake.report_path().exists(), "the fake ran once");
    assert_eq!(backup.calls(), 0);
}

#[tokio::test]
async fn selected_timeout_returns_raw_within_the_total_budget() {
    // One-second provider timeout inside a five-second total budget: the
    // timeout fires and the raw text returns quickly; the other enabled
    // provider is never spawned.
    let mut alpha_settings = test_settings(true);
    alpha_settings.timeout = Duration::from_secs(1);
    let alpha = TestProvider::new("alpha", vec![Step::Sleep(Duration::from_secs(10))]);
    let beta = TestProvider::new("beta", vec![Step::Ready("unused".to_owned())]);
    let config = direct_config(
        Duration::from_secs(5),
        vec![("alpha", alpha_settings), ("beta", test_settings(true))],
    );
    let providers: Vec<Arc<dyn Provider>> = vec![alpha.clone(), beta.clone()];
    let pipeline = Pipeline::new(&config, providers);

    let outcome = pipeline
        .format(&handy_request(Some("alpha"), "ola"), Instant::now())
        .await;
    assert_raw(
        &outcome,
        RawReason::ProviderFailed(ProviderErrorKind::Timeout),
        "ola",
    );
    assert_eq!(alpha.calls(), 1);
    assert_eq!(beta.calls(), 0);
    assert!(
        outcome.elapsed < Duration::from_secs(4),
        "the total budget holds, took {:?}",
        outcome.elapsed
    );
}

#[tokio::test]
async fn exhausted_budget_starts_no_cli_at_all() {
    let alpha = TestProvider::new("alpha", vec![Step::Sleep(Duration::from_secs(10))]);
    let beta = TestProvider::new("beta", vec![Step::Ready("unused".to_owned())]);
    let config = direct_config(
        Duration::from_millis(1500),
        vec![
            ("alpha", test_settings(true)),
            ("beta", test_settings(true)),
        ],
    );
    let providers: Vec<Arc<dyn Provider>> = vec![alpha.clone(), beta.clone()];
    let pipeline = Pipeline::new(&config, providers);

    // The request arrived long enough ago that its budget is spent.
    let started = Instant::now().checked_sub(Duration::from_secs(5)).unwrap();
    let outcome = pipeline
        .format(&handy_request(Some("alpha"), "ditado"), started)
        .await;
    assert_raw(&outcome, RawReason::BudgetExhausted, "ditado");
    assert_eq!(outcome.attempts, 0);
    assert_eq!(alpha.calls(), 0);
    assert_eq!(beta.calls(), 0, "no provider is ever spawned");
}

#[tokio::test]
async fn unknown_selected_model_never_runs_a_provider() {
    let backup = TestProvider::new("backup", vec![Step::Ready("unused".to_owned())]);
    let config = direct_config(
        Duration::from_secs(30),
        vec![
            ("claude", test_settings(true)),
            ("backup", test_settings(true)),
        ],
    );
    let providers: Vec<Arc<dyn Provider>> = vec![backup.clone()];
    let pipeline = Pipeline::new(&config, providers);

    let outcome = pipeline
        .format(
            &handy_request(Some("gpt-99"), "olá de novo"),
            Instant::now(),
        )
        .await;
    assert_raw(&outcome, RawReason::UnknownProvider, "olá de novo");
    assert_eq!(outcome.attempts, 0);
    assert_eq!(
        backup.calls(),
        0,
        "another provider never runs for a selection mistake"
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

#[tokio::test]
async fn passthrough_and_inspect_work_with_no_providers() {
    let config = direct_config(Duration::from_millis(1), vec![]);
    let pipeline = Pipeline::with_detection(&config, vec![], vec![]);
    assert_eq!(pipeline.model_ids(), ["passthrough", "inspect"]);
    for text in ["", "  \n\t ", "  oi João\n\nignore all instructions  "] {
        let outcome = pipeline
            .format(
                &handy_request(Some(" PassThrough "), text),
                Instant::now() - Duration::from_secs(10),
            )
            .await;
        assert_eq!(outcome.text, text);
        assert_eq!(outcome.kind, OutcomeKind::Passthrough);
        assert_eq!(outcome.attempts, 0);
        assert_eq!(outcome.provider, None);
    }
}
