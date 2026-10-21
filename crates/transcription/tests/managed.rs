//! Process-level tests for `pumice-transcribe serve --managed`: the real CLI
//! entry point with a fake engine, run as a child process, so stdout, stdin
//! EOF, the exit status and the watchdog are observed as an owner sees them.
//! The one-off `file` tests at the end reuse the same child.
//!
//! This target has `harness = false`: the same executable is the test runner
//! and, when `PUMICE_MANAGED_TEST_CHILD` is set, the child. libtest would
//! print its own lines on stdout, which the child must never do.

use pumice_transcription::cli::run_with_engine_factory;
use pumice_transcription::compute::{Compute, ComputeBackend};
use pumice_transcription::engine::EngineRequest;
use pumice_transcription::{
    CancelToken, Engine, EngineError, Error, Transcript, TranscriptionOptions,
};
use serde_json::Value;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitCode, ExitStatus, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const CHILD_ENV: &str = "PUMICE_MANAGED_TEST_CHILD";
/// `ok`, `stall` (ignores cancellation forever) or `cooperative` (runs until
/// cancelled), `fail` (every run is a native engine failure), `fail-load`,
/// or `no-threads` (on Linux, forbids new threads before loading).
const ENGINE_ENV: &str = "PUMICE_TEST_ENGINE";
/// `vulkan`: the fake engine reports a GPU snapshot; otherwise a CPU one.
const COMPUTE_ENV: &str = "PUMICE_TEST_COMPUTE";
const LOAD_MS_ENV: &str = "PUMICE_TEST_LOAD_MS";
/// Created by the fake engine when a transcription starts.
const STARTED_FILE_ENV: &str = "PUMICE_TEST_STARTED_FILE";
/// Created by the fake engine factory when the model load starts (in
/// `no-threads` mode, once new threads are forbidden).
const LOADING_FILE_ENV: &str = "PUMICE_TEST_LOADING_FILE";

const MODEL: &str = "turbo";
const INSTANCE: &str = "gen-1";
const BOUNDARY: &str = "pumice-managed-boundary";
/// Matches `managed::SHUTDOWN_GRACE`; the forced exit cannot come earlier.
const GRACE: Duration = Duration::from_millis(2000);
/// Upper bound for anything that should be prompt, generous for slow CI.
const PROMPT: Duration = Duration::from_secs(10);

// --- Child -------------------------------------------------------------------

struct ChildEngine {
    mode: String,
    started_file: Option<PathBuf>,
    compute: Compute,
}

impl Engine for ChildEngine {
    fn compute(&self) -> Option<Compute> {
        Some(self.compute.clone())
    }

    fn transcribe(
        &self,
        _pcm: &[f32],
        _options: &TranscriptionOptions,
        cancel: &CancelToken,
    ) -> Result<Transcript, EngineError> {
        if let Some(path) = &self.started_file {
            std::fs::write(path, b"started").unwrap();
        }
        match self.mode.as_str() {
            "stall" => loop {
                thread::sleep(Duration::from_millis(10));
            },
            "cooperative" => {
                while !cancel.is_cancelled() {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(EngineError::Cancelled)
            }
            "fail" => Err(EngineError::Failed),
            "reject" => Err(EngineError::Rejected),
            _ => Ok(Transcript {
                text: "fake words".to_string(),
                language: Some("en".to_string()),
            }),
        }
    }
}

fn child_main() -> ExitCode {
    let mode = std::env::var(ENGINE_ENV).unwrap_or_else(|_| "ok".to_string());
    let load_ms: u64 = std::env::var(LOAD_MS_ENV)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let started_file = std::env::var_os(STARTED_FILE_ENV).map(PathBuf::from);
    let loading_file = std::env::var_os(LOADING_FILE_ENV).map(PathBuf::from);
    let gpu = std::env::var(COMPUTE_ENV).is_ok_and(|value| value == "vulkan");
    run_with_engine_factory(std::env::args_os(), move |request: EngineRequest<'_>| {
        #[cfg(target_os = "linux")]
        if mode == "no-threads" {
            let none = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            // SAFETY: plain syscall on this process with a valid struct.
            assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NPROC, &none) }, 0);
        }
        if let Some(path) = &loading_file {
            std::fs::write(path, b"loading").unwrap();
        }
        thread::sleep(Duration::from_millis(load_ms));
        if mode == "fail-load" {
            return Err(Error::model_load_failed());
        }
        let compute = if gpu {
            Compute::new(
                request.backend,
                ComputeBackend::Vulkan,
                "Vulkan0",
                "Fake GPU",
                Some("x64"),
                None,
            )
        } else {
            Compute::new(
                request.backend,
                ComputeBackend::Cpu,
                "CPU",
                "Fake CPU",
                None,
                None,
            )
        };
        Ok(Box::new(ChildEngine {
            mode,
            started_file,
            compute,
        }) as Box<dyn Engine>)
    })
}

