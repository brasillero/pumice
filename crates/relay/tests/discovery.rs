//! Tests for startup provider detection (S2.8 part 1): version parsing over
//! realistic outputs, missing binaries, probe deadlines, the never-spawn
//! guarantees for Antigravity and the CLI-less generic adapter, and
//! full-registry detection. Every CLI below is the portable fake; no test
//! looks a real CLI up on PATH (each test passes descriptors whose binaries
//! it controls).

mod support;

use std::path::Path;
use std::time::Duration;

use pumice::config::{self, Config};
use pumice::process::ProcessRunner;
use pumice::providers::discovery::{self, Found, Version};
use pumice::providers::{self, ProbeSpec, ProviderDescriptor};
use serde_json::json;
use support::FakeCli;
use tempfile::TempDir;

/// `descriptor` copied out of the registry (descriptors are `Copy`).
fn descriptor(id: &str) -> ProviderDescriptor {
    *providers::descriptor(id).unwrap_or_else(|| panic!("provider {id} is registered"))
}

/// Loads a configuration from `yaml` written into a fresh temporary
/// directory. Relative binary paths would resolve against it, so the tests
/// always pass absolute fake paths and the directory can drop after loading.
fn load(yaml: &str) -> Config {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("pumice.yaml");
    std::fs::write(&path, yaml).expect("write config");
    config::load_with_env(Some(&path), |_| None)
        .expect("config loads")
        .config
}

/// A config with `claude` enabled, its model and binary at `path` (every
/// other setting default).
fn claude_at(path: &Path) -> Config {
    load(&format!(
        "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: '{}'\n",
        path.display()
    ))
}

#[tokio::test]
async fn version_probes_parse_realistic_cli_outputs() {
    for (stdout, expected) in [
        ("2.1.288 (Claude Code)\n", "2.1.288"),
        ("codex-cli 0.160.0\n", "0.160.0"),
        ("1.18.34\n", "1.18.34"),
    ] {
        let fake = FakeCli::new(json!({"stdout": stdout, "exit_code": 0}));
        let config = claude_at(fake.path());
        let statuses =
            discovery::detect(&config, &[descriptor("claude")], &ProcessRunner::new()).await;

        assert_eq!(statuses.len(), 1);
        let status = &statuses[0];
        assert_eq!(status.id, "claude");
        assert!(status.enabled);
        assert!(
            matches!(status.found, Found::Found(_)),
            "stdout {stdout:?} must leave the provider found"
        );
        assert_eq!(status.version, Version::Parsed(expected.to_owned()));
    }
}

#[tokio::test]
async fn garbage_or_failing_probes_leave_the_version_unavailable() {
    for scenario in [
        json!({"stdout": "no version information here\n", "exit_code": 0}),
        json!({"stdout": "2.1.288\n", "exit_code": 1}),
        json!({"stdout": "x".repeat(8 * 1024), "exit_code": 0}),
    ] {
        let fake = FakeCli::new(scenario);
        let config = claude_at(fake.path());
        let statuses =
            discovery::detect(&config, &[descriptor("claude")], &ProcessRunner::new()).await;

        assert!(
            matches!(statuses[0].found, Found::Found(_)),
            "a failed probe never means missing"
        );
        assert_eq!(statuses[0].version, Version::Unavailable);
    }
}

#[tokio::test]
async fn a_missing_binary_reports_missing() {
    // The directory exists so the path is absolute and well-formed; the
    // binary inside it does not.
    let dir = TempDir::new().expect("temp dir");
    let missing = dir.path().join("claude");
    let config = claude_at(&missing);
    let statuses = discovery::detect(&config, &[descriptor("claude")], &ProcessRunner::new()).await;

    assert_eq!(statuses[0].found, Found::Missing);
    assert_eq!(statuses[0].version, Version::Unavailable);
}

#[tokio::test]
async fn a_slow_probe_dies_with_its_deadline() {
    let fake = FakeCli::new(json!({"stdout": "2.1.288", "exit_code": 0, "sleep_ms": 10_000}));
    let config = claude_at(fake.path());
    let started = std::time::Instant::now();
    let statuses = discovery::detect_with_timeout(
        &config,
        &[descriptor("claude")],
        &ProcessRunner::new(),
        Duration::from_millis(200),
    )
    .await;
    let elapsed = started.elapsed();

    assert!(
        matches!(statuses[0].found, Found::Found(_)),
        "the timed-out probe is still found"
    );
    assert_eq!(statuses[0].version, Version::Unavailable);
    assert!(
        elapsed < Duration::from_secs(5),
        "the probe must die with its 200 ms deadline, not the fake's 10 s sleep: {elapsed:?}"
    );
}

