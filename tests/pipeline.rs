//! Tests for the formatting pipeline: provider selection, deadlines, the
//! busy guard and the exact raw-text fallback. Everything runs through the
//! portable fake CLI; no real AI CLI is ever invoked.

mod support;

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pumice::config::{self, Config, DebugLogSettings, PromptSettings};
use pumice::pipeline::{FormatOutcome, OutcomeKind, Pipeline, ProviderErrorKind, RawReason};
use pumice::process::ProcessRunner;
use pumice::providers::{
    self, FormatInput, Provider, ProviderError, ProviderErrorCode, ProviderFuture, ProviderSettings,
};
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

/// One scripted reaction of a [`TestProvider`].
enum Step {
    /// Succeed with this final text.
    Ready(String),
    /// Fail immediately with this error.
    Fail(ProviderError),
    /// Sleep this long before succeeding; the test expects the pipeline's
    /// deadline to fire first.
    Sleep(Duration),
}

/// A scripted [`Provider`] for fallback-chain tests: every `format` call
/// pops the next [`Step`] and counts as one call. Real fake-CLI providers
/// cannot build two chain entries (every adapter reports one fixed id), so
/// chain logic runs against this double, with one real adapter mixed in
/// where the protocol matters.
struct TestProvider {
    id: &'static str,
    calls: AtomicUsize,
    steps: Mutex<VecDeque<Step>>,
}

