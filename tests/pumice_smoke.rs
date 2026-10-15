//! CLI tests for the `pumice` dispatcher binary.
//!
//! The relay's full test suite lives in `crates/relay/tests`; this file
//! verifies the orchestrator entry point: the bare-command contract (D1),
//! the `relay` subcommand against a hermetic config directory, the
//! `relay.yml` rename (D3) and the `transcribe` sibling dispatch (D2). No
//! provider CLI is ever invoked — `check-config` only validates YAML.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use tempfile::TempDir;

/// Every config-reading test runs against its own XDG/APPDATA directory so
/// the developer's real per-user config never takes part.
struct HermeticHome {
    _keep: TempDir,
    xdg: PathBuf,
}

impl HermeticHome {
    fn new() -> Self {
        let keep = TempDir::new().expect("temp config home");
        let xdg = keep.path().join("xdg");
        fs::create_dir_all(&xdg).expect("create config home");
        HermeticHome { _keep: keep, xdg }
    }

    /// The relay's per-user config directory inside this home.
    fn pumice_dir(&self) -> PathBuf {
        self.xdg.join("pumice")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pumice"));
        command
            .args(args)
            .env("XDG_CONFIG_HOME", &self.xdg)
            .env("APPDATA", &self.xdg)
            // A bare name must never resolve against the developer's PATH
            // leftovers; only the temp home matters here.
            .env("HOME", self._keep.path());
        command
    }
}

fn run(command: &mut Command) -> Output {
    command
        .stdin(std::process::Stdio::null())
        .output()
        .expect("pumice binary runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn bare_pumice_prints_the_overview_on_stderr_and_exits_2() {
    let home = HermeticHome::new();
    let output = run(&mut home.command(&[]));
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr(&output);
    assert!(stderr.contains("usage:"), "overview on stderr: {stderr}");
    assert!(
        stderr.contains("pumice relay"),
        "overview lists the subcommands: {stderr}"
    );
    assert!(
        stderr.contains("pumice transcribe"),
        "overview lists the subcommands: {stderr}"
    );
    assert!(
        output.stdout.is_empty(),
        "bare pumice must not write stdout: {:?}",
        output.stdout
    );
}

#[test]
fn help_prints_the_overview_on_stdout_and_version_names_pumice() {
    let home = HermeticHome::new();
    let output = run(&mut home.command(&["--help"]));
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout(&output).contains("usage:"));
    for flag in ["-h", "--help"] {
        let output = run(&mut home.command(&[flag]));
        assert_eq!(output.status.code(), Some(0), "flag {flag}");
        assert!(stdout(&output).contains("usage:"));
    }
    for flag in ["-V", "--version"] {
        let output = run(&mut home.command(&[flag]));
        assert_eq!(output.status.code(), Some(0), "flag {flag}");
        assert_eq!(
            stdout(&output).trim(),
            format!("pumice {}", env!("CARGO_PKG_VERSION")),
            "flag {flag}"
        );
    }
}

#[test]
fn unknown_command_errors_with_the_overview_and_exits_2() {
    let home = HermeticHome::new();
    let output = run(&mut home.command(&["bogus"]));
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr(&output);
    assert!(
        stderr.contains("unknown command 'bogus'"),
        "error line: {stderr}"
    );
    assert!(stderr.contains("usage:"), "overview follows: {stderr}");
}

#[test]
fn relay_check_config_reads_the_per_user_relay_yml() {
    let home = HermeticHome::new();
    let dir = home.pumice_dir();
    fs::create_dir_all(&dir).expect("create per-user config dir");
    let config = dir.join("relay.yml");
    fs::write(&config, "port: 17655\n").expect("write relay.yml");

    let output = run(&mut home.command(&["relay", "check-config"]));
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(
        stdout.contains(&format!("config: {}", config.display())),
        "the per-user relay.yml is used: {stdout}"
    );
    assert!(
        stdout.contains("port: 17655"),
        "settings come from it: {stdout}"
    );
}

#[test]
fn relay_check_config_honours_an_explicit_config_path() {
    let home = HermeticHome::new();
    let dir = TempDir::new().expect("temp dir");
    let config = dir.path().join("elsewhere.yml");
    fs::write(&config, "port: 17656\n").expect("write config");

    let output = run(&mut home.command(&[
        "relay",
        "--config",
        config.to_str().expect("UTF-8 path"),
        "check-config",
    ]));
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(
        stdout.contains(&format!("config: {}", config.display())),
        "explicit --config wins: {stdout}"
    );
    assert!(
        stdout.contains("port: 17656"),
        "settings come from it: {stdout}"
    );
}

