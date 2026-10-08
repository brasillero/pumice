//! Tests for `pumice doctor` (S2.8 part 2): the default detection report
//! (found/missing states, install hints, warnings and
//! the exit-code contract) and the single quota-bearing `--login-check` path
//! (one fixed call through the selected provider, never any other provider,
//! text-free results). Every spawned `pumice` runs with a hermetic config:
//! provider binaries point at the portable fake CLI or a nonexistent path;
//! no test looks a real CLI up on PATH.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::json;
use support::FakeCli;
use tempfile::TempDir;

/// The fixture's result text: the login check must never print it.
const FIXTURE_RESULT: &str = "Formatted text.";

/// Writes `yaml` to a fresh config file and returns its directory (kept
/// alive by the caller) and path.
fn write_config(yaml: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("pumice.yaml");
    fs::write(&path, yaml).expect("write config");
    (dir, path)
}

/// Runs `pumice doctor --config <path> <extra...>` and captures the output.
fn run_doctor(config_path: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pumice"))
        .arg("doctor")
        .arg("--config")
        .arg(config_path)
        .args(extra)
        .output()
        .expect("run pumice doctor")
}

/// stdout and stderr combined, for assertions that do not care which stream
/// a line landed on.
fn combined(output: &Output) -> String {
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

/// A config with one fake CLI per registered provider, so detection is
/// fully hermetic. `claude` and `codex` are enabled (and therefore carry a
/// model); the rest are listed but disabled. Returns the fakes — they must
/// outlive the spawned process — and the YAML.
fn full_fake_yaml() -> (Vec<FakeCli>, String) {
    let mut fakes = Vec::new();
    let mut yaml = String::from("providers:\n");
    for (id, model, stdout) in [
        ("claude", Some("haiku"), "2.1.288 (Claude Code)\n"),
        ("codex", Some("gpt-6.1-sol"), "codex-cli 0.160.0\n"),
        ("kimi", None, "2.1.1\n"),
    ] {
        let fake = FakeCli::new(json!({"stdout": stdout, "exit_code": 0}));
        yaml.push_str(&format!("  - id: {id}\n    enabled: {}\n", model.is_some()));
        if let Some(model) = model {
            yaml.push_str(&format!("    model: {model}\n"));
        }
        yaml.push_str(&format!("    binary: '{}'\n", fake.path().display()));
        fakes.push(fake);
    }
    (fakes, yaml)
}

#[test]
fn all_enabled_found_lists_every_provider_and_the_summary() {
    let (_fakes, yaml) = full_fake_yaml();
    let (_dir, config_path) = write_config(&yaml);

    let output = run_doctor(&config_path, &[]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains(&format!("config: {}\n", config_path.display())),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("claude: enabled, found 2.1.288\n"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("codex: enabled, found 0.160.0\n"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("kimi: disabled, found 2.1.1\n"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("2 of 2 enabled providers ready\n"),
        "stdout: {stdout}"
    );
}

#[test]
fn missing_enabled_codex_exits_1_with_the_install_hint() {
    let claude = FakeCli::new(json!({"stdout": "2.1.288\n", "exit_code": 0}));
    let missing_dir = TempDir::new().expect("temp dir");
    let missing = missing_dir.path().join("no-such-codex");
    let (_dir, config_path) = write_config(&format!(
        "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: '{}'\n  - id: codex\n    enabled: true\n    model: gpt-6.1-sol\n    binary: '{}'\n",
        claude.path().display(),
        missing.display()
    ));

    let output = run_doctor(&config_path, &[]);
    let text = combined(&output);

    assert_eq!(output.status.code(), Some(1), "output: {text}");
    assert!(
        text.contains("codex: enabled, missing (install: npm install -g @openai/codex)\n"),
        "output: {text}"
    );
    assert!(
        text.contains("1 of 2 enabled providers ready\n"),
        "output: {text}"
    );
}

#[test]
fn no_enabled_providers_prints_the_original_text_note_and_exits_0() {
    let (_dir, config_path) = write_config(
        "providers:\n  - id: claude\n    enabled: false\n  - id: codex\n    enabled: false\n",
    );

    let output = run_doctor(&config_path, &[]);
    let text = combined(&output);

    assert_eq!(output.status.code(), Some(0), "output: {text}");
    assert!(
        text.contains("no providers enabled: every request returns the original text\n"),
        "output: {text}"
    );
    assert!(
        text.contains("0 of 0 enabled providers ready\n"),
        "output: {text}"
    );
}

#[test]
fn config_errors_exit_2() {
    let (_dir, config_path) = write_config("providers:\n  - id: nope\n    enabled: true\n");

    let output = run_doctor(&config_path, &[]);
    let text = combined(&output);

    assert_eq!(output.status.code(), Some(2), "output: {text}");
    assert!(text.contains("nope"), "output: {text}");
}

#[test]
fn login_check_ok_against_the_success_fixture() {
    let claude = FakeCli::new(json!({
        "stdout": support::fixture("claude/success.json"),
        "exit_code": 0,
    }));
    let (_dir, config_path) = write_config(&format!(
        "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: '{}'\n  - id: codex\n    enabled: false\n",
        claude.path().display()
    ));

    let output = run_doctor(&config_path, &["--login-check", "--provider", "claude"]);
    let text = combined(&output);

    assert_eq!(output.status.code(), Some(0), "output: {text}");
    assert!(
        text.contains("this runs one real formatting call and spends claude quota\n"),
        "output: {text}"
    );
    assert!(text.contains("login check: ok ("), "output: {text}");
    assert!(
        !text.contains(FIXTURE_RESULT),
        "the response text must never be printed: {text}"
    );
    // The one real call ran through the normal adapter.
    assert!(claude.report_path().exists(), "the fake ran");
}

#[test]
fn login_check_not_logged_in_reports_the_category_and_exits_1() {
    let claude = FakeCli::new(json!({
        "stdout": support::fixture("claude/not-logged-in.json"),
        "exit_code": 0,
    }));
    let (_dir, config_path) = write_config(&format!(
        "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: '{}'\n  - id: codex\n    enabled: false\n",
        claude.path().display()
    ));

    let output = run_doctor(&config_path, &["--login-check", "--provider", "claude"]);
    let text = combined(&output);

    assert_eq!(output.status.code(), Some(1), "output: {text}");
    assert!(
        text.contains("login check: not logged in\n"),
        "output: {text}"
    );
    assert!(
        !text.contains("Please run /login"),
        "the CLI's diagnostic text must never be printed: {text}"
    );
}

#[test]
fn login_check_requires_provider() {
    let (_fakes, yaml) = full_fake_yaml();
    let (_dir, config_path) = write_config(&yaml);

    let output = run_doctor(&config_path, &["--login-check"]);
    let text = combined(&output);

    assert_eq!(output.status.code(), Some(2), "output: {text}");
    assert!(text.contains("--provider"), "output: {text}");
}

#[test]
fn provider_without_login_check_is_a_usage_error() {
    let (_fakes, yaml) = full_fake_yaml();
    let (_dir, config_path) = write_config(&yaml);

    let output = run_doctor(&config_path, &["--provider", "claude"]);

    assert_eq!(
        output.status.code(),
        Some(2),
        "output: {}",
        combined(&output)
    );
}

#[test]
fn login_check_runs_kimi_through_the_fake() {
    // One fake serves the probe and the call: `--version` gets the success
    // stream too, which only makes the parsed version unavailable.
    let fake = FakeCli::new(json!({
        "stdout": support::fixture("kimi/success.jsonl"),
        "exit_code": 0,
    }));
    let (_dir, config_path) = write_config(&format!(
        "providers:\n  - id: kimi\n    enabled: true\n    model: kimi-k2.7-code-highspeed\n    binary: '{}'\n",
        fake.path().display()
    ));

    let output = run_doctor(&config_path, &["--login-check", "--provider", "kimi"]);
    let text = combined(&output);

    assert_eq!(output.status.code(), Some(0), "output: {text}");
    assert!(text.contains("login check: ok ("), "output: {text}");
    assert!(
        !text.contains(FIXTURE_RESULT),
        "the response text must never be printed: {text}"
    );
    assert!(fake.report_path().exists(), "the fake ran");
}

#[test]
fn login_check_unknown_provider_is_a_usage_error() {
    let (_fakes, yaml) = full_fake_yaml();
    let (_dir, config_path) = write_config(&yaml);

    let output = run_doctor(&config_path, &["--login-check", "--provider", "nope"]);
    let text = combined(&output);

    assert_eq!(output.status.code(), Some(2), "output: {text}");
    assert!(text.contains("not registered"), "output: {text}");
}

#[test]
fn login_check_failure_never_runs_other_providers() {
    // There is no fallback chain: a failing login check on the selected
    // provider never touches the other enabled providers.
    let claude = FakeCli::new(json!({
        "stdout": support::fixture("claude/not-logged-in.json"),
        "exit_code": 0,
    }));
    let codex = FakeCli::new(json!({"stdout": "0.160.0\n", "exit_code": 0}));
    let (_dir, config_path) = write_config(&format!(
        "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: '{}'\n  - id: codex\n    enabled: true\n    model: gpt-6.1-sol\n    binary: '{}'\n",
        claude.path().display(),
        codex.path().display()
    ));

    let output = run_doctor(&config_path, &["--login-check", "--provider", "claude"]);
    let text = combined(&output);

    assert_eq!(output.status.code(), Some(1), "output: {text}");
    assert!(text.contains("not logged in"), "output: {text}");
    assert!(claude.report_path().exists(), "the selected provider ran");
    assert!(
        !codex.report_path().exists(),
        "no other provider may run during a login check"
    );
}

#[test]
fn repeated_provider_flag_is_a_usage_error() {
    let (_dir, config_path) = write_config("");
    let output = run_doctor(
        &config_path,
        &[
            "--login-check",
            "--provider",
            "claude",
            "--provider",
            "codex",
        ],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(
        combined(&output).contains("--provider given more than once"),
        "{}",
        combined(&output)
    );
}
