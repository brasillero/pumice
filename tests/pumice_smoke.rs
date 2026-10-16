//! CLI tests for the `pumice` dispatcher binary.
//!
//! The relay's full test suite lives in `crates/relay/tests`; this file
//! verifies the orchestrator entry point: the bare-command contract (D1),
//! the `relay` subcommand against a hermetic config directory, the
//! `relay.yml` rename (D3) and the `transcribe` sibling dispatch (D2: it runs
//! only the sibling next to `pumice`; D4: on Unix it execs it, so signals and
//! exit codes are native). No provider CLI is ever invoked — `check-config`
//! only validates YAML.

#[path = "../crates/relay/tests/support/install.rs"]
mod install;
use install::install_executable;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
#[cfg(unix)]
use std::time::{Duration, Instant};

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

/// Runs the command with `input` on stdin.
fn run_with_stdin(command: &mut Command, input: &[u8]) -> Output {
    use std::io::Write;

    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("pumice binary runs");
    child
        .stdin
        .as_mut()
        .expect("stdin is piped")
        .write_all(input)
        .expect("write stdin");
    child.wait_with_output().expect("reap pumice")
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
fn format_list_runs_in_process_and_uses_the_per_user_config() {
    let home = HermeticHome::new();
    let dir = home.pumice_dir();
    fs::create_dir_all(&dir).expect("create per-user config dir");
    fs::write(
        dir.join("notes.yml"),
        "schema_version: 1\nid: notes\ndescription: Test\nsystem: RULES\n",
    )
    .expect("write definition");
    fs::write(
        dir.join("formatter.yml"),
        "schema_version: 1\nformatters:\n  - path: notes.yml\ndefault: notes\n",
    )
    .expect("write formatter.yml");

    let output = run(&mut home.command(&["format", "list"]));
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(
        stdout.contains("builtin:clean"),
        "built-ins listed: {stdout}"
    );
    assert!(
        stdout.contains("notes"),
        "configured formatters listed: {stdout}"
    );
    assert!(stdout.contains("yes"), "the default is marked: {stdout}");
}

#[test]
fn format_show_builtin_prints_an_editable_definition() {
    let home = HermeticHome::new();
    let output = run(&mut home.command(&["format", "show", "builtin:clean"]));
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains("id: my-clean"), "placeholder id: {stdout}");
    assert!(stdout.contains("system: |"), "{stdout}");
}

#[test]
fn format_show_output_reloads_to_the_exact_same_definition() {
    // The in-process entry point, like the standalone binary: `show` stdout
    // must be a definition file that reloads to the exact same text, with no
    // added or dropped trailing newline.
    let home = HermeticHome::new();
    let dir = home.pumice_dir();
    fs::create_dir_all(&dir).expect("create per-user config dir");
    fs::write(
        dir.join("notes.yml"),
        "schema_version: 1\nid: notes\ndescription: Test\nsystem: \"Rule\\n\\n\"\n",
    )
    .expect("write definition");
    fs::write(
        dir.join("formatter.yml"),
        "schema_version: 1\nformatters:\n  - path: notes.yml\n",
    )
    .expect("write formatter.yml");

    let output = run(&mut home.command(&["format", "show", "notes"]));
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let tmp = TempDir::new().expect("temp dir");
    let path = tmp.path().join("show.yml");
    fs::write(&path, &output.stdout).expect("write show output");
    let loaded =
        pumice_formatter::load_definition_file(&path).expect("show output is a definition");
    assert_eq!(loaded.id, "notes");
    assert_eq!(loaded.system, "Rule\n\n");
}

