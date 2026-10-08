//! S2.5: the dormant Antigravity adapter.
//!
//! `agy` is never executed — not even `--version` — and no test looks it up
//! on PATH. Every protocol assertion runs against the fake CLI, and every
//! fixture is documentation-derived (docs/research/S0.2-cli-matrix.md's
//! "public documentation only" section): no Antigravity command was ever
//! run, so the coverage here proves protocol construction and safe refusal,
//! not safe real execution.

mod support;

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use pumice::config;
use pumice::pipeline::Pipeline;
use pumice::process::{Argument, ProcessRunner};
use pumice::providers::antigravity::{self, AntigravityAdapter};
use pumice::providers::cli::{CliAdapter, CliProvider};
use pumice::providers::{
    FormatInput, Provider, ProviderError, ProviderErrorCode, ProviderSettings, UserPrompt,
    build_from_config,
};
use serde_json::{Value, json};
use support::adapter_contract::{format_with, input, report_argv};
use support::{FakeCli, fixture};
use tempfile::TempDir;

/// Provider timeout for fake-CLI calls.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Model used by the test-only constructor (Antigravity has no default).
const MODEL: &str = "gemini-test";

fn adapter(timeout: Duration) -> AntigravityAdapter {
    AntigravityAdapter::new(
        PathBuf::from(antigravity::DEFAULT_BINARY),
        MODEL.to_owned(),
        timeout,
    )
}

fn fake_provider(fake: &FakeCli, timeout: Duration) -> Arc<dyn Provider> {
    Arc::new(CliProvider::new(
        AntigravityAdapter::new(fake.path().to_path_buf(), MODEL.to_owned(), timeout),
        Arc::new(ProcessRunner::new()),
        timeout,
    ))
}

fn argv_of(invocation: &pumice::process::CliInvocation) -> Vec<String> {
    invocation
        .args
        .iter()
        .map(|arg| match arg {
            Argument::Literal(value) => value.to_string_lossy().into_owned(),
            other => panic!("unexpected non-literal argument: {other:?}"),
        })
        .collect()
}

#[test]
fn invocation_argv_is_exact() {
    let invocation = adapter(Duration::from_secs(42))
        .invocation(input("ditado"))
        .expect("invocation builds");

    let args: Vec<OsString> = invocation
        .args
        .iter()
        .map(|arg| match arg {
            Argument::Literal(value) => value.clone(),
            other => panic!("unexpected non-literal argument: {other:?}"),
        })
        .collect();
    assert_eq!(
        args,
        [
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--print-timeout",
            "42s",
            "--sandbox",
            "--model",
            MODEL,
            "--effort",
            "low",
        ]
        .map(OsString::from)
    );
    // The merged-input transport names no file and sets no environment.
    assert!(invocation.control_files.is_empty());
    assert!(invocation.env.is_empty());
}

#[test]
fn stdin_event_is_one_merged_user_event() {
    let system = "You format speech. Coração 🎤";
    let text = "Olá! Ação — $(rm -rf ~) &";
    let invocation = adapter(Duration::from_secs(30))
        .invocation(FormatInput {
            system_prompt: system,
            user_prompt: UserPrompt {
                before_text: "Before\n",
                after_text: "\nAfter",
            },
            text,
        })
        .expect("invocation builds");

    // The merged input travels as stdin data, in no argv element.
    for arg in argv_of(&invocation) {
        assert!(!arg.contains(text), "argv element carries dictation: {arg}");
        assert!(
            !arg.contains(system),
            "argv element carries the system prompt: {arg}"
        );
    }

    let stdin = String::from_utf8(invocation.stdin).expect("UTF-8 stdin");
    // One newline-terminated JSON line; serde_json keeps Unicode literal.
    assert_eq!(stdin.matches('\n').count(), 1);
    assert!(stdin.ends_with('\n'));
    assert!(stdin.contains('🎤') && stdin.contains("Ação"));
    let event: Value =
        serde_json::from_str(stdin.trim_end_matches('\n')).expect("valid JSON event");
    assert_eq!(event["event"], json!("user"));
    assert_eq!(
        event["message"]["content"].as_str().unwrap(),
        format!("{system}\n\nBefore\n{text}\nAfter"),
        "the system prompt precedes the user message, separated by a blank line"
    );
}

/// Runs one parser case end to end through the fake CLI.
async fn parsed(stdout: &str, exit_code: i32) -> Result<String, ProviderError> {
    let fake = FakeCli::new(json!({"stdout": stdout, "exit_code": exit_code}));
    let provider = fake_provider(&fake, CALL_TIMEOUT);
    format_with(&provider, "ditado").await
}

#[tokio::test]
async fn parser_returns_only_the_terminal_response() {
    // All fixtures are documentation-derived.
    assert_eq!(
        parsed(&fixture("antigravity/success.ndjson"), 0)
            .await
            .unwrap(),
        "Olá mundo.",
        "text deltas, reasoning, checkpoints and usage never leak into the result"
    );
}

#[tokio::test]
async fn parser_rejects_tool_records() {
    assert_eq!(
        parsed(&fixture("antigravity/tool.ndjson"), 0)
            .await
            .unwrap_err(),
        ProviderError::other(ProviderErrorCode::UnexpectedToolActivity)
    );
}

#[tokio::test]
async fn parser_classifies_authentication_required() {
    assert_eq!(
        parsed(&fixture("antigravity/auth-required.ndjson"), 1)
            .await
            .unwrap_err(),
        ProviderError::NotLoggedIn
    );
}

