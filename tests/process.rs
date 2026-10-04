//! Tests for the shared process runner, through the portable fake CLI.

mod support;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use pumice::process::{
    Argument, CliInvocation, ControlFile, MAX_STDOUT_BYTES, ProcessOutput, ProcessRunner,
    ProgramSpec,
};
use pumice::providers::{ProviderError, ProviderErrorCode};
use serde_json::{Value, json};
use support::FakeCli;
use tokio::time::Instant;

/// Generous budget so slow CI machines never hit it by accident.
const RELAXED: Duration = Duration::from_secs(30);
/// Allowed lateness for timing assertions on CI.
const TOLERANCE: Duration = Duration::from_secs(2);

fn unused_parser(_: &ProcessOutput) -> Result<String, ProviderError> {
    Ok(String::new())
}

fn invocation(program: &Path) -> CliInvocation {
    CliInvocation {
        program: ProgramSpec {
            binary: program.to_path_buf(),
            npm_entrypoint: None,
        },
        args: Vec::new(),
        stdin: Vec::new(),
        env: BTreeMap::new(),
        remove_env: Vec::new(),
        control_files: Vec::new(),
        parser: unused_parser,
    }
}

fn literals(args: &[&str]) -> Vec<Argument> {
    args.iter().map(|a| Argument::literal(*a)).collect()
}

async fn run(invocation: CliInvocation) -> Result<ProcessOutput, ProviderError> {
    ProcessRunner::new()
        .run(invocation, Instant::now() + RELAXED)
        .await
}

/// The temporary root the runner created, derived from the reported cwd
/// (`<root>/workspace`).
fn temp_root(report: &Value) -> PathBuf {
    let cwd = Path::new(report["cwd"].as_str().expect("cwd in report"));
    assert_eq!(cwd.file_name().unwrap(), "workspace");
    cwd.parent().expect("workspace has a parent").to_path_buf()
}

#[tokio::test]
async fn delivers_stdin_and_returns_stdout() {
    let fake = FakeCli::new(json!({"stdout": "formatted ✓", "stderr": "warning"}));
    let mut inv = invocation(fake.path());
    inv.stdin = "dictation: ünïcödé\nline two".as_bytes().to_vec();

    let output = run(inv).await.expect("run succeeds");
    assert!(output.status.success());
    assert_eq!(output.stdout, "formatted ✓".as_bytes());
    assert_eq!(output.stderr_tail, b"warning");
    assert_eq!(
        fake.report()["stdin"],
        json!("dictation: ünïcödé\nline two")
    );
}

#[tokio::test]
async fn reports_nonzero_exit_status() {
    let fake = FakeCli::new(json!({"stdout": "partial", "exit_code": 3}));
    let output = run(invocation(fake.path())).await.expect("run completes");
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(output.stdout, b"partial");
}

#[tokio::test]
async fn workspace_is_empty_and_control_file_is_outside_it() {
    let fake = FakeCli::new(json!({"report_arg_files": ["--system-prompt-file"]}));
    let mut inv = invocation(fake.path());
    inv.args = vec![
        Argument::literal("--system-prompt-file"),
        Argument::ControlPath { file: 0 },
    ];
    inv.control_files = vec![ControlFile {
        name: "system.txt",
        contents: "You format dictated text. ✓".as_bytes().to_vec(),
    }];

    run(inv).await.expect("run succeeds");
    let report = fake.report();
    assert_eq!(report["cwd_entries"], json!([]));

    let file = &report["arg_files"]["--system-prompt-file"];
    assert_eq!(file["contents"], json!("You format dictated text. ✓"));
    let path = PathBuf::from(file["path"].as_str().unwrap());
    assert!(path.is_absolute(), "{}", path.display());
    let control_dir = path.parent().unwrap();
    assert_eq!(control_dir.file_name().unwrap(), "control");
    // Side by side under the same temporary root. Compared by the root's
    // unique name: the fake reports a canonical cwd (e.g. /private/var on
    // macOS) and the root no longer exists to canonicalize.
    let root = temp_root(&report);
    assert_eq!(
        control_dir.parent().unwrap().file_name(),
        root.file_name(),
        "control dir and workspace share the temporary root"
    );
    assert!(!path.starts_with(report["cwd"].as_str().unwrap()));
    // Removed with the temporary root.
    assert!(!path.exists());
}

