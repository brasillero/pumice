//! In-process fault tests of `pumice serve`'s dictation and transcription
//! routes (p13.4-dictations, p15-serve-transcriptions): the whole service
//! (`serve::run`) with the scenario-driven `pumice-test-transcribe` as its
//! child and the relay's `pumice-root-test-cli` as the provider, driven
//! through the service's test seams to inject a panic into a dictation's
//! task, an unconfirmed cleanup and a late formatter stage. No real
//! transcription module, model or AI CLI is involved.

#[path = "../crates/relay/tests/support/install.rs"]
mod install;
use install::install_executable;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::Instant;

use pumice_relay::process::CleanupScope;

use pumice::orchestrator::cancel::{self, CancelHandle, CancelReason};
use pumice::orchestrator::config;
use pumice::orchestrator::flow::Runtime;
use pumice::orchestrator::serve::child::ChildSpec;
use pumice::orchestrator::serve::dictation::Dictations;
use pumice::orchestrator::serve::http::{CleanupHook, FlowInfo, RequestHook};
use pumice::orchestrator::serve::output::Reporter;
use pumice::orchestrator::serve::supervisor::{Checkpoints, SupervisorConfig, Timings};
use pumice::orchestrator::serve::{self, Service};

const WAIT: Duration = Duration::from_secs(10);
const BOUNDARY: &str = "fault-boundary";

fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

fn scenario_path(exe_path: &Path) -> PathBuf {
    let mut path = exe_path.as_os_str().to_owned();
    path.push(".scenario.json");
    PathBuf::from(path)
}

/// The stub, the fake provider and the configuration files.
struct Setup {
    dir: TempDir,
    stub: PathBuf,
    log: PathBuf,
    /// Written by the fake provider when it runs: proof of an AI call.
    report: PathBuf,
}

fn setup() -> Setup {
    let dir = TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).expect("temp dir");
    let stub = dir.path().join(exe("pumice-transcribe"));
    install_executable(
        Path::new(env!("CARGO_BIN_EXE_pumice-test-transcribe")),
        &stub,
    );
    let log = dir.path().join("spawns.jsonl");
    fs::write(
        scenario_path(&stub),
        json!({"serve": {"spawn_log_path": log, "generations": [{}]}}).to_string(),
    )
    .unwrap();
    let fake = dir.path().join(exe("fake-claude"));
    let report = dir.path().join("fake-report.json");
    install_executable(Path::new(env!("CARGO_BIN_EXE_pumice-root-test-cli")), &fake);
    fs::write(
        scenario_path(&fake),
        json!({
            "stdout": json!({"type": "result", "subtype": "success", "is_error": false, "result": "done"}).to_string(),
            "sleep_ms": 30_000,
            "report_path": report,
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        dir.path().join("relay.yml"),
        format!(
            "port: 7567\ntotal_timeout_secs: 30\nmax_parallel: 4\nproviders:\n  - id: claude\n    enabled: true\n    model: haiku-test\n    binary: '{}'\n    timeout_secs: 30\ndebug_log:\n  enabled: false\n  path: pumice-debug.jsonl\n",
            fake.display()
        ),
    )
    .unwrap();
    fs::write(dir.path().join("transcribe.yml"), "schema_version: 1\ndefault_model: null\nlanguage: null\nthreads: auto\nbackend: cpu\nserver:\n  port: null\nmodels_dir: null\nmodels: []\n").unwrap();
    fs::write(dir.path().join("formatter.yml"), "schema_version: 1\n").unwrap();
    fs::write(
        dir.path().join("orchestrator.yml"),
        "schema_version: 1
modules:
  transcription:
    config: ./transcribe.yml
  formatter:
    config: ./formatter.yml
  relay:
    location: in_process
    config: ./relay.yml
flows:
  - id: plain
    enabled: true
    transcription_model: turbo
    formatter: null
    relay_route: claude
  - id: raw
    enabled: true
    transcription_model: turbo
    formatter: null
    relay_route: passthrough
  - id: clean
    enabled: true
    transcription_model: turbo
    formatter: builtin:clean
    relay_route: claude
    budgets_ms:
      formatter: 50
",
    )
    .unwrap();
    Setup {
        dir,
        stub,
        log,
        report,
    }
}

impl Setup {
    fn spawns(&self) -> Vec<Value> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|entry| entry["event"] == "spawn")
            .collect()
    }
}

