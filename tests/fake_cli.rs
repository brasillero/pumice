//! Tests for the portable fake CLI (`pumice-test-cli`) and its test helpers.
//!
//! These spawn the fake directly with `std::process::Command`; no shell and no
//! real AI CLI is involved, so they run the same on Windows, Linux and macOS.

mod support;

use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::{FAKE_CLI_BIN, FakeCli, fixture};
use tempfile::TempDir;

/// Runs `exe` with `args`, feeding `stdin`, in `cwd` (if given).
fn run(exe: &Path, args: &[&str], stdin: &str, cwd: Option<&Path>) -> Output {
    let mut command = Command::new(exe);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let mut child = command.spawn().expect("spawn fake CLI");
    let mut child_stdin = child.stdin.take().expect("stdin pipe");
    let input = stdin.as_bytes().to_vec();
    // Write from a thread so a fake that does not read stdin cannot deadlock us.
    let writer = thread::spawn(move || {
        let _ = child_stdin.write_all(&input);
    });
    let output = child.wait_with_output().expect("wait for fake CLI");
    writer.join().expect("stdin writer thread");
    output
}

fn same_dir(a: &Path, b: &Path) -> bool {
    fs::canonicalize(a).expect("canonicalize") == fs::canonicalize(b).expect("canonicalize")
}

#[test]
fn passes_through_stdout_stderr_and_exit_code() {
    let fake = FakeCli::new(json!({
        "stdout": "out: ünïcödé ✓\nsecond line\n",
        "stderr": "err: something failed\n",
        "exit_code": 3,
    }));
    let output = run(fake.path(), &[], "", None);
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "out: ünïcödé ✓\nsecond line\n"
    );
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "err: something failed\n"
    );
}

#[test]
fn defaults_to_empty_output_and_success() {
    let fake = FakeCli::new(json!({}));
    let output = run(fake.path(), &[], "ignored", None);
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn reports_argv_stdin_cwd_and_env() {
    let fake = FakeCli::new(json!({
        "report_env": ["PUMICE_FAKE_SET", "PUMICE_FAKE_UNSET"],
    }));
    let workspace = TempDir::new().unwrap();

    let output = Command::new(fake.path())
        .args(["-p", "--tools", "", "--model", "haiku", "spaced arg", "ünï"])
        .current_dir(workspace.path())
        .env("PUMICE_FAKE_SET", "0")
        .env_remove("PUMICE_FAKE_UNSET")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .take()
                .unwrap()
                .write_all("dictation: ünïcödé\nline two".as_bytes())?;
            child.wait_with_output()
        })
        .expect("run fake CLI");
    assert!(output.status.success(), "{output:?}");

    let report = fake.report();
    assert_eq!(
        report["argv"],
        json!(["-p", "--tools", "", "--model", "haiku", "spaced arg", "ünï"])
    );
    assert_eq!(report["stdin"], json!("dictation: ünïcödé\nline two"));
    assert!(same_dir(
        Path::new(report["cwd"].as_str().unwrap()),
        workspace.path()
    ));
    assert_eq!(report["cwd_entries"], json!([]));
    assert_eq!(
        report["env"],
        json!({"PUMICE_FAKE_SET": "0", "PUMICE_FAKE_UNSET": null})
    );
    assert_eq!(report["grandchild_pid"], Value::Null);
    assert!(report["pid"].as_u64().is_some());
    // The report lives next to the fake, outside the workspace.
    assert!(fake.report_path().starts_with(fake.dir()));
    assert_eq!(fs::read_dir(workspace.path()).unwrap().count(), 0);
}

#[test]
fn reports_non_empty_working_directory() {
    let fake = FakeCli::new(json!({}));
    let workspace = TempDir::new().unwrap();
    fs::write(workspace.path().join("b.txt"), "b").unwrap();
    fs::create_dir(workspace.path().join("a-dir")).unwrap();

    let output = run(fake.path(), &[], "", Some(workspace.path()));
    assert!(output.status.success());
    assert_eq!(fake.report()["cwd_entries"], json!(["a-dir", "b.txt"]));
}

#[test]
fn reports_files_named_by_flags() {
    let fake = FakeCli::new(json!({
        "report_arg_files": ["--system-prompt-file", "--absent-flag", "--missing-file"],
    }));
    let control = TempDir::new().unwrap();
    let prompt = control.path().join("SYSTEM.txt");
    fs::write(&prompt, "You format dictated text.").unwrap();
    let missing = control.path().join("missing.txt");

    let output = run(
        fake.path(),
        &[
            "--system-prompt-file",
            prompt.to_str().unwrap(),
            "--missing-file",
            missing.to_str().unwrap(),
        ],
        "",
        None,
    );
    assert!(output.status.success());

    let files = &fake.report()["arg_files"];
    assert_eq!(
        files["--system-prompt-file"],
        json!({"path": prompt.to_str().unwrap(), "contents": "You format dictated text."})
    );
    assert_eq!(files["--absent-flag"], Value::Null);
    assert_eq!(
        files["--missing-file"],
        json!({"path": missing.to_str().unwrap(), "contents": null})
    );
}