#[tokio::test]
async fn empty_and_unusual_arguments_survive() {
    let fake = FakeCli::new(json!({}));
    let mut inv = invocation(fake.path());
    inv.args = literals(&["--tools", "", "spaced arg", "ünï 🎤", "\"quoted\"", "a&b|c"]);

    run(inv).await.expect("run succeeds");
    assert_eq!(
        fake.report()["argv"],
        json!(["--tools", "", "spaced arg", "ünï 🎤", "\"quoted\"", "a&b|c"])
    );
}

#[tokio::test]
async fn applies_and_removes_environment_variables() {
    let fake = FakeCli::new(json!({"report_env": ["PUMICE_RUNNER_SET", "PATH"]}));
    let mut inv = invocation(fake.path());
    inv.env = BTreeMap::from([(
        OsString::from("PUMICE_RUNNER_SET"),
        OsString::from("value ✓"),
    )]);
    inv.remove_env = vec![OsString::from("PATH")];

    run(inv).await.expect("run succeeds");
    assert_eq!(
        fake.report()["env"],
        json!({"PUMICE_RUNNER_SET": "value ✓", "PATH": null})
    );
}

#[tokio::test]
async fn missing_program_is_not_installed() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("no-such-cli");
    let result = run(invocation(&missing)).await;
    assert_eq!(result.unwrap_err(), ProviderError::NotInstalled);
}

#[tokio::test]
async fn cli_that_never_reads_stdin_still_finishes() {
    let fake = FakeCli::new(json!({"read_stdin": false, "stdout": "done"}));
    let mut inv = invocation(fake.path());
    // Larger than any pipe buffer, so the write cannot complete unread.
    inv.stdin = vec![b'a'; 4 * 1024 * 1024];

    let start = Instant::now();
    let output = run(inv).await.expect("run succeeds");
    assert!(output.status.success());
    assert_eq!(output.stdout, b"done");
    assert!(start.elapsed() < Duration::from_secs(10));
    assert_eq!(fake.report()["stdin"], Value::Null);
}

#[tokio::test]
async fn stdout_at_the_limit_is_accepted() {
    let fake = FakeCli::new(json!({"stdout_repeat_bytes": MAX_STDOUT_BYTES}));
    let output = run(invocation(fake.path())).await.expect("run succeeds");
    assert_eq!(output.stdout.len(), MAX_STDOUT_BYTES);
}

#[tokio::test]
async fn stdout_over_the_limit_is_too_large() {
    let fake = FakeCli::new(json!({"stdout_repeat_bytes": MAX_STDOUT_BYTES + 1}));
    let result = run(invocation(fake.path())).await;
    assert_eq!(
        result.unwrap_err(),
        ProviderError::Other {
            code: ProviderErrorCode::OutputTooLarge
        }
    );
}

#[tokio::test]
async fn stderr_keeps_only_the_tail() {
    let mut stderr = "x".repeat(100 * 1024);
    stderr.push_str("END");
    let fake = FakeCli::new(json!({"stderr": stderr}));
    let output = run(invocation(fake.path())).await.expect("run succeeds");
    assert_eq!(output.stderr_tail.len(), 64 * 1024);
    assert!(output.stderr_tail.ends_with(b"END"));
}

