//! Integration tests for `pumice claude-probe` logic using a fake CLI.
//!
//! The fake CLI is a small shell script (hence `#[cfg(unix)]`) that verifies
//! the probe spawns it correctly: `-p` first, an EMPTY `--tools` argument, a
//! fresh empty working directory, and dictation arriving on stdin. It prints
//! the JSON shape the real `claude -p --output-format json` prints.

use std::path::PathBuf;
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

use pumice::probe::{ProbeError, ProbeOptions, run_probe};

#[cfg(unix)]
use std::{fs, os::unix::fs::PermissionsExt};

/// The fake claude: asserts the probe's spawn contract, then echoes stdin
/// uppercased inside a Claude-Code-shaped JSON envelope.
#[cfg(unix)]
const FAKE_CLAUDE: &str = r#"#!/bin/sh
[ "$1" = "-p" ] || exit 11
tools_empty=no; prompt=no; strict=no
while [ $# -gt 0 ]; do
  case "$1" in
    --tools) [ -z "$2" ] && tools_empty=yes; shift ;;
    --system-prompt) [ -n "$2" ] && prompt=yes; shift ;;
    --strict-mcp-config) strict=yes ;;
  esac
  shift
done
[ "$tools_empty" = yes ] || exit 15
[ "$strict" = yes ] || exit 16
[ "$prompt" = yes ] || exit 19
[ -z "$(ls -A .)" ] || exit 20
input=$(cat)
upper=$(printf '%s' "$input" | tr '[:lower:]' '[:upper:]')
printf '{"type":"result","is_error":false,"result":"%s"}\n' "$upper"
"#;

#[cfg(unix)]
fn write_script(dir: &tempfile::TempDir, name: &str, body: &str) -> PathBuf {
    let path = dir.path().join(name);
    fs::write(&path, body).expect("write script");
    let mut perms = fs::metadata(&path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).expect("chmod");
    path
}

#[cfg(unix)]
#[test]
fn probe_formats_text_with_fake_claude() {
    let dir = tempfile::tempdir().expect("temp dir");
    let fake = write_script(&dir, "fake-claude", FAKE_CLAUDE);

    let options = ProbeOptions {
        text: "hello pumice probe".to_string(),
        claude_bin: fake,
        timeout: Duration::from_secs(10),
    };
    let result = run_probe(&options).expect("probe succeeds");

    // The fake CLI uppercases stdin; proves the text went through stdin and
    // the JSON result field was extracted.
    assert_eq!(result.text, "HELLO PUMICE PROBE");
    assert_eq!(result.is_error, Some(false));
    assert!(result.exit_status.success());
}

#[test]
fn probe_reports_missing_claude_as_not_installed() {
    let options = ProbeOptions {
        text: "x".to_string(),
        claude_bin: PathBuf::from("/definitely/not/an/existing/claude-binary-xyz"),
        timeout: Duration::from_secs(5),
    };
    let err = run_probe(&options).expect_err("must fail");
    assert!(
        matches!(err, ProbeError::NotInstalled),
        "expected NotInstalled, got: {err}"
    );
}

#[cfg(unix)]
#[test]
fn probe_times_out_a_hanging_cli() {
    let dir = tempfile::tempdir().expect("temp dir");
    let sleeper = write_script(&dir, "slow-claude", "#!/bin/sh\nsleep 60\n");

    let options = ProbeOptions {
        text: "x".to_string(),
        claude_bin: sleeper,
        timeout: Duration::from_secs(1),
    };
    let started = Instant::now();
    let err = run_probe(&options).expect_err("must time out");
    assert!(
        matches!(err, ProbeError::Timeout { .. }),
        "expected Timeout, got: {err}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "timeout must actually kill the child"
    );
}

#[cfg(unix)]
#[test]
fn probe_surfaces_nonzero_exit_with_stderr() {
    let dir = tempfile::tempdir().expect("temp dir");
    let failing = write_script(
        &dir,
        "failing-claude",
        "#!/bin/sh\necho broken >&2\nexit 3\n",
    );

    let options = ProbeOptions {
        text: "x".to_string(),
        claude_bin: failing,
        timeout: Duration::from_secs(10),
    };
    let err = run_probe(&options).expect_err("must fail");
    match err {
        ProbeError::Exited { status, stderr } => {
            assert_eq!(status.code(), Some(3));
            assert!(stderr.contains("broken"), "stderr included: {stderr}");
        }
        other => panic!("expected Exited, got: {other}"),
    }
}

#[cfg(unix)]
#[test]
fn probe_surfaces_is_error_envelope_on_stdout() {
    // Real Claude prints e.g. "Not logged in" as a JSON envelope on stdout
    // with is_error: true and exit code 1, leaving stderr empty.
    let dir = tempfile::tempdir().expect("temp dir");
    let script = "#!/bin/sh\ncat >/dev/null\nprintf '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":true,\"result\":\"Not logged in\"}'\nexit 1\n";
    let fake = write_script(&dir, "logged-out-claude", script);

    let options = ProbeOptions {
        text: "x".to_string(),
        claude_bin: fake,
        timeout: Duration::from_secs(10),
    };
    match run_probe(&options).expect_err("must fail") {
        ProbeError::Exited { stderr, .. } => {
            assert!(
                stderr.contains("Not logged in"),
                "message included: {stderr}"
            );
        }
        other => panic!("expected Exited, got: {other}"),
    }
}