#[test]
fn format_apply_transforms_a_request_and_names_the_program_in_errors() {
    let home = HermeticHome::new();
    let request =
        br#"{"model":"claude","messages":[{"role":"user","content":"hi"}],"stream":false}"#;
    let output = run_with_stdin(
        &mut home.command(&["format", "apply", "--formatter", "builtin:clean"]),
        request,
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let transformed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(transformed["messages"][0]["role"], "system");
    assert_eq!(transformed["messages"][1]["content"], "hi");

    // The in-process CLI shows `pumice format` as its program name.
    let help = run(&mut home.command(&["format", "--help"]));
    assert_eq!(help.status.code(), Some(0));
    assert!(
        stdout(&help).contains("pumice format"),
        "help names the program: {}",
        stdout(&help)
    );
    let bad = run(&mut home.command(&["format", "show", "missing", "--json"]));
    assert_eq!(bad.status.code(), Some(2));
    let envelope: serde_json::Value = serde_json::from_slice(&bad.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "formatter_not_found");
}

#[test]
fn the_overview_lists_the_format_command() {
    let home = HermeticHome::new();
    let output = run(&mut home.command(&[]));
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("pumice format"),
        "overview lists the format subcommand: {}",
        stderr(&output)
    );
}

#[test]
fn help_after_a_relay_subcommand_prints_its_help() {
    let home = HermeticHome::new();
    for (command, usage) in [
        (
            "serve",
            "usage: pumice relay serve [--config <path>] [--plain]",
        ),
        (
            "check-config",
            "usage: pumice relay check-config [--config <path>]",
        ),
        (
            "doctor",
            "usage: pumice relay doctor [--config <path>] [--login-check --provider <id>]",
        ),
    ] {
        for flag in ["--help", "-h"] {
            let output = run(&mut home.command(&["relay", command, flag]));
            assert_eq!(
                output.status.code(),
                Some(0),
                "{command} {flag}: {}",
                stderr(&output)
            );
            assert!(
                stdout(&output).contains(usage),
                "{command} {flag}: {}",
                stdout(&output)
            );
        }
    }
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

/// The copy fallback for file systems without hard links: copied while other
/// threads keep spawning children, the copy runs at once instead of failing
/// with "text file busy" (ETXTBSY), because a `cp` child writes it.
#[cfg(unix)]
#[test]
fn the_copy_fallback_runs_at_once_while_other_threads_spawn() {
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Stops the spawning threads even when the copy panics, so the scope
    /// cannot wait for them forever.
    struct StopOnDrop<'a>(&'a AtomicBool);
    impl Drop for StopOnDrop<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }

    let dir = TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).expect("temp dir");
    let stop = AtomicBool::new(false);
    let failures = std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                while !stop.load(Ordering::Relaxed) {
                    let _ = Command::new("true").status();
                }
            });
        }
        let _stop = StopOnDrop(&stop);
        let mut failures = Vec::new();
        for round in 0..20 {
            let dest = dir.path().join(format!("pumice-{round}"));
            install::copy_executable(Path::new(env!("CARGO_BIN_EXE_pumice")), &dest);
            match Command::new(&dest).arg("--version").output() {
                Ok(output) if output.status.success() => {}
                other => failures.push(format!("round {round}: {other:?}")),
            }
            fs::remove_file(&dest).expect("remove the copy");
        }
        failures
    });
    assert!(failures.is_empty(), "{failures:#?}");
}

/// A temp directory holding a copy of `pumice` and, optionally, the
/// `pumice-transcribe` stand-in next to it.
struct Install {
    _keep: TempDir,
    dir: PathBuf,
}