// --- Runner --------------------------------------------------------------------

fn main() -> ExitCode {
    if std::env::var_os(CHILD_ENV).is_some() {
        return child_main();
    }
    let tests: &[(&str, fn())] = &[
        (
            "only_a_whole_response_counts_as_complete",
            only_a_whole_response_counts_as_complete,
        ),
        (
            "ready_follows_load_and_bind_and_stdin_eof_stops_cleanly",
            ready_follows_load_and_bind_and_stdin_eof_stops_cleanly,
        ),
        (
            "eof_during_model_load_never_announces_ready",
            eof_during_model_load_never_announces_ready,
        ),
        (
            "eof_during_a_stuck_model_load_hits_the_watchdog",
            eof_during_a_stuck_model_load_hits_the_watchdog,
        ),
        #[cfg(target_os = "linux")]
        (
            "a_watchdog_that_cannot_start_exits_at_once",
            a_watchdog_that_cannot_start_exits_at_once,
        ),
        (
            "eof_during_a_stalled_inference_hits_the_watchdog",
            eof_during_a_stalled_inference_hits_the_watchdog,
        ),
        (
            "eof_during_a_cooperative_inference_drains_and_exits_cleanly",
            eof_during_a_cooperative_inference_drains_and_exits_cleanly,
        ),
        (
            "a_gpu_engine_failure_retires_a_managed_child",
            a_gpu_engine_failure_retires_a_managed_child,
        ),
        (
            "a_gpu_engine_failure_retires_a_standalone_server",
            a_gpu_engine_failure_retires_a_standalone_server,
        ),
        (
            "a_cpu_engine_failure_keeps_serving",
            a_cpu_engine_failure_keeps_serving,
        ),
        (
            "a_content_error_on_a_gpu_keeps_serving",
            a_content_error_on_a_gpu_keeps_serving,
        ),
        (
            "cancellation_on_a_gpu_does_not_retire",
            cancellation_on_a_gpu_does_not_retire,
        ),
        (
            "invalid_instance_id_is_refused_without_echo",
            invalid_instance_id_is_refused_without_echo,
        ),
        (
            "startup_failures_are_one_error_record",
            startup_failures_are_one_error_record,
        ),
        (
            "a_model_id_not_in_the_folder_is_refused_before_loading",
            a_model_id_not_in_the_folder_is_refused_before_loading,
        ),
        (
            "normal_mode_ignores_stdin_and_keeps_stdout_empty",
            normal_mode_ignores_stdin_and_keeps_stdout_empty,
        ),
        #[cfg(unix)]
        (
            "sigterm_stops_a_managed_child_cleanly",
            sigterm_stops_a_managed_child_cleanly,
        ),
        (
            "a_one_off_file_runs_without_a_config_file",
            a_one_off_file_runs_without_a_config_file,
        ),
        (
            "a_one_off_file_still_needs_a_complete_config_file",
            a_one_off_file_still_needs_a_complete_config_file,
        ),
    ];
    let filter = std::env::args().skip(1).find(|arg| !arg.starts_with('-'));
    let selected: Vec<_> = tests
        .iter()
        .filter(|(name, _)| filter.as_deref().is_none_or(|f| name.contains(f)))
        .collect();
    println!("\nrunning {} tests", selected.len());
    // Every test owns its child and port, so they run in parallel.
    let handles: Vec<_> = selected
        .iter()
        .map(|&&(name, test)| (name, thread::spawn(test)))
        .collect();
    let mut failed = Vec::new();
    for (name, handle) in handles {
        if handle.join().is_ok() {
            println!("test {name} ... ok");
        } else {
            println!("test {name} ... FAILED");
            failed.push(name);
        }
    }
    if failed.is_empty() {
        println!("\ntest result: ok. {} passed", selected.len());
        ExitCode::SUCCESS
    } else {
        println!("\ntest result: FAILED. {} failed: {failed:?}", failed.len());
        ExitCode::FAILURE
    }
}

// --- Helpers -----------------------------------------------------------------

struct Setup {
    _dir: tempfile::TempDir,
    config: PathBuf,
    started_file: PathBuf,
    loading_file: PathBuf,
    model: String,
}

fn setup(model_present: bool) -> Setup {
    setup_with_model_id(model_present, MODEL)
}