impl TestProvider {
    fn new(id: &'static str, steps: Vec<Step>) -> Arc<TestProvider> {
        Arc::new(TestProvider {
            id,
            calls: AtomicUsize::new(0),
            steps: Mutex::new(steps.into()),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Provider for TestProvider {
    fn id(&self) -> &'static str {
        self.id
    }

    fn format<'a>(&'a self, _input: FormatInput<'a>, _deadline: Instant) -> ProviderFuture<'a> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let step = self
            .steps
            .lock()
            .expect("test provider steps")
            .pop_front()
            .expect("test provider has a scripted step");
        Box::pin(async move {
            match step {
                Step::Ready(text) => Ok(text),
                Step::Fail(error) => Err(error),
                Step::Sleep(duration) => {
                    tokio::time::sleep(duration).await;
                    Ok("woke up after the deadline".to_owned())
                }
            }
        })
    }
}

/// Claude defaults under a test-only id, for directly built configs (YAML
/// only knows registered provider ids).
fn test_settings(enabled: bool) -> ProviderSettings {
    let descriptor = providers::descriptor("claude").expect("claude is registered");
    let mut settings = (descriptor.defaults)();
    settings.enabled = enabled;
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
fn direct_config(
    default_provider: &str,
    total_timeout: Duration,
    fallback_order: &[&str],
    entries: Vec<(&str, ProviderSettings)>,
) -> Config {
    Config {
        port: 7567,
        default_provider: default_provider.to_owned(),
        total_timeout,
        fallback_order: fallback_order.iter().map(|id| id.to_string()).collect(),
        prompts: PromptSettings::default(),
        debug_log: DebugLogSettings {
            enabled: false,
            path: PathBuf::from("pumice-debug.jsonl"),
        },
        providers: entries
            .into_iter()
            .map(|(id, settings)| (id.to_owned(), settings))
            .collect(),
    }
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

#[tokio::test]
async fn selected_failure_falls_back_to_the_next_provider() {
    // Claude's fake answers with a not-logged-in envelope; the chain must
    // move on to the next provider, which formats the dictation.
    let fake = FakeCli::new(json!({
        "stdout": support::fixture("claude/not-logged-in.json"),
        "exit_code": 1,
    }));
    let backup = TestProvider::new("backup", vec![Step::Ready("Olá de novo.".to_owned())]);
    let config = direct_config(
        "claude",
        Duration::from_secs(30),
        &["backup"],
        vec![
            ("claude", claude_settings_at(&fake)),
            ("backup", test_settings(true)),
        ],
    );
    let providers: Vec<Arc<dyn Provider>> =
        vec![build_claude(&config.providers["claude"]), backup.clone()];
    let pipeline = Pipeline::new(&config, providers);

    let outcome = pipeline
        .format(&handy_request(None, "ola mundo"), Instant::now())
        .await;
    assert_eq!(outcome.kind, OutcomeKind::Formatted);
    assert_eq!(outcome.provider, Some("backup"));
    assert_eq!(outcome.attempts, 2);
    assert_eq!(outcome.text, "Olá de novo.");
    assert!(fake.report_path().exists(), "the fake ran once");
    assert_eq!(backup.calls(), 1);
}

#[tokio::test]
async fn fallback_provider_never_runs_when_the_selected_succeeds() {
    let selected = TestProvider::new("alpha", vec![Step::Ready("Texto pronto.".to_owned())]);
    let backup = TestProvider::new("backup", vec![Step::Ready("unused".to_owned())]);
    let config = direct_config(
        "alpha",
        Duration::from_secs(30),
        &["backup"],
        vec![
            ("alpha", test_settings(true)),
            ("backup", test_settings(true)),
        ],
    );
    let providers: Vec<Arc<dyn Provider>> = vec![selected.clone(), backup.clone()];
    let pipeline = Pipeline::new(&config, providers);

    let outcome = pipeline
        .format(&handy_request(None, "dita"), Instant::now())
        .await;
    assert_eq!(outcome.kind, OutcomeKind::Formatted);
    assert_eq!(outcome.provider, Some("alpha"));
    assert_eq!(outcome.attempts, 1);
    assert_eq!(selected.calls(), 1);
    assert_eq!(backup.calls(), 0, "the fallback must not run");
}

#[tokio::test]
async fn selected_reappearing_in_fallback_order_runs_once() {
    // Through real YAML this time: the selected id may also appear in
    // `fallback_order`; it must not be called twice.
    let (pipeline, fake) = pipeline(
        "fallback_order: [claude]\nproviders:\n  claude:\n    binary: '{binary}'\n",
        success_scenario("Feito."),
    );
    let outcome = pipeline
        .format(&handy_request(None, "dita"), Instant::now())
        .await;
    assert_eq!(outcome.kind, OutcomeKind::Formatted);
    assert_eq!(outcome.provider, Some("claude"));
    assert_eq!(outcome.attempts, 1);
    assert!(fake.report_path().exists(), "the fake ran once");
}

#[tokio::test]
async fn every_failing_candidate_returns_raw_with_the_last_failure() {
    let alpha = TestProvider::new("alpha", vec![Step::Fail(ProviderError::NotLoggedIn)]);
    let beta = TestProvider::new(
        "beta",
        vec![Step::Fail(ProviderError::QuotaExceeded {
            retry_after: None,
        })],
    );
    let config = direct_config(
        "alpha",
        Duration::from_secs(30),
        &["beta"],
        vec![
            ("alpha", test_settings(true)),
            ("beta", test_settings(true)),
        ],
    );
    let providers: Vec<Arc<dyn Provider>> = vec![alpha.clone(), beta.clone()];
    let pipeline = Pipeline::new(&config, providers);

    let outcome = pipeline
        .format(&handy_request(None, "texto ditado"), Instant::now())
        .await;
    assert_raw(
        &outcome,
        RawReason::ProviderFailed(ProviderErrorKind::QuotaExceeded { retry_after: None }),
        "texto ditado",
    );
    assert_eq!(outcome.attempts, 2);
    assert_eq!(alpha.calls(), 1);
    assert_eq!(beta.calls(), 1);
}

#[tokio::test]
async fn cleanup_failure_as_the_last_failure_is_reported() {
    let alpha = TestProvider::new("alpha", vec![Step::Fail(ProviderError::NotLoggedIn)]);
    // Preamble-only output cleans down to nothing: a cleanup failure.
    let beta = TestProvider::new(
        "beta",
        vec![Step::Ready("Here is the formatted text:".to_owned())],
    );
    let config = direct_config(
        "alpha",
        Duration::from_secs(30),
        &["beta"],
        vec![
            ("alpha", test_settings(true)),
            ("beta", test_settings(true)),
        ],
    );
    let providers: Vec<Arc<dyn Provider>> = vec![alpha.clone(), beta.clone()];
    let pipeline = Pipeline::new(&config, providers);

    let outcome = pipeline
        .format(&handy_request(None, "texto ditado"), Instant::now())
        .await;
    assert_raw(&outcome, RawReason::CleanupFailed, "texto ditado");
    assert_eq!(outcome.attempts, 2);
    assert_eq!(alpha.calls(), 1);
    assert_eq!(beta.calls(), 1);
}

#[tokio::test]
async fn cleanup_failure_falls_back_to_the_next_provider() {
    // The real adapter's preamble-only output fails cleanup; the next
    // candidate formats the dictation instead.
    let fake = FakeCli::new(success_scenario("Here is the formatted text:"));
    let backup = TestProvider::new("backup", vec![Step::Ready("Texto limpo.".to_owned())]);
    let config = direct_config(
        "claude",
        Duration::from_secs(30),
        &["backup"],
        vec![
            ("claude", claude_settings_at(&fake)),
            ("backup", test_settings(true)),
        ],
    );
    let providers: Vec<Arc<dyn Provider>> =
        vec![build_claude(&config.providers["claude"]), backup.clone()];
    let pipeline = Pipeline::new(&config, providers);

    let outcome = pipeline
        .format(&handy_request(None, "ola mundo"), Instant::now())
        .await;
    assert_eq!(outcome.kind, OutcomeKind::Formatted);
    assert_eq!(outcome.provider, Some("backup"));
    assert_eq!(outcome.attempts, 2);
    assert_eq!(outcome.text, "Texto limpo.");
    assert!(fake.report_path().exists(), "the fake ran once");
    assert_eq!(backup.calls(), 1);
}

#[tokio::test]
async fn selected_timeout_falls_back_within_the_total_budget() {
    // One-second provider timeout inside a five-second total budget: the
    // timeout fires, the next provider formats, and the total holds.
    let mut alpha_settings = test_settings(true);
    alpha_settings.timeout = Duration::from_secs(1);
    let alpha = TestProvider::new("alpha", vec![Step::Sleep(Duration::from_secs(10))]);
    let beta = TestProvider::new("beta", vec![Step::Ready("Depois do timeout.".to_owned())]);
    let config = direct_config(
        "alpha",
        Duration::from_secs(5),
        &["beta"],
        vec![("alpha", alpha_settings), ("beta", test_settings(true))],
    );
    let providers: Vec<Arc<dyn Provider>> = vec![alpha.clone(), beta.clone()];
    let pipeline = Pipeline::new(&config, providers);

    let outcome = pipeline
        .format(&handy_request(None, "ola"), Instant::now())
        .await;
    assert_eq!(outcome.kind, OutcomeKind::Formatted);
    assert_eq!(outcome.provider, Some("beta"));
    assert_eq!(outcome.attempts, 2);
    assert_eq!(alpha.calls(), 1);
    assert_eq!(beta.calls(), 1);
    assert!(
        outcome.elapsed < Duration::from_secs(4),
        "the total budget holds, took {:?}",
        outcome.elapsed
    );
}

#[tokio::test]
async fn exhausted_budget_mid_chain_starts_no_further_cli() {
    let alpha = TestProvider::new("alpha", vec![Step::Sleep(Duration::from_secs(10))]);
    let beta = TestProvider::new("beta", vec![Step::Ready("unused".to_owned())]);
    let config = direct_config(
        "alpha",
        Duration::from_millis(1500),
        &["beta"],
        vec![
            ("alpha", test_settings(true)),
            ("beta", test_settings(true)),
        ],
    );
    let providers: Vec<Arc<dyn Provider>> = vec![alpha.clone(), beta.clone()];
    let pipeline = Pipeline::new(&config, providers);

    let outcome = pipeline
        .format(&handy_request(None, "ditado"), Instant::now())
        .await;
    // A candidate was tried, so the last failure wins over BudgetExhausted.
    assert_raw(
        &outcome,
        RawReason::ProviderFailed(ProviderErrorKind::Timeout),
        "ditado",
    );
    assert_eq!(outcome.attempts, 1);
    assert_eq!(beta.calls(), 0, "the second provider is never spawned");
    assert!(
        outcome.elapsed < Duration::from_millis(1500) + Duration::from_secs(1),
        "took {:?}",
        outcome.elapsed
    );
}

#[tokio::test]
async fn fallback_chain_skips_duplicates_and_unbuilt_providers() {
    let alpha = TestProvider::new(
        "alpha",
        vec![Step::Fail(ProviderError::other(
            ProviderErrorCode::NonzeroExit,
        ))],
    );
    let gamma = TestProvider::new("gamma", vec![Step::Ready("Texto final.".to_owned())]);
    // `beta` is configured but disabled, so it was never built; `alpha`
    // repeats twice — once as the selected provider.
    let config = direct_config(
        "alpha",
        Duration::from_secs(30),
        &["alpha", "beta", "gamma", "alpha"],
        vec![
            ("alpha", test_settings(true)),
            ("beta", test_settings(false)),
            ("gamma", test_settings(true)),
        ],
    );
    let providers: Vec<Arc<dyn Provider>> = vec![alpha.clone(), gamma.clone()];
    let pipeline = Pipeline::new(&config, providers);

    let outcome = pipeline
        .format(&handy_request(None, "ola"), Instant::now())
        .await;
    assert_eq!(outcome.kind, OutcomeKind::Formatted);
    assert_eq!(outcome.provider, Some("gamma"));
    assert_eq!(outcome.attempts, 2, "alpha and gamma only");
    assert_eq!(alpha.calls(), 1, "duplicates never call a provider twice");
    assert_eq!(gamma.calls(), 1);
}

#[tokio::test]
async fn unknown_selected_model_never_starts_the_chain() {
    let backup = TestProvider::new("backup", vec![Step::Ready("unused".to_owned())]);
    let config = direct_config(
        "claude",
        Duration::from_secs(30),
        &["backup"],
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
        "a fallback never runs for a selection mistake"
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