impl Install {
    /// Installs `pumice` alone: no transcription sibling installed.
    fn without_transcribe() -> Self {
        let keep = TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).expect("temp install dir");
        let dir = keep.path().join("bin");
        fs::create_dir_all(&dir).expect("create install dir");
        install_executable(Path::new(env!("CARGO_BIN_EXE_pumice")), &dir.join("pumice"));
        Install { _keep: keep, dir }
    }

    /// Installs `pumice` together with the test stand-in as
    /// `pumice-transcribe`.
    fn with_transcribe() -> Self {
        let install = Self::without_transcribe();
        install_executable(
            Path::new(env!("CARGO_BIN_EXE_pumice-test-transcribe")),
            &install.dir.join(transcribe_name()),
        );
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
fn transcribe_propagates_small_and_large_child_exit_codes() {
    let install = Install::with_transcribe();
    for code in ["3", "255"] {
        let output = run(&mut install.command(&["transcribe", code]));
        assert_eq!(
            output.status.code(),
            Some(code.parse().unwrap()),
            "exit status {code} propagates"
        );
    }
}

#[cfg(windows)]
#[test]
fn transcribe_propagates_the_childs_full_exit_code_on_windows() {
    // The Windows dispatch must not truncate the child's status to eight
    // bits: 256 would become 0, 3010 would become 194.
    let install = Install::with_transcribe();
    for code in ["256", "3010"] {
        let output = run(&mut install.command(&["transcribe", code]));
        assert_eq!(
            output.status.code(),
            Some(code.parse().unwrap()),
            "the full exit status {code} must not truncate"
        );
    }
}

#[cfg(unix)]
#[test]
fn transcribe_preserves_non_utf8_arguments_byte_for_byte() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let install = Install::with_transcribe();
    // Not valid UTF-8 anywhere, and it contains a space: only byte-faithful
    // forwarding keeps it intact.
    let raw = b"file /tmp/audio-\xff.wav";
    let mut command = Command::new(install.pumice());
    command
        .args(["transcribe", "0"])
        .arg(OsStr::from_bytes(raw))
        .stdin(std::process::Stdio::null());
    let output = run(&mut command);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let mut expected = raw.to_vec();
    expected.push(b'\n');
    assert_eq!(
        output.stdout, expected,
        "the argument must arrive byte for byte: {:?}",
        output.stdout
    );

    // A non-UTF-8 first argument is an unknown command, not a panic and not
    // a Unicode validation error.
    let mut command = Command::new(install.pumice());
    command
        .arg(OsStr::from_bytes(b"\xff"))
        .stdin(std::process::Stdio::null());
    let output = run(&mut command);
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr(&output);
    assert!(stderr.contains("unknown command"), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn sigterm_right_after_start_reaches_the_execd_stub_and_orphans_nothing() {
    use std::os::unix::process::ExitStatusExt;

    let install = Install::with_transcribe();
    let pid_file = install._keep.path().join("stub.pid");
    let mut command = Command::new(install.pumice());
    command
        .args(["transcribe", "0"])
        .env("PUMICE_STUB_PID_FILE", &pid_file)
        .stdin(std::process::Stdio::null());
    let mut child = command.spawn().expect("spawn pumice");
    let pumice_pid = child.id();

    // The stub only writes this file after the exec, when `pumice`'s PID has
    // become the stub's own.
    let stub_pid = wait_for_pid_file(&pid_file, Duration::from_secs(10));
    assert_eq!(
        stub_pid,
        i64::from(pumice_pid),
        "exec replaces the process: the stub owns `pumice`'s PID"
    );

    // The same cancellation a supervisor sends to the `pumice` PID it just
    // started now lands on the stub itself.
    assert_eq!(unsafe { libc::kill(stub_pid as i32, libc::SIGTERM) }, 0);
    let status = child.wait().expect("reap the stub");
    assert_eq!(
        status.signal(),
        Some(libc::SIGTERM),
        "SIGTERM reaches the stub itself, not an abandoned wrapper"
    );

    // And nothing is left behind: the PID names no process anymore.
    let alive = unsafe { libc::kill(stub_pid as i32, 0) };
    assert_eq!(alive, -1, "no orphan remains with the stub's PID");
}

#[cfg(unix)]
fn wait_for_pid_file(path: &Path, timeout: Duration) -> i64 {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(text) = fs::read_to_string(path)
            && let Ok(pid) = text.trim().parse::<i64>()
        {
            return pid;
        }
        assert!(
            Instant::now() < deadline,
            "the stub did not record its pid within {timeout:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
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
        stderr.contains(&user_facing(&install.dir)),
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
    install_executable(
        Path::new(env!("CARGO_BIN_EXE_pumice-test-transcribe")),
        &path_dir.join(transcribe_name()),
    );
    install_executable(
        Path::new(env!("CARGO_BIN_EXE_pumice-test-transcribe")),
        &cwd.join(transcribe_name()),
    );

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

/// The install directory as `pumice` reports it: resolved (Windows expands
/// short 8.3 names) and without the Windows verbatim `\\?\` prefix.
fn user_facing(dir: &std::path::Path) -> String {
    let text = dir
        .canonicalize()
        .expect("canonicalize install dir")
        .display()
        .to_string();
    text.strip_prefix(r"\\?\")
        .map(str::to_string)
        .unwrap_or(text)
}
