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
//!
//! **Managed serve** (`--config <FILE> serve --model <ID> --port 0 --managed
//! --instance-id <ID>`, for `pumice serve`): a scripted managed child with a
//! real loopback `GET /ready`, driven by the scenario's `serve` section:
//!
//! ```json
//! {"serve": {"spawn_log_path": "/tmp/spawns.jsonl",
//!            "generations": [{"crash_after_ready_ms": 100}, {}]}}
//! ```
//!
//! Each spawn appends `{"event":"spawn","pid":…,"instance_id":…,"argv":[…],
//! "prev_alive":…}` to the spawn log (`prev_alive`: on Linux, whether the
//! previous spawn's process still existed, zombie included) and later
//! `{"event":"eof",…}` when its stdin closes. The n-th spawn follows
//! `generations[n]` (the last entry repeats); see [`Managed`] for the knobs.

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
    #[serde(default)]
    serve: ManagedScenario,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedScenario {
    #[serde(default)]
    spawn_log_path: Option<PathBuf>,
    #[serde(default)]
    generations: Vec<Managed>,
}

/// One managed child's script. The defaults behave like a healthy child.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Managed {
    /// Delay before `ready` (the model load).
    #[serde(default)]
    ready_delay_ms: u64,
    /// Never write `ready` (wait for stdin EOF instead).
    #[serde(default)]
    never_ready: bool,
    /// Fields replacing the `ready` record's own (wrong instance, version,
    /// model, host, port, capabilities, …).
    #[serde(default)]
    ready_overrides: serde_json::Map<String, serde_json::Value>,
    /// Raw lines written before `ready`.
    #[serde(default)]
    stdout_before: Vec<String>,
    /// Raw lines written after `ready`.
    #[serde(default)]
    stdout_after: Vec<String>,
    /// Write a startup `error` record with this code and exit 2.
    #[serde(default)]
    error_code: Option<String>,
    /// Bytes written to stderr before `ready` (blocks unless drained).
    #[serde(default)]
    stderr_flood_bytes: Option<u64>,
    /// Abort this long after `ready`.
    #[serde(default)]
    crash_after_ready_ms: Option<u64>,
    /// Exit with status 0 this long after `ready`.
    #[serde(default)]
    exit_after_ready_ms: Option<u64>,
    /// Answer this many `GET /ready` correctly, then fail them as
    /// `ready_failure` says ("hang" (default), "503" or "wrong_instance").
    #[serde(default)]
    ready_ok_count: Option<u32>,
    #[serde(default)]
    ready_failure: Option<String>,
    /// Keep running after stdin EOF.
    #[serde(default)]
    ignore_eof: bool,
    /// After stdin EOF, wait this long before `stopping`/`stopped` and exit.
    #[serde(default)]
    exit_delay_ms: u64,
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
        Some(name) if name == OsStr::new("serve") => managed_mode(args, &scenario.serve),
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

/// Serializes writes to stdout between the main and lifeline threads.
static STDOUT: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn stdout_line(line: &str) {
    let _guard = STDOUT.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(line.as_bytes());
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}

/// Best-effort: the log's directory may already be gone (a test that
/// failed and removed it), and the lifeline must still end the process.
fn append_log(path: &Option<PathBuf>, value: serde_json::Value) {
    let Some(path) = path else { return };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(format!("{value}\n").as_bytes());
    }
}

