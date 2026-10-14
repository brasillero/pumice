//! Smoke tests for the root `pumice` binary.
//!
//! The relay's full test suite lives in `crates/relay/tests`; this file only
//! verifies that the legacy product binary still starts and reports itself
//! correctly.

use std::process::Command;

use tempfile::TempDir;

#[test]
fn help_matches_legacy_text() {
    let output = Command::new(env!("CARGO_BIN_EXE_pumice"))
        .arg("--help")
        .output()
        .expect("pumice binary runs");
    assert!(output.status.success(), "--help exits 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("pumice - connects dictation apps"));
    assert!(stdout.contains("pumice serve"));
    assert!(stdout.contains("pumice check-config"));
    assert!(stdout.contains("pumice doctor"));
}

#[test]
fn version_reports_name_and_semver() {
    let output = Command::new(env!("CARGO_BIN_EXE_pumice"))
        .arg("--version")
        .output()
        .expect("pumice binary runs");
    assert!(output.status.success(), "--version exits 0");
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let version = env!("CARGO_PKG_VERSION");
    assert_eq!(stdout, format!("pumice {version}"));
}

#[test]
fn check_config_with_defaults_succeeds() {
    let config_home = TempDir::new().expect("temp config dir");
    let output = Command::new(env!("CARGO_BIN_EXE_pumice"))
        .arg("check-config")
        .env("XDG_CONFIG_HOME", config_home.path())
        .env("APPDATA", config_home.path())
        .output()
        .expect("pumice binary runs");
    assert!(
        output.status.success(),
        "check-config with defaults exits 0"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("config: built-in defaults"));
}