#[tokio::test]
async fn deadline_exceeded_is_timeout() {
    // Does not read the report: on a slow runner the deadline can kill the
    // fake before it writes one. Cleanup after a started CLI is checked by
    // the grandchild and cancellation tests, which wait for the report.
    let fake = FakeCli::new(json!({"sleep_ms": 30_000, "stdout": "too late"}));
    let budget = Duration::from_millis(500);

    let start = Instant::now();
    let result = ProcessRunner::new()
        .run(invocation(fake.path()), start + budget)
        .await;
    let elapsed = start.elapsed();

    assert_eq!(result.unwrap_err(), ProviderError::Timeout);
    assert!(elapsed < budget + TOLERANCE, "took {elapsed:?}");
}

#[tokio::test]
async fn cancelled_run_kills_the_whole_tree() {
    const SLEEP_MS: u64 = 30_000;
    let fake = FakeCli::new(json!({
        "sleep_ms": SLEEP_MS,
        "spawn_grandchild_sleep_ms": SLEEP_MS,
    }));
    let inv = invocation(fake.path());
    let task = tokio::spawn(async move {
        ProcessRunner::new()
            .run(inv, Instant::now() + Duration::from_millis(SLEEP_MS))
            .await
    });

    // The fake writes its report (with the grandchild PID) before sleeping.
    let started_by = std::time::Instant::now() + RELAXED;
    while !fake.report_path().exists() {
        assert!(std::time::Instant::now() < started_by, "fake never started");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let report = fake.report();
    let parent = report["pid"].as_u64().expect("pid") as u32;
    let grandchild = report["grandchild_pid"].as_u64().expect("grandchild pid") as u32;

    // Cancel the run, as the pipeline does on its total deadline or when the
    // client disconnects.
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());

    assert!(process_alive(std::process::id()));
    wait_until_gone(parent);
    wait_until_gone(grandchild);
}

#[tokio::test]
async fn returns_promptly_and_kills_grandchild_holding_pipes() {
    const GRANDCHILD_MS: u64 = 20_000;
    let fake = FakeCli::new(json!({
        "stdout": "parent output",
        "spawn_grandchild_sleep_ms": GRANDCHILD_MS,
    }));

    let start = Instant::now();
    let output = run(invocation(fake.path())).await.expect("run succeeds");
    let elapsed = start.elapsed();

    assert!(output.status.success());
    assert_eq!(output.stdout, b"parent output");
    assert!(elapsed < Duration::from_secs(5), "took {elapsed:?}");

    let pid = fake.report()["grandchild_pid"]
        .as_u64()
        .expect("grandchild pid") as u32;
    // Guard against a liveness check that always says "gone".
    assert!(process_alive(std::process::id()));
    wait_until_gone(pid);
}

#[tokio::test]
async fn temp_root_is_removed_afterwards() {
    let fake = FakeCli::new(json!({"stdout": "ok"}));
    let mut inv = invocation(fake.path());
    inv.control_files = vec![ControlFile {
        name: "system.txt",
        contents: b"prompt".to_vec(),
    }];
    run(inv).await.expect("run succeeds");
    let root = temp_root(&fake.report());
    assert!(!root.exists(), "{} still exists", root.display());
}

#[tokio::test]
async fn control_file_names_must_be_plain() {
    let fake = FakeCli::new(json!({}));
    let mut inv = invocation(fake.path());
    inv.control_files = vec![ControlFile {
        name: "../escape.txt",
        contents: Vec::new(),
    }];
    assert_eq!(
        run(inv).await.unwrap_err(),
        ProviderError::Other {
            code: ProviderErrorCode::InvalidConfiguration
        }
    );
    assert!(!fake.report_path().exists(), "the CLI must not start");
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
async fn past_deadline_never_starts_the_cli() {
    let fake = FakeCli::new(json!({}));
    let result = ProcessRunner::new()
        .run(invocation(fake.path()), Instant::now())
        .await;
    assert_eq!(result.unwrap_err(), ProviderError::Timeout);
    assert!(!fake.report_path().exists());
}