#[tokio::test]
async fn detection_covers_every_registered_provider_enabled_or_not() {
    let claude = FakeCli::new(json!({"stdout": "2.1.288 (Claude Code)"}));
    let codex = FakeCli::new(json!({"stdout": "codex-cli 0.160.0"}));
    let kimi = FakeCli::new(json!({"stdout": "2.1.1"}));
    let kiro = FakeCli::new(json!({"stdout": "2.29.0"}));
    let config = load(&format!(
        "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: '{0}'\n  - id: codex\n    enabled: true\n    model: gpt-6.1-sol\n    binary: '{1}'\n  - id: kimi\n    enabled: false\n    binary: '{2}'\n  - id: kiro\n    enabled: false\n    binary: '{3}'\n",
        claude.path().display(),
        codex.path().display(),
        kimi.path().display(),
        kiro.path().display(),
    ));
    let statuses = discovery::detect(&config, providers::PROVIDERS, &ProcessRunner::new()).await;

    assert_eq!(statuses.len(), providers::PROVIDERS.len());
    let status = |id: &str| {
        statuses
            .iter()
            .find(|status| status.id == id)
            .unwrap_or_else(|| panic!("status for {id} present"))
    };
    assert!(matches!(status("claude").found, Found::Found(_)));
    assert_eq!(status("claude").version, Version::Parsed("2.1.288".into()));
    assert_eq!(status("codex").version, Version::Parsed("0.160.0".into()));
    // Disabled but still probed, so `doctor` can show it.
    assert!(!status("kimi").enabled);
    assert_eq!(status("kimi").version, Version::Parsed("2.1.1".into()));
    assert!(!status("kiro").enabled);
    assert_eq!(status("kiro").version, Version::Parsed("2.29.0".into()));
}

#[test]
fn registry_probe_metadata_is_consistent() {
    for descriptor in providers::PROVIDERS {
        match descriptor.probe {
            ProbeSpec::Version(args) => assert_eq!(args, ["--version"], "{}", descriptor.id),
            ProbeSpec::PathOnly | ProbeSpec::NotApplicable => {}
        }
        if descriptor.probe == ProbeSpec::NotApplicable {
            // No CLI exists: the empty default binary is never resolved.
            assert!(descriptor.default_binary.is_empty(), "{}", descriptor.id);
        } else {
            assert!(!descriptor.default_binary.is_empty(), "{}", descriptor.id);
        }
        assert!(!descriptor.install_hint.is_empty(), "{}", descriptor.id);
    }
}

/// Validates `yaml` against `descriptors` alone, so archived (unregistered)
/// providers keep their detection guarantees tested.
fn load_with(yaml: &str, descriptors: &[ProviderDescriptor]) -> Config {
    config::validate_text_with_descriptors(yaml, Path::new("pumice.yaml"), descriptors)
        .expect("config validates")
}

#[tokio::test]
async fn archived_antigravity_is_resolved_but_never_spawned() {
    let fake = FakeCli::new(json!({"stdout": "1.2.14", "exit_code": 0}));
    let descriptors = [providers::antigravity::DESCRIPTOR];
    let config = load_with(
        &format!(
            "providers:\n  - id: antigravity\n    enabled: false\n    binary: '{}'\n",
            fake.path().display()
        ),
        &descriptors,
    );
    let statuses = discovery::detect(&config, &descriptors, &ProcessRunner::new()).await;

    assert_eq!(statuses.len(), 1);
    assert!(!statuses[0].enabled);
    assert!(
        matches!(statuses[0].found, Found::Found(_)),
        "PATH lookup still reports the binary"
    );
    assert_eq!(statuses[0].version, Version::Skipped);
    assert!(
        !fake.report_path().exists(),
        "agy must never be spawned, not even for --version"
    );
}

#[tokio::test]
async fn archived_generic_is_never_resolved_or_spawned() {
    // The generic adapter has no CLI: even with a `binary` that exists on
    // disk, detection neither resolves nor spawns it.
    let fake = FakeCli::new(json!({}));
    let descriptors = [providers::generic::DESCRIPTOR];
    let config = load_with(
        &format!(
            "providers:\n  - id: generic\n    enabled: false\n    binary: '{}'\n",
            fake.path().display()
        ),
        &descriptors,
    );
    let statuses = discovery::detect(&config, &descriptors, &ProcessRunner::new()).await;

    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].found, Found::NotApplicable);
    assert_eq!(statuses[0].version, Version::Skipped);
    assert!(
        !fake.report_path().exists(),
        "the generic adapter has no CLI: nothing may ever be spawned for it"
    );
}

#[test]
fn archived_probe_metadata_keeps_its_guarantees() {
    assert_eq!(
        providers::antigravity::DESCRIPTOR.probe,
        ProbeSpec::PathOnly,
        "agy is never spawned, so its probe must stay PATH-only"
    );
    assert_eq!(
        providers::generic::DESCRIPTOR.probe,
        ProbeSpec::NotApplicable,
        "generic spawns no CLI, so detection must not touch PATH or the runner"
    );
}