#[tokio::test]
async fn parser_rejects_malformed_and_contradictory_streams() {
    let init = r#"{"event":"init"}"#;
    let result = |response: &str| {
        format!(r#"{{"event":"result","status":"SUCCESS","result":{{"response":"{response}"}}}}"#)
            + "\n"
    };
    let invalid: Vec<(String, i32)> = vec![
        (String::new(), 0),
        ("not json\n".to_owned(), 0),
        ("\n".to_owned(), 0),
        ("[1,2]\n".to_owned(), 0),
        // No "event" key.
        (
            "{\"status\":\"SUCCESS\",\"result\":{\"response\":\"x\"}}\n".to_owned(),
            0,
        ),
        // Missing initialization record.
        (result("x"), 0),
        // Two contradictory terminal results.
        (format!("{init}\n{0}{0}", result("x")), 0),
        // Missing and non-string responses.
        (
            format!("{init}\n{{\"event\":\"result\",\"status\":\"SUCCESS\"}}\n"),
            0,
        ),
        (
            format!(
                "{init}\n{{\"event\":\"result\",\"status\":\"SUCCESS\",\"result\":{{\"response\":42}}}}\n"
            ),
            0,
        ),
        // FAILURE with a clean exit is contradictory.
        (
            format!(
                "{init}\n{{\"event\":\"result\",\"status\":\"FAILURE\",\"result\":{{\"error\":\"boom\"}}}}\n"
            ),
            0,
        ),
        // A truncated stream dies mid-line.
        (
            fixture("antigravity/success.ndjson")
                .chars()
                .take(50)
                .collect(),
            0,
        ),
    ];
    for (stdout, exit_code) in invalid {
        assert_eq!(
            parsed(&stdout, exit_code).await.unwrap_err(),
            ProviderError::other(ProviderErrorCode::InvalidOutput),
            "case: {stdout}"
        );
    }

    // A successful-looking stream with a nonzero exit is a failure, not text.
    assert_eq!(
        parsed(&fixture("antigravity/success.ndjson"), 1)
            .await
            .unwrap_err(),
        ProviderError::other(ProviderErrorCode::NonzeroExit)
    );
    let failed = format!(
        "{init}\n{{\"event\":\"result\",\"status\":\"FAILURE\",\"result\":{{\"error\":\"boom\"}}}}\n"
    );
    assert_eq!(
        parsed(&failed, 1).await.unwrap_err(),
        ProviderError::other(ProviderErrorCode::NonzeroExit)
    );
}

fn no_env(_: &str) -> Option<OsString> {
    None
}

#[test]
fn absent_entry_means_not_configured() {
    let loaded = config::load_with_env(None, no_env).expect("defaults load");
    assert!(
        loaded.config.provider("antigravity").is_none(),
        "providers are only configured by list entries"
    );
    assert!(loaded.config.providers.is_empty());
}

#[test]
fn a_config_entry_is_refused_as_archived() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("pumice.yaml");
    fs::write(
        &path,
        "providers:\n  - id: antigravity\n    enabled: false\n",
    )
    .expect("write config");

    let error = config::load_with_env(Some(&path), no_env)
        .expect_err("archived providers cannot be listed")
        .to_string();
    assert!(
        error.contains(
            ":2:9: \"antigravity\" is archived for now and cannot be used; delete this entry"
        ),
        "{error}"
    );
}

#[test]
fn build_fails_even_when_called_directly() {
    let descriptor = &pumice::providers::antigravity::DESCRIPTOR;
    let settings = ProviderSettings {
        enabled: false,
        ..(descriptor.defaults)()
    };
    let error = (descriptor.build)(&settings, Arc::new(ProcessRunner::new()))
        .err()
        .expect("build never produces a runnable provider")
        .to_string();
    assert!(
        error.contains("cannot be enabled yet"),
        "defense in depth: {error}"
    );
}

#[test]
fn models_listing_never_includes_antigravity() {
    let loaded = config::load_with_env(None, no_env).expect("defaults load");
    let providers = build_from_config(&loaded.config, Arc::new(ProcessRunner::new()))
        .expect("enabled providers build");
    let pipeline = Pipeline::new(&loaded.config, providers);
    let ids = pipeline.model_ids();
    assert!(!ids.contains(&"antigravity"), "{ids:?}");
    assert_eq!(ids, ["passthrough", "inspect"]);
}

#[tokio::test]
async fn end_to_end_format_through_the_fake_cli() {
    let fake = FakeCli::new(json!({
        "stdout": fixture("antigravity/success.ndjson"),
        "exit_code": 0,
    }));
    let provider = fake_provider(&fake, CALL_TIMEOUT);

    assert_eq!(
        format_with(&provider, "ditado de teste").await.unwrap(),
        "Olá mundo."
    );
    let report = fake.report();
    assert_eq!(
        report_argv(&report),
        [
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--print-timeout",
            "30s",
            "--sandbox",
            "--model",
            MODEL,
            "--effort",
            "low",
        ]
    );
    assert_eq!(
        report["cwd_entries"],
        json!([]),
        "the workspace starts empty"
    );
}

#[test]
fn enabled_true_is_refused_at_the_enabled_line() {
    // The archived descriptor still refuses enablement at the exact line.
    let error = config::validate_text_with_descriptors(
        "providers:\n  - id: antigravity\n    enabled: true\n",
        std::path::Path::new("pumice.yaml"),
        &[pumice::providers::antigravity::DESCRIPTOR],
    )
    .expect_err("enablement is refused")
    .to_string();
    assert!(
        error.contains(":3:14: providers.antigravity cannot be enabled yet"),
        "{error}"
    );
}
