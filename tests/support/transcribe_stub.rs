//! Test-only stand-in for the `pumice-transcribe` executable.
//!
//! Two modes:
//!
//! **Dispatcher mode** (no scenario file): `pumice-test-transcribe
//! [EXIT_CODE] [ARGS…]` prints every argument after EXIT_CODE on its own line
//! and exits with EXIT_CODE (default 0 when absent or unparsable), a full
//! `i32` through `process::exit` so Windows tests can exercise exit statuses
//! above 255. Unix only, when PUMICE_STUB_PID_FILE is set, the stub writes
//! its own PID there and waits forever, so a test can prove the dispatcher
//! exec'd it and deliver a signal directly.
//!
//! **Scenario mode** (a `<exe>.scenario.json` file next to the executable,
//! found through `std::env::current_exe()`, so parallel tests never share
//! state): the stub answers `pumice run`'s two calls —
//! `--config <FILE> models list --json` and
//! `--config <FILE> file <AUDIO> --model <ID> --json` — from the scenario:
//!
//! ```json
//! {
//!   "report_path": "/tmp/file-report.json",
//!   "audio_arg_path": "/tmp/audio-arg.bin",
//!   "pid_path": "/tmp/stub-pids.json",
//!   "models_list": {"stdout": "...", "exit_code": 0},
//!   "file": {"stdout": "...", "exit_code": 0, "sleep_ms": 30000,
//!            "spawn_grandchild_sleep_ms": 30000, "crash": false}
//! }
//! ```
//!
//! Per call: write the pid file (own PID, then the grandchild's), write the
//! argv report, sleep, abort when `crash` is set, write stdout (plus
//! `stdout_repeat_bytes` filler) and stderr, and exit with `exit_code`. A
//! grandchild inherits stdout/stderr, so it holds the caller's pipes open
//! after the stub exits.

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::exit;

/// Suffix appended to the executable path to find the scenario file.
const SCENARIO_SUFFIX: &str = ".scenario.json";
/// Argv flag that switches the stub into grandchild mode.
const GRANDCHILD_FLAG: &str = "--pumice-stub-grandchild";
/// Exit code when the scenario file cannot be read or parsed.
const EXIT_BAD_SCENARIO: i32 = 98;

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();

    if args.first().map(OsString::as_os_str) == Some(OsStr::new(GRANDCHILD_FLAG)) {
        let ms = args
            .get(1)
            .and_then(|arg| arg.to_string_lossy().parse::<u64>().ok())
            .expect("grandchild sleep milliseconds");
        std::thread::sleep(std::time::Duration::from_millis(ms));
        exit(0);
    }

    let exe = std::env::current_exe().expect("locate own executable");
    let mut scenario_path = exe.into_os_string();
    scenario_path.push(SCENARIO_SUFFIX);
    let scenario_path = PathBuf::from(scenario_path);
    if scenario_path.is_file() {
        scenario_mode(&args, &scenario_path);
    }
    dispatcher_mode(args);
}

/// Legacy dispatcher contract: `[EXIT_CODE] [ARGS…]`.
fn dispatcher_mode(args: Vec<OsString>) -> ! {
    let mut args = args.into_iter();
    let code: i32 = args
        .next()
        .and_then(|arg| arg.into_string().ok())
        .and_then(|arg| arg.parse().ok())
        .unwrap_or(0);

    #[cfg(unix)]
    if let Some(pid_file) = std::env::var_os("PUMICE_STUB_PID_FILE") {
        std::fs::write(pid_file, format!("{}\n", std::process::id()))
            .expect("write the stub pid file");
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }

    let mut stdout = std::io::stdout().lock();
    for arg in args {
        write_arg(&mut stdout, &arg);
        stdout.write_all(b"\n").expect("write argument line");
    }
    drop(stdout);
    exit(code);
}

/// Writes the argument exactly as it arrived, byte for byte where the
/// platform exposes the raw bytes; a lossy conversion would hide a
/// dispatcher bug that mangles non-UTF-8 arguments.
#[cfg(unix)]
fn write_arg(stdout: &mut impl Write, arg: &OsStr) {
    use std::os::unix::ffi::OsStrExt;
    stdout.write_all(arg.as_bytes()).expect("write argument");
}

#[cfg(not(unix))]
fn write_arg(stdout: &mut impl Write, arg: &OsStr) {
    stdout
        .write_all(arg.to_string_lossy().as_bytes())
        .expect("write argument");
}