fn setup_with_model_id(model_present: bool, model: &str) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    // The models folder: the model's id is its file name without .gguf.
    std::fs::create_dir(dir.path().join("models")).unwrap();
    if model_present {
        std::fs::write(
            dir.path().join("models").join(format!("{model}.gguf")),
            b"not a real model",
        )
        .unwrap();
    }
    let config = dir.path().join("transcribe.yml");
    std::fs::write(
        &config,
        "schema_version: 1\ndefault_model: null\nlanguage: null\nthreads: auto\nbackend: cpu\nserver:\n  port: null\nmodels_dir: models\n",
    )
    .unwrap();
    let started_file = dir.path().join("started");
    let loading_file = dir.path().join("loading");
    Setup {
        _dir: dir,
        config,
        started_file,
        loading_file,
        model: model.to_string(),
    }
}

struct Spawned {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    stdout: Option<thread::JoinHandle<Vec<String>>>,
    stderr: Arc<Mutex<String>>,
    stderr_reader: Option<thread::JoinHandle<()>>,
}

fn spawn(setup: &Setup, engine: &str, load_ms: u64, extra: &[&str]) -> Spawned {
    spawn_with_compute(setup, engine, "cpu", load_ms, extra)
}

fn spawn_with_compute(
    setup: &Setup,
    engine: &str,
    compute: &str,
    load_ms: u64,
    extra: &[&str],
) -> Spawned {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .env(CHILD_ENV, "1")
        .env(ENGINE_ENV, engine)
        .env(COMPUTE_ENV, compute)
        .env(LOAD_MS_ENV, load_ms.to_string())
        .env(STARTED_FILE_ENV, &setup.started_file)
        .env(LOADING_FILE_ENV, &setup.loading_file)
        .arg("--config")
        .arg(&setup.config)
        .args(["serve", "--model", &setup.model, "--port", "0"])
        .args(extra)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let stdin = child.stdin.take();

    let (tx, lines) = mpsc::channel();
    let out = child.stdout.take().unwrap();
    let stdout = thread::spawn(move || {
        let mut all = Vec::new();
        for line in BufReader::new(out).lines() {
            let line = line.unwrap();
            all.push(line.clone());
            let _ = tx.send(line);
        }
        all
    });
    let stderr = Arc::new(Mutex::new(String::new()));
    let mut err = child.stderr.take().unwrap();
    let sink = Arc::clone(&stderr);
    let stderr_reader = thread::spawn(move || {
        let mut buf = [0u8; 1024];
        while let Ok(n) = err.read(&mut buf) {
            if n == 0 {
                break;
            }
            sink.lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&buf[..n]));
        }
    });
    Spawned {
        child,
        stdin,
        lines,
        stdout: Some(stdout),
        stderr,
        stderr_reader: Some(stderr_reader),
    }
}

fn managed_args() -> [&'static str; 3] {
    ["--managed", "--instance-id", INSTANCE]
}

impl Spawned {
    fn next_record(&self) -> Value {
        let line = self
            .lines
            .recv_timeout(PROMPT)
            .unwrap_or_else(|_| panic!("no record; stderr: {}", self.stderr()));
        parse_record(&line)
    }

    fn close_stdin(&mut self) {
        drop(self.stdin.take());
    }

    fn stderr(&self) -> String {
        self.stderr.lock().unwrap().clone()
    }