#[test]
fn legacy_pumice_yaml_in_the_config_dir_is_ignored() {
    let home = HermeticHome::new();
    let dir = home.pumice_dir();
    fs::create_dir_all(&dir).expect("create per-user config dir");
    // Not just absent but invalid: if the relay still read the old name,
    // this would fail with a parse error instead of falling back to defaults.
    fs::write(dir.join("pumice.yaml"), "port: [unclosed\n").expect("write legacy file");

    let output = run(&mut home.command(&["relay", "check-config"]));
    assert_eq!(
        output.status.code(),
        Some(0),
        "pumice.yaml must not be read: {}",
        stderr(&output)
    );
    assert!(
        stdout(&output).contains("config: built-in defaults"),
        "the old file is ignored entirely: {}",
        stdout(&output)
    );
}

/// A temp directory holding a copy of `pumice` and, optionally, the
/// `pumice-transcribe` stand-in next to it.
struct Install {
    _keep: TempDir,
    dir: PathBuf,
}

impl Install {
    /// Copies `pumice` alone: no transcription sibling installed.
    fn without_transcribe() -> Self {
        let keep = TempDir::new().expect("temp install dir");
        let dir = keep.path().join("bin");
        fs::create_dir_all(&dir).expect("create install dir");
        fs::copy(env!("CARGO_BIN_EXE_pumice"), dir.join("pumice")).expect("copy pumice");
        Install { _keep: keep, dir }
    }

    /// Copies `pumice` together with the test stand-in as `pumice-transcribe`.
    fn with_transcribe() -> Self {
        let install = Self::without_transcribe();
        let name = transcribe_name();
        fs::copy(
            env!("CARGO_BIN_EXE_pumice-test-transcribe"),
            install.dir.join(name),
        )
        .expect("copy pumice-transcribe stand-in");
        install
    }

    fn pumice(&self) -> PathBuf {
        self.dir.join("pumice")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(self.pumice());
        command.args(args).stdin(std::process::Stdio::null());
        command
    }
}

#[cfg(windows)]
fn transcribe_name() -> &'static str {
    "pumice-transcribe.exe"
}

#[cfg(not(windows))]
fn transcribe_name() -> &'static str {
    "pumice-transcribe"
}

#[test]
fn transcribe_passes_arguments_through_and_propagates_the_exit_code() {
    let install = Install::with_transcribe();
    // The stand-in's contract: first argument is the exit code, the rest is
    // echoed one per line.
    let output = run(&mut install.command(&["transcribe", "7", "alpha", "two words"]));
    assert_eq!(output.status.code(), Some(7), "exit status propagates");
    let stdout = stdout(&output);
    assert_eq!(stdout.lines().collect::<Vec<_>>(), ["alpha", "two words"]);
}

#[test]
fn transcribe_forwards_help_verbatim() {
    let install = Install::with_transcribe();
    let output = run(&mut install.command(&["transcribe", "0", "--help"]));
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stdout(&output).lines().collect::<Vec<_>>(), ["--help"]);
}

#[test]
fn transcribe_without_a_sibling_fails_with_an_install_message() {
    let install = Install::without_transcribe();
    let output = run(&mut install.command(&["transcribe", "--version"]));
    assert_eq!(output.status.code(), Some(1));
    let stderr = stderr(&output);
    assert!(
        stderr.contains("pumice-transcribe is not installed next to pumice"),
        "names the problem: {stderr}"
    );
    assert!(
        stderr.contains(&install.dir.display().to_string()),
        "names the directory it looked in: {stderr}"
    );
}

#[test]
fn transcribe_neither_searches_path_nor_the_working_directory() {
    let install = Install::without_transcribe();
    // Decoys that a PATH or cwd lookup would find: the stand-in echoes its
    // arguments, so any use of a decoy is visible in the output.
    let path_dir = install._keep.path().join("path-decoy");
    let cwd = install._keep.path().join("cwd-decoy");
    fs::create_dir_all(&path_dir).expect("create PATH decoy dir");
    fs::create_dir_all(&cwd).expect("create cwd decoy dir");
    fs::copy(
        env!("CARGO_BIN_EXE_pumice-test-transcribe"),
        path_dir.join(transcribe_name()),
    )
    .expect("copy PATH decoy");
    fs::copy(
        env!("CARGO_BIN_EXE_pumice-test-transcribe"),
        cwd.join(transcribe_name()),
    )
    .expect("copy cwd decoy");

    let mut command = install.command(&["transcribe", "0"]);
    command.env("PATH", &path_dir).current_dir(&cwd);
    let output = run(&mut command);
    assert_eq!(
        output.status.code(),
        Some(1),
        "no decoy may satisfy the lookup: {}",
        stdout(&output)
    );
    let stderr = stderr(&output);
    assert!(
        stderr.contains("pumice-transcribe is not installed next to pumice"),
        "the sibling rule holds: {stderr}"
    );
    assert!(
        output.stdout.is_empty(),
        "no decoy may run: {:?}",
        output.stdout
    );
}