fn flag_value(args: &[OsString], flag: &str) -> String {
    args.iter()
        .position(|a| a == OsStr::new(flag))
        .and_then(|i| args.get(i + 1))
        .map(|a| a.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn managed_mode(args: &[OsString], scenario: &ManagedScenario) -> ! {
    let instance_id = flag_value(args, "--instance-id");
    let model = flag_value(args, "--model");
    let log = scenario.spawn_log_path.clone();

    // Which spawn this is, and whether the previous one is still around.
    let previous: Vec<serde_json::Value> = log
        .as_ref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|entry| entry["event"] == "spawn")
        .collect();
    let index = previous.len();
    let prev_alive = previous
        .last()
        .and_then(|entry| entry["pid"].as_u64())
        .map(process_exists);
    let behavior = scenario
        .generations
        .get(index)
        .or(scenario.generations.last())
        .cloned()
        .unwrap_or_default();
    append_log(
        &log,
        serde_json::json!({
            "event": "spawn",
            "pid": std::process::id(),
            "instance_id": instance_id,
            "argv": args.iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>(),
            "prev_alive": prev_alive,
        }),
    );

    // The lifeline: stdin EOF stops the child, unless told to ignore it.
    {
        let log = log.clone();
        let instance_id = instance_id.clone();
        let behavior = behavior.clone();
        std::thread::spawn(move || {
            let mut sink = Vec::new();
            let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut sink);
            append_log(
                &log,
                serde_json::json!({"event": "eof", "pid": std::process::id()}),
            );
            if behavior.ignore_eof {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(behavior.exit_delay_ms));
            stdout_line(
                &serde_json::json!({"schema_version":1,"event":"stopping","module":"transcription","instance_id":instance_id,"reason":"stdin_closed"})
                    .to_string(),
            );
            stdout_line(
                &serde_json::json!({"schema_version":1,"event":"stopped","module":"transcription","instance_id":instance_id})
                    .to_string(),
            );
            exit(0);
        });
    }

    if let Some(n) = behavior.stderr_flood_bytes {
        let chunk = [b'e'; 64 * 1024];
        let mut err = std::io::stderr().lock();
        let mut remaining = n;
        while remaining > 0 {
            let take = remaining.min(chunk.len() as u64) as usize;
            if err.write_all(&chunk[..take]).is_err() {
                break;
            }
            remaining -= take as u64;
        }
    }
    for line in &behavior.stdout_before {
        stdout_line(line);
    }
    if let Some(code) = &behavior.error_code {
        stdout_line(
            &serde_json::json!({"schema_version":1,"event":"error","module":"transcription","instance_id":instance_id,"code":code,"message":"S3NT1NEL stub failure"})
                .to_string(),
        );
        exit(2);
    }

    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind the stub listener");
    let port = listener.local_addr().expect("stub address").port();
    {
        let instance_id = instance_id.clone();
        let model = model.clone();
        let behavior = behavior.clone();
        std::thread::spawn(move || serve_ready(listener, port, instance_id, model, behavior));
    }

    std::thread::sleep(std::time::Duration::from_millis(behavior.ready_delay_ms));
    if !behavior.never_ready {
        let mut ready = serde_json::json!({
            "schema_version": 1,
            "event": "ready",
            "module": "transcription",
            "instance_id": instance_id,
            "protocol_version": 1,
            "version": env!("CARGO_PKG_VERSION"),
            "host": "127.0.0.1",
            "port": port,
            "model": model,
            "capabilities": ["openai_transcriptions", "detailed_transcriptions", "managed_shutdown"],
            "model_load_ms": 7,
        });
        for (key, value) in &behavior.ready_overrides {
            ready[key] = value.clone();
        }
        stdout_line(&ready.to_string());
    }
    for line in &behavior.stdout_after {
        stdout_line(line);
    }
    if let Some(ms) = behavior.crash_after_ready_ms {
        std::thread::sleep(std::time::Duration::from_millis(ms));
        std::process::abort();
    }
    if let Some(ms) = behavior.exit_after_ready_ms {
        std::thread::sleep(std::time::Duration::from_millis(ms));
        exit(0);
    }
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

/// A minimal HTTP/1.1 `GET /ready` server, one thread per connection.
fn serve_ready(
    listener: std::net::TcpListener,
    port: u16,
    instance_id: String,
    model: String,
    behavior: Managed,
) {
    let answered = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let answered = std::sync::Arc::clone(&answered);
        let instance_id = instance_id.clone();
        let model = model.clone();
        let behavior = behavior.clone();
        std::thread::spawn(move || {
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && head.len() < 8192 {
                match std::io::Read::read(&mut stream, &mut byte) {
                    Ok(1) => head.push(byte[0]),
                    _ => return,
                }
            }
            let head = String::from_utf8_lossy(&head).into_owned();
            let host_ok = head
                .lines()
                .any(|line| line.eq_ignore_ascii_case(&format!("host: 127.0.0.1:{port}")));
            let (status, body) = if !head.starts_with("GET /ready ") {
                (
                    "404 Not Found",
                    serde_json::json!({"error": {"code": "not_found"}}),
                )
            } else if !host_ok || head.to_ascii_lowercase().contains("\r\norigin:") {
                (
                    "403 Forbidden",
                    serde_json::json!({"error": {"code": "host_not_allowed"}}),
                )
            } else {
                let n = answered.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let ok = behavior.ready_ok_count.is_none_or(|limit| n < limit);
                let identity = |id: &str| {
                    serde_json::json!({
                        "schema_version": 1,
                        "module": "transcription",
                        "instance_id": id,
                        "protocol_version": 1,
                        "version": env!("CARGO_PKG_VERSION"),
                        "model": model,
                        "capabilities": ["openai_transcriptions", "detailed_transcriptions", "managed_shutdown"],
                        "state": "ready",
                    })
                };
                match (ok, behavior.ready_failure.as_deref()) {
                    (true, _) => ("200 OK", identity(&instance_id)),
                    (false, Some("503")) => (
                        "503 Service Unavailable",
                        serde_json::json!({"error": {"code": "shutting_down"}}),
                    ),
                    (false, Some("wrong_instance")) => ("200 OK", identity("someone-else")),
                    (false, _) => loop {
                        std::thread::sleep(std::time::Duration::from_secs(3600));
                    },
                }
            };
            let body = body.to_string();
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        });
    }
}

#[cfg(target_os = "linux")]
fn process_exists(pid: u64) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(not(target_os = "linux"))]
fn process_exists(_pid: u64) -> bool {
    false
}