    fn wait(&mut self, limit: Duration) -> ExitStatus {
        let started = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                // The child has exited, but its stderr may still be in the
                // pipe: let the reader reach end of file before callers read
                // it. A grandchild holding the pipe open would block this, and
                // the managed child spawns none.
                if let Some(reader) = self.stderr_reader.take() {
                    reader.join().unwrap();
                }
                return status;
            }
            if started.elapsed() > limit {
                let _ = self.child.kill();
                panic!("child still running; stderr: {}", self.stderr());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Every stdout line the child wrote, once it has exited.
    fn all_records(&mut self) -> Vec<Value> {
        self.stdout
            .take()
            .unwrap()
            .join()
            .unwrap()
            .iter()
            .map(|line| parse_record(line))
            .collect()
    }

    fn wait_for_started(&self, setup: &Setup) {
        self.wait_for_file(&setup.started_file, "the engine never started");
    }

    fn wait_for_loading(&self, setup: &Setup) {
        self.wait_for_file(&setup.loading_file, "the model load never started");
    }

    fn wait_for_file(&self, path: &Path, failure: &str) {
        let started = Instant::now();
        while !path.exists() {
            assert!(
                started.elapsed() < PROMPT,
                "{failure}; stderr: {}",
                self.stderr()
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Spawned {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A lifecycle record: one bounded JSON object of the managed protocol.
fn parse_record(line: &str) -> Value {
    assert!(line.len() < 4096, "record too long");
    let value: Value =
        serde_json::from_str(line).unwrap_or_else(|_| panic!("not a JSON record: {line}"));
    assert_eq!(value["schema_version"], 1, "{line}");
    assert_eq!(value["module"], "transcription", "{line}");
    assert!(
        ["ready", "stopping", "stopped", "error"].contains(&value["event"].as_str().unwrap()),
        "{line}"
    );
    value
}

fn events(records: &[Value]) -> Vec<String> {
    records
        .iter()
        .map(|r| r["event"].as_str().unwrap().to_string())
        .collect()
}

fn wav(seconds: f64) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::new(&mut buf, spec).unwrap();
    for _ in 0..(seconds * 16_000.0) as usize {
        writer.write_sample(0i16).unwrap();
    }
    writer.finalize().unwrap();
    buf.into_inner()
}

fn transcription_body() -> Vec<u8> {
    let mut body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"clip.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(&wav(1.0));
    body.extend_from_slice(
        format!(
            "\r\n--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n{MODEL}\r\n--{BOUNDARY}--\r\n"
        )
        .as_bytes(),
    );
    body
}

/// One HTTP/1.1 request over a real socket; returns the status and body.
fn http(port: u16, method: &str, path: &str, body: &[u8]) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(PROMPT)).unwrap();
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    if !body.is_empty() {
        head.push_str(&format!(
            "Content-Type: multipart/form-data; boundary={BOUNDARY}\r\n"
        ));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
    let mut response = Vec::new();
    // A server that answers without reading the request body (a 404 for a
    // POST) closes with that body unread, which resets the connection; on
    // macOS the reset can reach this read after the response did. The bytes
    // read before the error stay in `response`, and count only when they are
    // the whole response.
    match stream.read_to_end(&mut response) {
        Ok(_) => {}
        Err(error)
            if error.kind() == std::io::ErrorKind::ConnectionReset
                && complete_response(&response) => {}
        Err(error) => panic!("reading the response: {error}"),
    }
    let response = String::from_utf8(response).unwrap();
    let status = response[9..12].parse().unwrap();
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default();
    (status, body)
}

/// Whether `bytes` hold one whole HTTP/1.1 response: a status line, the
/// headers and as many body bytes as their `Content-Length` announces.
fn complete_response(bytes: &[u8]) -> bool {
    let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let Ok(head) = std::str::from_utf8(&bytes[..end]) else {
        return false;
    };
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    if !status_line.starts_with("HTTP/1.1 ") || status_line.len() < 12 {
        return false;
    }
    let length = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse::<usize>().ok())
            .flatten()
    });
    length.is_some_and(|length| bytes.len() - (end + 4) == length)
}

fn only_a_whole_response_counts_as_complete() {
    let whole =
        b"HTTP/1.1 404 Not Found\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}";
    assert!(complete_response(whole));
    for cut in 1..whole.len() {
        assert!(!complete_response(&whole[..cut]), "{cut}");
    }
    assert!(!complete_response(b"HTTP/1.1 200 OK\r\n\r\n{}"));
    assert!(!complete_response(b""));
}

fn ready_port(record: &Value) -> u16 {
    assert_eq!(record["event"], "ready", "{record}");
    let port = record["port"].as_u64().unwrap();
    assert_ne!(port, 0);
    u16::try_from(port).unwrap()
}

// --- Tests -------------------------------------------------------------------

fn ready_follows_load_and_bind_and_stdin_eof_stops_cleanly() {
    let setup = setup(true);
    let started = Instant::now();
    let mut child = spawn(&setup, "ok", 300, &managed_args());

    let ready = child.next_record();
    assert!(started.elapsed() >= Duration::from_millis(300));
    let port = ready_port(&ready);
    let keys: Vec<_> = ready.as_object().unwrap().keys().cloned().collect();
    assert_eq!(
        keys,
        [
            "schema_version",
            "event",
            "module",
            "instance_id",
            "protocol_version",
            "version",
            "host",
            "port",
            "model",
            "capabilities",
            "model_load_ms",
            "compute",
        ]
    );
    assert_eq!(ready["instance_id"], INSTANCE);
    assert_eq!(ready["protocol_version"], 2);
    assert_eq!(
        ready["compute"],
        serde_json::json!({
            "requested": "cpu",
            "backend": "cpu",
            "device": {"name": "CPU", "description": "Fake CPU"},
            "cpu_variant": null,
            "fallback_reason": null,
        })
    );
    assert_eq!(ready["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(ready["host"], "127.0.0.1");
    assert_eq!(ready["model"], MODEL);
    assert_eq!(
        ready["capabilities"],
        serde_json::json!([
            "openai_transcriptions",
            "detailed_transcriptions",
            "managed_shutdown"
        ])
    );
    assert!(ready["model_load_ms"].as_u64().unwrap() >= 300);

    // The announced listener answers at once.
    let (status, body) = http(port, "GET", "/ready", b"");
    assert_eq!(status, 200, "{body}");
    let body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["instance_id"], INSTANCE);
    assert_eq!(body["protocol_version"], 2);
    assert_eq!(body["compute"], ready["compute"]);
    assert_eq!(body["state"], "ready");

    let (status, body) = http(
        port,
        "POST",
        "/pumice/v1/transcriptions",
        &transcription_body(),
    );
    assert_eq!(status, 200, "{body}");
    let body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["schema_version"], 1);
    assert_eq!(body["text"], "fake words");
    assert_eq!(body["language"], "en");
    assert_eq!(body["audio_seconds"], 1.0);
    assert!(body["inference_ms"].is_u64());

    let (status, body) = http(
        port,
        "POST",
        "/v1/audio/transcriptions",
        &transcription_body(),
    );
    assert_eq!((status, body.as_str()), (200, r#"{"text":"fake words"}"#));

    child.close_stdin();
    let closed = Instant::now();
    let status = child.wait(PROMPT);
    assert_eq!(status.code(), Some(0), "stderr: {}", child.stderr());
    assert!(closed.elapsed() < GRACE, "{:?}", closed.elapsed());

    let records = child.all_records();
    assert_eq!(events(&records), ["ready", "stopping", "stopped"]);
    assert_eq!(records[1]["reason"], "stdin_closed");
    assert_eq!(records[1]["instance_id"], INSTANCE);
    assert_eq!(records[2]["instance_id"], INSTANCE);
}

fn eof_during_model_load_never_announces_ready() {
    let setup = setup(true);
    let mut child = spawn(&setup, "ok", 800, &managed_args());
    child.wait_for_loading(&setup);
    child.close_stdin();

    let stopping = child.next_record();
    assert_eq!(stopping["event"], "stopping");
    let status = child.wait(PROMPT);
    assert_eq!(status.code(), Some(0), "stderr: {}", child.stderr());
    assert_eq!(events(&child.all_records()), ["stopping", "stopped"]);
}

fn eof_during_a_stuck_model_load_hits_the_watchdog() {
    let setup = setup(true);
    let mut child = spawn(&setup, "ok", 60_000, &managed_args());
    child.wait_for_loading(&setup);
    child.close_stdin();
    let closed = Instant::now();

    let status = child.wait(PROMPT);
    let elapsed = closed.elapsed();
    assert_eq!(status.code(), Some(3), "stderr: {}", child.stderr());
    assert!(elapsed >= GRACE - Duration::from_millis(100), "{elapsed:?}");
    assert_eq!(events(&child.all_records()), ["stopping"]);
}

/// The watchdog thread cannot be created (no threads allowed): with nothing
/// to bound the shutdown, a stop request exits at once instead.
#[cfg(target_os = "linux")]
fn a_watchdog_that_cannot_start_exits_at_once() {
    // SAFETY: plain syscall without arguments.
    if unsafe { libc::geteuid() } == 0 {
        // RLIMIT_NPROC does not apply to root.
        eprintln!("skipped a_watchdog_that_cannot_start_exits_at_once: running as root");
        return;
    }
    let setup = setup(true);
    let mut child = spawn(&setup, "no-threads", 60_000, &managed_args());
    child.wait_for_loading(&setup);
    child.close_stdin();
    let closed = Instant::now();

    let status = child.wait(PROMPT);
    assert_eq!(status.code(), Some(3), "stderr: {}", child.stderr());
    assert!(closed.elapsed() < GRACE, "{:?}", closed.elapsed());
    assert!(child.all_records().is_empty());
}

fn eof_during_a_stalled_inference_hits_the_watchdog() {
    let setup = setup(true);
    let mut child = spawn(&setup, "stall", 0, &managed_args());
    let port = ready_port(&child.next_record());

    let client = thread::spawn(move || {
        http(
            port,
            "POST",
            "/pumice/v1/transcriptions",
            &transcription_body(),
        )
    });
    child.wait_for_started(&setup);
    child.close_stdin();
    let closed = Instant::now();

    // The request is answered at once; the stuck engine holds the exit.
    let (status, body) = client.join().unwrap();
    assert_eq!(status, 503, "{body}");
    assert!(body.contains(r#""code":"shutting_down""#), "{body}");

    let status = child.wait(PROMPT);
    let elapsed = closed.elapsed();
    assert_eq!(status.code(), Some(3), "stderr: {}", child.stderr());
    assert!(elapsed >= GRACE - Duration::from_millis(100), "{elapsed:?}");
    assert_eq!(events(&child.all_records()), ["ready", "stopping"]);
}

fn eof_during_a_cooperative_inference_drains_and_exits_cleanly() {
    let setup = setup(true);
    let mut child = spawn(&setup, "cooperative", 0, &managed_args());
    let port = ready_port(&child.next_record());

    let client = thread::spawn(move || {
        http(
            port,
            "POST",
            "/pumice/v1/transcriptions",
            &transcription_body(),
        )
    });
    child.wait_for_started(&setup);
    child.close_stdin();
    let closed = Instant::now();

    let (status, _) = client.join().unwrap();
    assert_eq!(status, 503);
    let status = child.wait(PROMPT);
    assert_eq!(status.code(), Some(0), "stderr: {}", child.stderr());
    assert!(closed.elapsed() < GRACE, "{:?}", closed.elapsed());
    assert_eq!(
        events(&child.all_records()),
        ["ready", "stopping", "stopped"]
    );
}

fn a_gpu_engine_failure_retires_a_managed_child() {
    let setup = setup(true);
    let mut args = managed_args().to_vec();
    args.extend(["--backend", "auto"]);
    let mut child = spawn_with_compute(&setup, "fail", "vulkan", 0, &args);
    let ready = child.next_record();
    let port = ready_port(&ready);
    assert_eq!(ready["compute"]["requested"], "auto");
    assert_eq!(ready["compute"]["backend"], "vulkan");

    // The failing request is answered with the usual error, never retried.
    let (status, body) = http(
        port,
        "POST",
        "/pumice/v1/transcriptions",
        &transcription_body(),
    );
    assert_eq!(status, 500, "{body}");
    assert!(body.contains(r#""code":"transcription_failed""#), "{body}");

    let status = child.wait(PROMPT);
    assert_eq!(status.code(), Some(1), "stderr: {}", child.stderr());
    let records = child.all_records();
    assert_eq!(events(&records), ["ready", "stopping", "stopped"]);
    assert_eq!(records[1]["reason"], "engine_failed");
    assert!(child.stderr().contains("the GPU engine failed"));
}

fn a_gpu_engine_failure_retires_a_standalone_server() {
    let setup = setup(true);
    let mut child = spawn_with_compute(&setup, "fail", "vulkan", 0, &["--backend", "gpu"]);
    let port = listening_port(&child);
    assert!(
        child
            .stderr()
            .contains(r#"compute: {"requested":"gpu","backend":"vulkan""#),
        "stderr: {}",
        child.stderr()
    );
    let (status, body) = http(
        port,
        "POST",
        "/v1/audio/transcriptions",
        &transcription_body(),
    );
    assert_eq!(status, 500, "{body}");
    let status = child.wait(PROMPT);
    assert_eq!(status.code(), Some(1), "stderr: {}", child.stderr());
    assert!(
        child
            .stderr()
            .contains("the GPU engine failed during a transcription")
    );
    let lines = child.stdout.take().unwrap().join().unwrap();
    assert!(lines.is_empty(), "{lines:?}");
}

fn a_cpu_engine_failure_keeps_serving() {
    let setup = setup(true);
    let mut child = spawn(&setup, "fail", 0, &managed_args());
    let port = ready_port(&child.next_record());
    for _ in 0..2 {
        let (status, _) = http(
            port,
            "POST",
            "/pumice/v1/transcriptions",
            &transcription_body(),
        );
        assert_eq!(status, 500);
    }
    let (status, _) = http(port, "GET", "/ready", b"");
    assert_eq!(status, 200);
    child.close_stdin();
    assert_eq!(child.wait(PROMPT).code(), Some(0));
    let records = child.all_records();
    assert_eq!(events(&records), ["ready", "stopping", "stopped"]);
    assert_eq!(records[1]["reason"], "stdin_closed");
}

/// A request the engine refuses for its content leaves a GPU process ready
/// (p14.2-backend-policy/D1).
fn a_content_error_on_a_gpu_keeps_serving() {
    let setup = setup(true);
    let mut child = spawn_with_compute(&setup, "reject", "vulkan", 0, &managed_args());
    let port = ready_port(&child.next_record());
    for _ in 0..2 {
        let (status, body) = http(
            port,
            "POST",
            "/pumice/v1/transcriptions",
            &transcription_body(),
        );
        assert_eq!(status, 500, "{body}");
        assert!(body.contains(r#""code":"transcription_failed""#), "{body}");
    }
    let (status, _) = http(port, "GET", "/ready", b"");
    assert_eq!(status, 200);
    child.close_stdin();
    assert_eq!(
        child.wait(PROMPT).code(),
        Some(0),
        "stderr: {}",
        child.stderr()
    );
    let records = child.all_records();
    assert_eq!(events(&records), ["ready", "stopping", "stopped"]);
    assert_eq!(records[1]["reason"], "stdin_closed");
    assert!(!child.stderr().contains("the GPU engine failed"));
}

/// A run cancelled by the owner's shutdown on a GPU is not an engine
/// failure: the stop keeps its own reason and a clean exit.
fn cancellation_on_a_gpu_does_not_retire() {
    let setup = setup(true);
    let mut child = spawn_with_compute(&setup, "cooperative", "vulkan", 0, &managed_args());
    let port = ready_port(&child.next_record());
    let client = thread::spawn(move || {
        http(
            port,
            "POST",
            "/pumice/v1/transcriptions",
            &transcription_body(),
        )
    });
    child.wait_for_started(&setup);
    child.close_stdin();
    let (status, _) = client.join().unwrap();
    assert_eq!(status, 503);
    assert_eq!(
        child.wait(PROMPT).code(),
        Some(0),
        "stderr: {}",
        child.stderr()
    );
    let records = child.all_records();
    assert_eq!(events(&records), ["ready", "stopping", "stopped"]);
    assert_eq!(records[1]["reason"], "stdin_closed");
}

/// The port from a standalone server's startup line on stderr.
fn listening_port(child: &Spawned) -> u16 {
    let started = Instant::now();
    loop {
        let stderr = child.stderr();
        if let Some(rest) = stderr.split("listening on http://127.0.0.1:").nth(1) {
            return rest.split('/').next().unwrap().parse::<u16>().unwrap();
        }
        assert!(started.elapsed() < PROMPT, "no listening line: {stderr}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn invalid_instance_id_is_refused_without_echo() {
    let setup = setup(true);
    for id in ["bad id", "quote\"brace}", &"x".repeat(65), ""] {
        let mut child = spawn(&setup, "ok", 0, &["--managed", "--instance-id", id]);
        let status = child.wait(PROMPT);
        assert_eq!(status.code(), Some(2));
        let lines = child.stdout.take().unwrap().join().unwrap();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(id.is_empty() || !lines[0].contains(id), "{}", lines[0]);
        let record = parse_record(&lines[0]);
        assert_eq!(record["event"], "error");
        assert_eq!(record["code"], "invalid_arguments");
        assert!(record.get("instance_id").is_none());
    }
}

fn startup_failures_are_one_error_record() {
    // (setup with a model file, engine, extra arguments, exit code, error code)
    let cases: [(bool, &str, &[&str], i32, &str); 4] = [
        (false, "ok", &managed_args(), 2, "model_not_found"),
        (true, "fail-load", &managed_args(), 1, "model_load_failed"),
        (true, "ok", &["--managed"], 2, "invalid_arguments"),
        (
            true,
            "ok",
            &["--managed", "--instance-id", INSTANCE, "--bogus"],
            2,
            "invalid_arguments",
        ),
    ];
    for (model_present, engine, extra, exit, code) in cases {
        let setup = setup(model_present);
        let mut child = spawn(&setup, engine, 0, extra);
        let status = child.wait(PROMPT);
        assert_eq!(status.code(), Some(exit), "stderr: {}", child.stderr());
        let records = child.all_records();
        assert_eq!(events(&records), ["error"], "{code}");
        assert_eq!(records[0]["code"], code);
        assert!(
            !child.stderr().is_empty(),
            "the human message goes to stderr"
        );
    }
}

fn a_model_id_not_in_the_folder_is_refused_before_loading() {
    // No file name can give this id (ids are at most 512 bytes, names
    // usually 255): it is never in the folder.
    let long_id = "m".repeat(600);
    let setup = setup_with_model_id(false, &long_id);
    // A load would fail with `model_load_failed`; it never starts.
    let mut child = spawn(&setup, "fail-load", 0, &managed_args());
    let status = child.wait(PROMPT);
    assert_eq!(status.code(), Some(2), "stderr: {}", child.stderr());
    let records = child.all_records();
    assert_eq!(events(&records), ["error"]);
    assert_eq!(records[0]["code"], "model_not_found");
    assert_eq!(records[0]["instance_id"], INSTANCE);
}

fn normal_mode_ignores_stdin_and_keeps_stdout_empty() {
    let setup = setup(true);
    let mut child = spawn(&setup, "ok", 0, &[]);
    let port = listening_port(&child);

    child.close_stdin();
    thread::sleep(Duration::from_millis(300));
    assert!(
        child.child.try_wait().unwrap().is_none(),
        "stdin EOF stopped it"
    );
    assert_eq!(http(port, "GET", "/ready", b"").0, 404);
    assert_eq!(
        http(
            port,
            "POST",
            "/pumice/v1/transcriptions",
            &transcription_body()
        )
        .0,
        404
    );

    child.child.kill().unwrap();
    child.child.wait().unwrap();
    let lines = child.stdout.take().unwrap().join().unwrap();
    assert!(lines.is_empty(), "{lines:?}");
}

#[cfg(unix)]
fn sigterm_stops_a_managed_child_cleanly() {
    let setup = setup(true);
    let mut child = spawn(&setup, "ok", 0, &managed_args());
    ready_port(&child.next_record());
    let pid = libc::pid_t::try_from(child.child.id()).unwrap();
    // SAFETY: plain syscall on the child we spawned and still own.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    let status = child.wait(PROMPT);
    assert_eq!(status.code(), Some(0), "stderr: {}", child.stderr());
    let records = child.all_records();
    assert_eq!(events(&records), ["ready", "stopping", "stopped"]);
    assert_eq!(records[1]["reason"], "signal");
}

// --- One-off `file` ----------------------------------------------------------
//
// The same child, as `pumice-transcribe file`: a one-off `file --model-file`
// runs from its flags without a configuration file, but a file that exists
// must be complete.

/// Runs `args` in the child with `home` as the only per-user location.
fn run_file_child(home: &Path, args: &[OsString]) -> std::process::Output {
    Command::new(std::env::current_exe().unwrap())
        .env(CHILD_ENV, "1")
        .env(ENGINE_ENV, "ok")
        .env_remove(COMPUTE_ENV)
        .env_remove(LOAD_MS_ENV)
        .env_remove(STARTED_FILE_ENV)
        .env_remove(LOADING_FILE_ENV)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("APPDATA", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("LOCALAPPDATA", home.join("data"))
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn one_off_file_args(wav: &Path, model: &Path) -> Vec<OsString> {
    vec![
        "file".into(),
        wav.into(),
        "--model-file".into(),
        model.into(),
        "--json".into(),
    ]
}

fn a_one_off_file_runs_without_a_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let wav_path = dir.path().join("clip.wav");
    std::fs::write(&wav_path, wav(0.5)).unwrap();
    let model = dir.path().join("model.gguf");
    std::fs::write(&model, b"not a real model").unwrap();

    let out = run_file_child(dir.path(), &one_off_file_args(&wav_path, &model));
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["ok"], true, "{json}");
    assert_eq!(json["result"]["text"], "fake words", "{json}");
    // Named by the models folder's rule: the file name without .gguf.
    assert_eq!(json["result"]["model"], "model", "{json}");
    // Without a configuration the backend is the flag default, cpu.
    assert_eq!(json["result"]["compute"]["requested"], "cpu", "{json}");

    // The flags still apply.
    let mut args = one_off_file_args(&wav_path, &model);
    args.extend(["--backend", "auto", "--language", "pt"].map(OsString::from));
    let out = run_file_child(dir.path(), &args);
    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["result"]["compute"]["requested"], "auto", "{json}");
    assert_eq!(json["result"]["language"], "pt", "{json}");

    // A model id needs the configuration file.
    let out = run_file_child(
        dir.path(),
        &[
            "file".into(),
            wav_path.clone().into(),
            "--model".into(),
            MODEL.into(),
            "--json".into(),
        ],
    );
    assert_eq!(out.status.code(), Some(2));
    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["error"]["code"], "invalid_config", "{json}");
}

fn a_one_off_file_still_needs_a_complete_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let wav_path = dir.path().join("clip.wav");
    std::fs::write(&wav_path, wav(0.5)).unwrap();
    let model = dir.path().join("model.gguf");
    std::fs::write(&model, b"not a real model").unwrap();
    let incomplete = "schema_version: 1\n";

    // The per-user file.
    let per_user = dir.path().join("config").join("pumice");
    std::fs::create_dir_all(&per_user).unwrap();
    std::fs::write(per_user.join("transcribe.yml"), incomplete).unwrap();
    let out = run_file_child(dir.path(), &one_off_file_args(&wav_path, &model));
    assert_eq!(out.status.code(), Some(2));
    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["error"]["code"], "invalid_config", "{json}");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("\"default_model\" is missing"),
        "{json}"
    );

    // An explicit --config file.
    let explicit = dir.path().join("explicit.yml");
    std::fs::write(&explicit, incomplete).unwrap();
    let mut args: Vec<OsString> = vec!["--config".into(), explicit.into()];
    args.extend(one_off_file_args(&wav_path, &model));
    let out = run_file_child(dir.path(), &args);
    assert_eq!(out.status.code(), Some(2));
    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["error"]["code"], "invalid_config", "{json}");
}