/// The whole service on `127.0.0.1:0`, serving dictations.
async fn start(
    setup: &Setup,
    request_hook: Option<RequestHook>,
    cleanup_hook: Option<CleanupHook>,
) -> (
    tokio::task::JoinHandle<u8>,
    u16,
    CancelHandle,
    Arc<CleanupScope>,
) {
    let loaded = config::load(Some(&setup.dir.path().join("orchestrator.yml"))).expect("config");
    let scope = Arc::new(CleanupScope::new());
    let runtime = Runtime::build(&loaded, Arc::clone(&scope)).unwrap_or_else(|_| panic!("runtime"));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind 127.0.0.1:0");
    let port = listener.local_addr().unwrap().port();
    let (signal, signals) = cancel::context();
    let service = Service {
        listener,
        supervisor: SupervisorConfig {
            spec: ChildSpec {
                binary: setup.stub.clone(),
                config: setup.dir.path().join("transcribe.yml"),
                model: "turbo".to_owned(),
            },
            startup_timeout: Duration::from_secs(5),
            timings: Timings {
                probe_interval: Duration::from_secs(30),
                ..Timings::default()
            },
            scope: Arc::clone(&scope),
            checkpoints: Checkpoints::default(),
        },
        model: "turbo".to_owned(),
        flows: loaded
            .config
            .flows
            .iter()
            .map(|flow| FlowInfo {
                id: flow.id.clone(),
                transcription_model: flow.transcription_model.clone(),
                formatter: flow.formatter.clone(),
                relay_route: flow.relay_route.clone(),
            })
            .collect(),
        dictations: Some(Dictations {
            runtime: Arc::new(runtime),
            flows: loaded.config.flows.clone(),
            transcription_flows: vec!["raw".to_owned(), "clean".to_owned()],
        }),
        scope: Arc::clone(&scope),
        allowed_origins: Vec::new(),
        request_hook,
        cleanup_hook,
    };
    let task = tokio::spawn(async move { serve::run(service, &Reporter::silent(), signals).await });
    (task, port, signal, scope)
}

/// One request over a real socket: the status and JSON body, or `None`
/// when the connection closed without a complete answer.
async fn send(port: u16, method: &str, path: &str, body: &[u8]) -> Option<(u16, Value)> {
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: multipart/form-data; boundary={BOUNDARY}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let answer = exchange(port, head, body).await?;
    let split = answer.windows(4).position(|w| w == b"\r\n\r\n")?;
    let status = std::str::from_utf8(answer.get(9..12)?).ok()?.parse().ok()?;
    Some((status, serde_json::from_slice(&answer[split + 4..]).ok()?))
}

/// One OpenAI transcription request: the status, the post-processing
/// status header and the raw body.
async fn transcribe(port: u16, model: &str, format: &str) -> (u16, Option<String>, Vec<u8>) {
    let body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\n\r\nRIFF audio\r\n--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n{model}\r\n--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"response_format\"\r\n\r\n{format}\r\n--{BOUNDARY}--\r\n"
    );
    let head = format!(
        "POST /v1/audio/transcriptions HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: multipart/form-data; boundary={BOUNDARY}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let answer = exchange(port, head, body.as_bytes())
        .await
        .expect("a complete answer");
    let split = answer
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("a head");
    let head = String::from_utf8_lossy(&answer[..split]).into_owned();
    let status = head[9..12].parse().unwrap();
    let post_processing = head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("pumice-postprocessing-status")
            .then(|| value.trim().to_owned())
    });
    (status, post_processing, answer[split + 4..].to_vec())
}