/// One scripted child behavior (`models_list` or `file`).
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Behavior {
    /// Written to stdout as-is.
    #[serde(default)]
    stdout: String,
    /// Number of filler bytes written to stdout after `stdout`.
    #[serde(default)]
    stdout_repeat_bytes: Option<u64>,
    /// Written to stderr as-is.
    #[serde(default)]
    stderr: String,
    /// Process exit code.
    #[serde(default)]
    exit_code: i32,
    /// Delay before writing any output.
    #[serde(default)]
    sleep_ms: Option<u64>,
    /// Spawn a grandchild that inherits stdout/stderr and sleeps this long.
    #[serde(default)]
    spawn_grandchild_sleep_ms: Option<u64>,
    /// Abort (SIGABRT) after the delay instead of exiting normally.
    #[serde(default)]
    crash: bool,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    /// JSON report of the `file` call's arguments.
    #[serde(default)]
    report_path: Option<PathBuf>,
    /// JSON report of the `models list` call's arguments.
    #[serde(default)]
    models_report_path: Option<PathBuf>,
    /// JSON `{"pid": N, "grandchild_pid": M}` of the current call.
    #[serde(default)]
    pid_path: Option<PathBuf>,
    /// The raw bytes of the `file` call's audio argument (path arguments are
    /// not always UTF-8).
    #[serde(default)]
    audio_arg_path: Option<PathBuf>,
    #[serde(default)]
    models_list: Behavior,
    #[serde(default)]
    file: Behavior,
}

fn scenario_mode(args: &[OsString], scenario_path: &Path) -> ! {
    let text = std::fs::read_to_string(scenario_path).unwrap_or_else(|e| {
        eprintln!("pumice-test-transcribe: cannot read scenario: {e}");
        exit(EXIT_BAD_SCENARIO);
    });
    let scenario: Scenario = serde_json::from_str(&text).unwrap_or_else(|e| {
        eprintln!("pumice-test-transcribe: invalid scenario: {e}");
        exit(EXIT_BAD_SCENARIO);
    });

    // `pumice run` always calls with `--config <FILE>` first.
    let config = (args.first().map(OsString::as_os_str) == Some(OsStr::new("--config")))
        .then(|| args.get(1).cloned())
        .flatten();
    let rest = if config.is_some() { &args[2..] } else { args };

    let (behavior, report_path) = match rest.first().map(OsString::as_os_str) {
        Some(name) if name == OsStr::new("models") => {
            (&scenario.models_list, &scenario.models_report_path)
        }
        Some(name) if name == OsStr::new("file") => (&scenario.file, &scenario.report_path),
        _ => {
            eprintln!("pumice-test-transcribe: unknown subcommand in scenario mode");
            exit(EXIT_BAD_SCENARIO);
        }
    };

    if let Some(path) = &scenario.pid_path {
        write_pid_file(path, None);
    }

    // Spawning is the side effect here; the detached grandchild keeps the
    // caller's pipes open after this process exits.
    let _ = behavior.spawn_grandchild_sleep_ms.map(|ms| {
        let exe = std::env::current_exe().expect("locate own executable");
        let pid = std::process::Command::new(exe)
            .arg(GRANDCHILD_FLAG)
            .arg(ms.to_string())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .expect("spawn grandchild")
            .id();
        if let Some(path) = &scenario.pid_path {
            write_pid_file(path, Some(pid));
        }
        pid
    });

    if let Some(path) = report_path {
        let report = serde_json::json!({
            "argv": args.iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>(),
            "config": config.map(|c| c.to_string_lossy().into_owned()),
        });
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap())
            .expect("write the stub report");
    }

    if let Some(path) = &scenario.audio_arg_path {
        // The argument right after `file`; raw bytes, not a lossy string.
        let audio = rest.get(1).expect("file call has an audio argument");
        write_arg_bytes(path, audio);
    }

    if let Some(ms) = behavior.sleep_ms {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }

    if behavior.crash {
        // A hard crash: no output, no exit status handling.
        std::process::abort();
    }

    let mut out = std::io::stdout().lock();
    let _ = out.write_all(behavior.stdout.as_bytes());
    if let Some(n) = behavior.stdout_repeat_bytes {
        let chunk = [b'x'; 64 * 1024];
        let mut remaining = n;
        while remaining > 0 {
            let take = remaining.min(chunk.len() as u64) as usize;
            if out.write_all(&chunk[..take]).is_err() {
                break;
            }
            remaining -= take as u64;
        }
    }
    let _ = out.flush();
    drop(out);
    let mut err = std::io::stderr().lock();
    let _ = err.write_all(behavior.stderr.as_bytes());
    let _ = err.flush();
    drop(err);
    exit(behavior.exit_code);
}

fn write_pid_file(path: &Path, grandchild_pid: Option<u32>) {
    let pid = std::process::id();
    let text = match grandchild_pid {
        Some(grandchild) => format!("{{\"pid\":{pid},\"grandchild_pid\":{grandchild}}}"),
        None => format!("{{\"pid\":{pid},\"grandchild_pid\":null}}"),
    };
    std::fs::write(path, text).expect("write the stub pid file");
}

#[cfg(unix)]
fn write_arg_bytes(path: &Path, arg: &OsStr) {
    use std::os::unix::ffi::OsStrExt;
    std::fs::write(path, arg.as_bytes()).expect("write the audio argument bytes");
}

#[cfg(not(unix))]
fn write_arg_bytes(path: &Path, arg: &OsStr) {
    std::fs::write(path, arg.to_string_lossy().as_bytes()).expect("write the audio argument bytes");
}