#[test]
fn read_stdin_false_never_reads_input() {
    let fake = FakeCli::new(json!({
        "read_stdin": false,
        "stdout": "done",
    }));
    // Keep stdin open and never write: a fake that read stdin would hang.
    let mut child = Command::new(fake.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _stdin = child.stdin.take().unwrap();
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(stdout, "done");
    assert_eq!(fake.report()["stdin"], Value::Null);
}

#[test]
fn writes_filler_after_stdout() {
    let fake = FakeCli::new(json!({
        "stdout": "head",
        "stdout_repeat_bytes": 1_048_576,
    }));
    let output = run(fake.path(), &[], "", None);
    assert!(output.status.success());
    assert_eq!(output.stdout.len(), 4 + 1_048_576);
    assert!(output.stdout.starts_with(b"head"));
    assert!(output.stdout[4..].iter().all(|&b| b == b'x'));
}

#[test]
fn sleeping_fake_can_be_killed() {
    let fake = FakeCli::new(json!({
        "sleep_ms": 30_000,
        "stdout": "too late",
    }));
    let start = Instant::now();
    let mut child = Command::new(fake.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // The report is written before the sleep; wait for it so the kill lands
    // while the fake is sleeping.
    while !fake.report_path().exists() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "fake never wrote its report"
        );
        thread::sleep(Duration::from_millis(10));
    }
    child.kill().unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(start.elapsed() < Duration::from_secs(20));
}

#[test]
fn grandchild_keeps_pipes_open_after_parent_exits() {
    const GRANDCHILD_MS: u64 = 800;
    let fake = FakeCli::new(json!({
        "stdout": "parent output",
        "spawn_grandchild_sleep_ms": GRANDCHILD_MS,
    }));
    let start = Instant::now();
    let mut child = Command::new(fake.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout_pipe = child.stdout.take().unwrap();
    let mut stderr_pipe = child.stderr.take().unwrap();

    let status = child.wait().unwrap();
    let parent_exited = start.elapsed();
    assert!(status.success());

    let mut stdout = String::new();
    stdout_pipe.read_to_string(&mut stdout).unwrap();
    let stdout_eof = start.elapsed();
    let mut stderr = String::new();
    stderr_pipe.read_to_string(&mut stderr).unwrap();

    assert_eq!(stdout, "parent output");
    assert!(stderr.is_empty(), "stderr: {stderr}");
    assert!(
        stdout_eof >= Duration::from_millis(GRANDCHILD_MS),
        "stdout reached EOF after {stdout_eof:?}, before the grandchild finished"
    );
    assert!(
        parent_exited < stdout_eof,
        "parent exited at {parent_exited:?}, EOF at {stdout_eof:?}"
    );

    let report = fake.report();
    let grandchild = report["grandchild_pid"].as_u64().expect("grandchild pid");
    assert_ne!(Some(grandchild), report["pid"].as_u64());
}

#[test]
fn grandchild_flag_sleeps_without_scenario() {
    // The original binary has no scenario next to it; grandchild mode must
    // not need one.
    let output = Command::new(FAKE_CLI_BIN)
        .args(["--pumice-test-grandchild", "10"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn missing_scenario_exits_97() {
    let output = run(Path::new(FAKE_CLI_BIN), &["-p"], "", None);
    assert_eq!(output.status.code(), Some(97));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("scenario file not found"), "{stderr}");
}

#[test]
fn unknown_scenario_field_exits_98() {
    let fake = FakeCli::new(json!({"stdout": "x", "no_such_field": true}));
    let output = run(fake.path(), &[], "", None);
    assert_eq!(output.status.code(), Some(98));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("no_such_field"), "{stderr}");
    assert!(output.stdout.is_empty());
}

#[test]
fn replays_claude_fixtures() {
    let success = fixture("claude/success.json");
    let fake = FakeCli::new(json!({"stdout": success}));
    let output = run(fake.path(), &["-p", "--output-format", "json"], "", None);
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout.clone()).unwrap(), success);
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["is_error"], json!(false));
    assert_eq!(envelope["result"], json!("Formatted text."));

    let not_logged_in = fixture("claude/not-logged-in.json");
    let envelope: Value = serde_json::from_str(&not_logged_in).unwrap();
    assert_eq!(envelope["subtype"], json!("success"));
    assert_eq!(envelope["is_error"], json!(true));
    assert_eq!(
        envelope["result"],
        json!("Not logged in · Please run /login")
    );
}

#[test]
fn replays_codex_fixtures() {
    let parse = |text: &str| -> Vec<Value> {
        text.lines()
            .map(|line| serde_json::from_str(line).expect("one JSON object per line"))
            .collect()
    };

    let success = fixture("codex/success.jsonl");
    let fake = FakeCli::new(json!({"stdout": success}));
    let output = run(fake.path(), &["exec", "--json", "-"], "dictation", None);
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), success);
    let events = parse(&success);
    let types: Vec<&str> = events.iter().map(|e| e["type"].as_str().unwrap()).collect();
    assert_eq!(
        types,
        [
            "thread.started",
            "turn.started",
            "item.completed",
            "turn.completed"
        ]
    );
    assert_eq!(
        events[2]["item"],
        json!({"id": "item_0", "type": "agent_message", "text": "Formatted text."})
    );

    let unauthorized = parse(&fixture("codex/unauthorized.jsonl"));
    let last = unauthorized.last().unwrap();
    assert_eq!(last["type"], json!("turn.failed"));
    assert!(
        last["error"]["message"]
            .as_str()
            .unwrap()
            .contains("401 Unauthorized: Missing bearer or basic authentication")
    );
    assert!(unauthorized.iter().any(|e| e["type"] == json!("error")));
}