/// Sends `head` and `body` over a fresh connection and reads the answer
/// until the service closes it, within [`WAIT`]: the bytes, or `None` when
/// the connection could not be made or ended without a complete answer.
///
/// A refusal (a 503 once admission stopped) comes before the body is
/// read, and the connection then closes with the body unread, which
/// resets it; on Windows the reset can even drop an answer not yet read.
/// So the request goes out in one write, to reach the service in one
/// piece, a failed write still reads the answer, and a reset counts only
/// after a complete answer.
async fn exchange(port: u16, head: String, body: &[u8]) -> Option<Vec<u8>> {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .ok()?;
    let mut request = head.into_bytes();
    request.extend_from_slice(body);
    let _ = stream.write_all(&request).await;
    let mut answer = Vec::new();
    let mut chunk = [0u8; 8192];
    let deadline = Instant::now() + WAIT;
    loop {
        match tokio::time::timeout_at(deadline, stream.read(&mut chunk)).await {
            Ok(Ok(0)) => return Some(answer),
            Ok(Ok(n)) => answer.extend_from_slice(&chunk[..n]),
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Ok(Err(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
                ) && complete_response(&answer) =>
            {
                return Some(answer);
            }
            Ok(Err(_)) => return None,
            Err(_) => return complete_response(&answer).then_some(answer),
        }
    }
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

#[test]
fn only_a_whole_response_counts_as_complete() {
    let whole =
        b"HTTP/1.1 503 Service Unavailable\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}";
    assert!(complete_response(whole));
    for cut in 1..whole.len() {
        assert!(!complete_response(&whole[..cut]), "{cut}");
    }
    assert!(!complete_response(b"HTTP/1.1 200 OK\r\n\r\n{}"));
    assert!(!complete_response(b""));
}

fn dictation(flow: &str) -> Vec<u8> {
    format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\n\r\nRIFF audio\r\n--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"flow\"\r\n\r\n{flow}\r\n--{BOUNDARY}--\r\n"
    )
    .into_bytes()
}

async fn wait_ready(port: u16) {
    let deadline = Instant::now() + WAIT;
    while send(port, "GET", "/ready", b"")
        .await
        .map(|(status, _)| status)
        != Some(200)
    {
        assert!(Instant::now() < deadline, "the service never became ready");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks for existence.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_panic_in_a_dictation_task_stops_the_service_with_its_cleanup() {
    let setup = setup();
    // The relay stage panics as it starts, after a real transcription: the
    // injected panic happens inside the dictation's own task, not the
    // connection's.
    let hook: RequestHook = Arc::new(move |point: &str| {
        if point == "dictation:relay" {
            panic!("S3NT1NEL injected dictation panic");
        }
    });
    let (task, port, _signal, scope) = start(&setup, Some(hook), None).await;
    wait_ready(port).await;
    let started = Instant::now();
    let answer = send(port, "POST", "/pumice/v1/dictations", &dictation("plain")).await;
    // The connection may be dropped by the service's stop first; an answer
    // that makes it is the route's envelope.
    if let Some((status, body)) = answer {
        assert_eq!(status, 500, "{body}");
        assert!(!body.to_string().contains("S3NT1NEL"), "{body}");
        assert_eq!(body["schema_version"], 1, "{body}");
        assert_eq!(body["module"], "orchestrator");
        assert_eq!(body["ok"], false);
        assert!(body["operation_id"].is_string());
        assert!(body.get("flow").is_none() && body.get("result").is_none());
        assert_eq!(body["error"]["code"], "internal");
        assert_eq!(body["error"]["type"], "server_error");
        for stage in ["prepare", "transcription", "formatter", "relay"] {
            assert_eq!(body["stages"][stage]["status"], "skipped", "{body}");
        }
    }
    let exit = tokio::time::timeout(WAIT, task)
        .await
        .expect("the service stops on its own")
        .expect("the service task itself does not panic");
    assert_eq!(exit, 1, "a core failure (D9)");
    assert!(started.elapsed() < Duration::from_secs(6), "D9 budget");
    assert!(scope.is_empty(), "nothing left unreaped");
    // The same bounded teardown: no listener, the child stopped and reaped.
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err()
    );
    #[cfg(unix)]
    for spawn in setup.spawns() {
        let pid = spawn["pid"].as_u64().unwrap() as u32;
        assert!(!pid_alive(pid), "child {pid} left behind");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unconfirmed_cleanup_stops_admitting_dictations() {
    let setup = setup();
    let unconfirmed = Arc::new(AtomicBool::new(true));
    let flag = Arc::clone(&unconfirmed);
    let hook: CleanupHook = Arc::new(move || flag.load(Ordering::SeqCst));
    let (task, port, signal, _scope) = start(&setup, None, Some(hook)).await;
    wait_ready(port).await;

    // A dictation that would have succeeded: never reported as a success
    // over an unconfirmed cleanup.
    let (status, body) = send(port, "POST", "/pumice/v1/dictations", &dictation("raw"))
        .await
        .expect("an answer");
    assert_eq!(status, 500, "{body}");
    assert_eq!(body["error"]["code"], "cleanup_failed");
    assert_eq!(body["error"]["stage"], "cleanup");
    assert!(body.get("result").is_none());
    assert_eq!(body["transcript"]["text"], "stub transcript");

    // Admission stays closed, even once the cleanup would be fine, and
    // status says why; metadata routes keep answering.
    unconfirmed.store(false, Ordering::SeqCst);
    let (status, body) = send(port, "GET", "/pumice/v1/status", b"").await.unwrap();
    assert_eq!(status, 200);
    assert_eq!(body["dictations"]["admission"], "stopped", "{body}");
    assert_eq!(body["service"]["ready"], false);
    let (status, body) = send(port, "GET", "/ready", b"").await.unwrap();
    assert_eq!(status, 503);
    assert_eq!(body["error"]["code"], "admission_stopped");
    let (status, body) = send(port, "POST", "/pumice/v1/dictations", &dictation("raw"))
        .await
        .unwrap();
    assert_eq!(status, 503, "{body}");
    assert_eq!(body["error"]["code"], "admission_stopped");
    assert_eq!(body["stages"]["prepare"]["status"], "skipped");
    assert_eq!(setup.spawns().len(), 1);

    signal.cancel(CancelReason::Terminate);
    let exit = tokio::time::timeout(WAIT, task).await.unwrap().unwrap();
    assert_eq!(exit, 143);
}

/// D9: an unconfirmed cleanup after a successful transcription still
/// answers the raw transcript, and stops admission as for dictations.
async fn cleanup_failure_answers_the_raw_transcript(format: &str) {
    let setup = setup();
    let hook: CleanupHook = Arc::new(|| true);
    let (task, port, signal, _scope) = start(&setup, None, Some(hook)).await;
    wait_ready(port).await;

    let (status, post_processing, body) = transcribe(port, "raw", format).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(post_processing.as_deref(), Some("failed"));
    if format == "text" {
        assert_eq!(body, b"stub transcript");
    } else {
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["text"], "stub transcript", "{body}");
        let failure = &body["pumice"]["post_processing"];
        assert_eq!(failure["status"], "failed", "{body}");
        assert_eq!(failure["error"]["stage"], "cleanup");
        assert_eq!(failure["error"]["code"], "cleanup_failed");
        assert!(failure["error"]["message"].is_string());
        assert_eq!(body["pumice"]["transcript"]["text"], "stub transcript");
    }

    // Admission is stopped all the same: both routes refuse new work.
    let (status, body) = send(port, "GET", "/pumice/v1/status", b"").await.unwrap();
    assert_eq!(status, 200);
    assert_eq!(body["dictations"]["admission"], "stopped", "{body}");
    let (status, _, body) = transcribe(port, "raw", "json").await;
    assert_eq!(status, 503);
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["error"]["code"], "admission_stopped", "{body}");
    let (status, body) = send(port, "POST", "/pumice/v1/dictations", &dictation("raw"))
        .await
        .unwrap();
    assert_eq!(status, 503, "{body}");
    assert_eq!(setup.spawns().len(), 1);

    signal.cancel(CancelReason::Terminate);
    let exit = tokio::time::timeout(WAIT, task).await.unwrap().unwrap();
    assert_eq!(exit, 143);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cleanup_failure_after_a_transcription_answers_the_raw_transcript_as_json() {
    cleanup_failure_answers_the_raw_transcript("json").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cleanup_failure_after_a_transcription_answers_the_raw_transcript_as_text() {
    cleanup_failure_answers_the_raw_transcript("text").await;
}

/// D4: a formatter stage past its budget answers the raw transcript and
/// never calls the AI. The hook holds the stage's start past its 50 ms
/// budget, so the formatter's own deadline check fails it.
#[tokio::test(flavor = "multi_thread")]
async fn a_formatter_past_its_budget_answers_the_raw_transcript_without_the_ai() {
    let setup = setup();
    let hook: RequestHook = Arc::new(|point: &str| {
        if point == "dictation:formatter" {
            std::thread::sleep(Duration::from_millis(200));
        }
    });
    let (task, port, signal, _scope) = start(&setup, Some(hook), None).await;
    wait_ready(port).await;

    let (status, post_processing, body) = transcribe(port, "clean", "json").await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(post_processing.as_deref(), Some("failed"));
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["text"], "stub transcript", "{body}");
    let failure = &body["pumice"]["post_processing"];
    assert_eq!(failure["status"], "failed", "{body}");
    assert_eq!(failure["preset"], "builtin:clean");
    assert_eq!(failure["error"]["stage"], "formatter");
    assert_eq!(failure["error"]["code"], "formatter_timeout");
    assert_eq!(body["pumice"]["stages"]["formatter"]["status"], "error");
    assert_eq!(body["pumice"]["stages"]["relay"]["status"], "skipped");
    assert!(!setup.report.exists(), "the AI was never called");

    // Nothing was left behind: admission stays open.
    let (status, body) = send(port, "GET", "/pumice/v1/status", b"").await.unwrap();
    assert_eq!(status, 200);
    assert_eq!(body["dictations"]["admission"], "open", "{body}");

    signal.cancel(CancelReason::Terminate);
    let exit = tokio::time::timeout(WAIT, task).await.unwrap().unwrap();
    assert_eq!(exit, 143);
}
