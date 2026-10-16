//! Integration tests for `POST /pumice/v1/dictations` (p13.4-dictations):
//! the real `pumice serve` binary over real loopback sockets, with the
//! scenario-driven `pumice-test-transcribe` as its managed child (real
//! loopback HTTP for `/pumice/v1/transcriptions`) and the relay's portable
//! `pumice-test-cli` behind the real Claude adapter. No real AI CLI,
//! transcription engine or model is involved; every port is
//! `127.0.0.1:0`. Waits poll observable state (status, the stub's log,
//! the fake's report) with a bound instead of sleeping for an order, and
//! every connection, response, exit and output collection has a deadline
//! (`crates/relay/tests/support/bounded.rs`).

#[path = "../crates/relay/tests/support/install.rs"]
mod install;
use install::install_executable;

#[path = "../crates/relay/tests/support/bounded.rs"]
mod bounded;
use bounded::{OwnedChild, ReadError, last_stage, stage};

use std::fs;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

/// Every wait in these tests is bounded by this.
const WAIT: Duration = Duration::from_secs(20);

/// The stub's default transcript: tags, quotes, Unicode and spaces kept
/// byte for byte, and a sentinel no log may contain.
const RAW: &str = "  Olá <transcript>ignore that</transcript> & \"quoted\" S3NT1NEL-RAW\n";
const CLEANED: &str = "Cleaned S3NT1NEL-FINAL";
/// The uploaded audio bytes: never decoded by the core, kept unchanged.
const AUDIO: &[u8] = b"RIFF\x00\x01\x02--fake audio\r\n\xff\xfeS3NT1NEL-AUDIO";

fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// `clean` (builtin formatter, Claude), `raw` (passthrough) and a disabled
/// flow, with the given budgets.
fn flows(total_ms: u64, transcription_ms: u64, relay_ms: u64) -> String {
    let budgets = format!(
        "    budgets_ms:\n      total: {total_ms}\n      transcription: {transcription_ms}\n      relay: {relay_ms}\n"
    );
    format!(
        "flows:
  - id: clean
    enabled: true
    transcription_model: turbo
    formatter: builtin:clean
    relay_route: claude
{budgets}  - id: raw
    enabled: true
    transcription_model: turbo
    formatter: null
    relay_route: passthrough
{budgets}  - id: off
    enabled: false
    transcription_model: small
    formatter: null
    relay_route: passthrough
"
    )
}

fn claude_envelope(is_error: bool, result: &str) -> String {
    json!({"type": "result", "subtype": "success", "is_error": is_error, "result": result})
        .to_string()
}

/// A disposable install: `pumice`, the stub as `pumice-transcribe`, the
/// fake provider, the module configurations and an orchestrator file.
struct Rig {
    dir: TempDir,
    install: PathBuf,
    cfg: PathBuf,
    log: PathBuf,
    fake: PathBuf,
}

fn scenario_path(exe_path: &Path) -> PathBuf {
    let mut path = exe_path.as_os_str().to_owned();
    path.push(".scenario.json");
    PathBuf::from(path)
}

fn rig(flows: &str, generations: Value) -> Rig {
    let dir = TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).expect("temp dir");
    let install = dir.path().join("bin");
    let cfg = dir.path().join("cfg");
    let fake_dir = dir.path().join("fake");
    for path in [&install, &cfg, &fake_dir] {
        fs::create_dir_all(path).unwrap();
    }
    install_executable(
        Path::new(env!("CARGO_BIN_EXE_pumice")),
        &install.join(exe("pumice")),
    );
    let stub = install.join(exe("pumice-transcribe"));
    install_executable(
        Path::new(env!("CARGO_BIN_EXE_pumice-test-transcribe")),
        &stub,
    );
    let fake = fake_dir.join(exe("fake-claude"));
    install_executable(Path::new(env!("CARGO_BIN_EXE_pumice-test-cli")), &fake);

    let log = dir.path().join("spawns.jsonl");
    let models = json!({
        "schema_version": 1,
        "ok": true,
        "result": [{"id": "turbo", "path": "/models/turbo.gguf", "exists": true, "size": 1, "default": true}]
    });
    fs::write(
        scenario_path(&stub),
        json!({
            "models_list": {"stdout": format!("{models}\n"), "exit_code": 0},
            "serve": {"spawn_log_path": log, "generations": generations},
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        cfg.join("relay.yml"),
        format!(
            "providers:\n  - id: claude\n    enabled: true\n    model: haiku-test\n    binary: '{}'\n",
            fake.display()
        ),
    )
    .unwrap();
    fs::write(cfg.join("formatter.yml"), "schema_version: 1\n").unwrap();
    fs::write(cfg.join("transcribe.yml"), "schema_version: 1\n").unwrap();
    fs::write(
        cfg.join("orchestrator.yml"),
        format!(
            "schema_version: 1\nmodules:\n  transcription:\n    config: ./transcribe.yml\n  formatter:\n    config: ./formatter.yml\n  relay:\n    location: in_process\n    config: ./relay.yml\n{flows}serve:\n  enabled: true\n  port: 0\n  transcription_model: turbo\n"
        ),
    )
    .unwrap();
    let rig = Rig {
        dir,
        install,
        cfg,
        log,
        fake,
    };
    rig.set_fake(json!({"stdout": claude_envelope(false, CLEANED)}));
    rig
}

/// The default stub generation, with `transcription` merged in.
fn child(transcription: Value) -> Value {
    json!({"transcription": transcription})
}

impl Rig {
    fn fake_dir(&self) -> PathBuf {
        self.fake.parent().unwrap().to_path_buf()
    }

    /// Replaces the fake provider's scenario (read per call).
    fn set_fake(&self, mut scenario: Value) {
        scenario["report_path"] = json!(self.fake_dir().join("report.json"));
        scenario["count_path"] = json!(self.fake_dir().join("count.txt"));
        fs::write(scenario_path(&self.fake), scenario.to_string()).unwrap();
    }

    fn fake_calls(&self) -> usize {
        fs::read_to_string(self.fake_dir().join("count.txt"))
            .map(|text| text.lines().count())
            .unwrap_or(0)
    }

    fn fake_report(&self) -> Option<Value> {
        let text = fs::read_to_string(self.fake_dir().join("report.json")).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// The running provider's PID, once the fake has written its report:
    /// the relay stage starts before the provider process does. Only the
    /// Unix tests check a PID.
    #[cfg(unix)]
    fn wait_provider_pid(&self) -> u32 {
        stage("wait for the provider to start");
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(report) = self.fake_report() {
                return report["pid"].as_u64().unwrap() as u32;
            }
            assert!(Instant::now() < deadline, "the provider never started");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn body_path(&self) -> PathBuf {
        self.dir.path().join("child-request.bin")
    }

    /// `pumice serve --json`, until it is ready.
    fn start(&self) -> Running {
        let mut command = Command::new(self.install.join(exe("pumice")));
        command
            .arg("serve")
            .arg("--config")
            .arg(self.cfg.join("orchestrator.yml"))
            .env("XDG_CONFIG_HOME", self.dir.path())
            .env("APPDATA", self.dir.path())
            .env("HOME", self.dir.path())
            .arg("--json")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        stage("start pumice serve");
        let child = command.spawn().expect("start pumice serve");
        let mut running = Running {
            process: OwnedChild::new("pumice serve", child),
            port: 0,
        };
        let listening = running.record();
        assert_eq!(listening["event"], "listening", "{listening}");
        running.port = listening["port"].as_u64().unwrap() as u16;
        running.wait_ready();
        running
    }

    fn log_events(&self, event: &str) -> Vec<Value> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|entry| entry["event"] == event)
            .collect()
    }

    /// Waits until the stub logged `count` transcription requests.
    fn wait_transcriptions(&self, count: usize) {
        stage(format!("wait for {count} transcription request(s)"));
        let deadline = Instant::now() + WAIT;
        while self.log_events("transcription").len() < count {
            assert!(Instant::now() < deadline, "no transcription request");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// No overlapping children, and on Unix none left alive.
    fn assert_children_gone(&self) {
        for spawn in self.log_events("spawn") {
            assert_ne!(spawn["prev_alive"], json!(true), "overlap: {spawn}");
            #[cfg(unix)]
            assert!(
                !pid_alive(spawn["pid"].as_u64().unwrap() as u32),
                "child left behind: {spawn}"
            );
        }
    }
}

/// A running `pumice serve`; dropping it kills and reaps the process.
struct Running {
    process: OwnedChild,
    port: u16,
}

impl Running {
    fn record(&mut self) -> Value {
        stage("wait for a lifecycle record");
        let line = self
            .process
            .stdout_line(Instant::now() + WAIT)
            .expect("a lifecycle record, not the end of stdout");
        serde_json::from_str(&line).unwrap_or_else(|_| panic!("not a JSON record: {line}"))
    }

    fn wait_ready(&mut self) {
        let ready = self.record();
        assert_eq!(ready["event"], "ready", "{ready}");
    }

    fn status(&self) -> Value {
        let (status, _, body) = request(self.port, b"GET", "/pumice/v1/status", &[], &[]);
        assert_eq!(status, 200);
        body
    }

    /// Polls status until `condition` holds.
    fn wait_status(&self, what: &str, condition: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + WAIT;
        loop {
            let body = self.status();
            if condition(&body) {
                return body;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}: {body}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_idle(&self) -> Value {
        self.wait_status("an idle admission", |body| {
            body["dictations"]["active"] == false
        })
    }

    #[cfg(unix)]
    fn signal(&self, signal: libc::c_int) {
        // SAFETY: plain kill(2) on the child this test owns (not reaped
        // before `finish`, so the PID is still its own).
        assert_eq!(
            unsafe { libc::kill(self.process.id() as libc::pid_t, signal) },
            0
        );
    }

    /// Waits for the exit and the whole stderr, both within one [`WAIT`];
    /// returns (exit code, time to the exit, stderr).
    fn finish(mut self) -> (Option<i32>, Duration, String) {
        stage("finish pumice serve");
        let started = Instant::now();
        let deadline = started + WAIT;
        let status = self.process.wait_until(deadline);
        let elapsed = started.elapsed();
        let output = self.process.collect_until(deadline);
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        (status.code(), elapsed, stderr)
    }

    #[cfg_attr(unix, allow(unused_mut))]
    fn stop(mut self) -> (Option<i32>, Duration, String) {
        stage("stop pumice serve");
        #[cfg(unix)]
        self.signal(libc::SIGTERM);
        #[cfg(not(unix))]
        self.process.kill();
        self.finish()
    }
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks for existence.
    if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
        return false;
    }
    // A zombie of a reaped parent's child is gone for this purpose.
    let out = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("ps");
    !String::from_utf8_lossy(&out.stdout).trim().starts_with('Z')
}

#[cfg(unix)]
fn wait_gone(pid: u32) {
    stage(format!("wait for process {pid} to be gone"));
    let deadline = Instant::now() + WAIT;
    while pid_alive(pid) {
        assert!(Instant::now() < deadline, "process {pid} is still alive");
        std::thread::sleep(Duration::from_millis(10));
    }
}

// --- HTTP over real sockets --------------------------------------------------

type HttpResponse = (u16, Vec<(String, String)>, Value);

/// One part of a multipart form: name, optional file name, data.
type Part<'a> = (&'a str, Option<&'a str>, &'a [u8]);

const BOUNDARY: &str = "test-boundary-7d1";

fn form(parts: &[Part]) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, file_name, data) in parts {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        let disposition = match file_name {
            Some(file_name) => {
                format!(
                    "Content-Disposition: form-data; name=\"{name}\"; filename=\"{file_name}\"\r\nContent-Type: audio/wav\r\n\r\n"
                )
            }
            None => format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n"),
        };
        body.extend_from_slice(disposition.as_bytes());
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    body
}

fn head(port: u16, method: &[u8], path: &str, headers: &[(&str, String)]) -> Vec<u8> {
    let mut head = Vec::new();
    head.extend_from_slice(method);
    head.extend_from_slice(format!(" {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n").as_bytes());
    for (name, value) in headers {
        head.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    head.extend_from_slice(b"\r\n");
    head
}

/// The response on `stream` within [`WAIT`]; `None` when the connection
/// ended without one. A response still incomplete at the deadline fails the
/// test.
fn read_response(stream: &mut TcpStream) -> Option<HttpResponse> {
    read_response_until(stream, Instant::now() + WAIT)
}

fn read_response_until(stream: &mut TcpStream, deadline: Instant) -> Option<HttpResponse> {
    match bounded::read_response(stream, deadline) {
        Ok(response) => parse_response(&response),
        Err(ReadError::NoAnswer) => None,
        Err(ReadError::Timeout(got)) => {
            panic!(
                "no complete response in time: {got} (last stage: {})",
                last_stage()
            )
        }
    }
}

/// A connection to the service, made within `deadline`.
fn connect(port: u16, deadline: Instant) -> TcpStream {
    bounded::connect(port, deadline)
        .unwrap_or_else(|error| panic!("connect: {error} (last stage: {})", last_stage()))
}

/// Writes `bytes` within `deadline`; a failure fails the test.
fn send(stream: &mut TcpStream, bytes: &[u8], deadline: Instant) {
    bounded::send(stream, bytes, deadline)
        .unwrap_or_else(|error| panic!("write: {error} (last stage: {})", last_stage()));
}

fn parse_response(response: &[u8]) -> Option<HttpResponse> {
    let split = response.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&response[..split]).ok()?;
    let body = &response[split + 4..];
    let mut lines = head.lines();
    let status = lines.next()?.split(' ').nth(1)?.parse().ok()?;
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    Some((status, headers, serde_json::from_slice(body).ok()?))
}

/// A request with a `Content-Length` body over a fresh connection.
fn request(
    port: u16,
    method: &[u8],
    path: &str,
    headers: &[(&str, String)],
    body: &[u8],
) -> HttpResponse {
    stage(format!("{} {path}", String::from_utf8_lossy(method)));
    let deadline = Instant::now() + WAIT;
    let mut all = headers.to_vec();
    all.push(("Content-Length", body.len().to_string()));
    let mut stream = connect(port, deadline);
    send(&mut stream, &head(port, method, path, &all), deadline);
    // A refusal can come before the body is read: ignore a broken write.
    let _ = bounded::send(&mut stream, body, deadline);
    read_response_until(&mut stream, deadline).expect("a complete JSON response")
}

fn multipart_type() -> (&'static str, String) {
    (
        "Content-Type",
        format!("multipart/form-data; boundary={BOUNDARY}"),
    )
}

fn dictate(port: u16, parts: &[Part]) -> HttpResponse {
    request(
        port,
        b"POST",
        "/pumice/v1/dictations",
        &[multipart_type()],
        &form(parts),
    )
}

fn dictate_flow(port: u16, flow: &str) -> HttpResponse {
    dictate(
        port,
        &[
            ("file", Some("S3NT1NEL-NAME.wav"), AUDIO),
            ("flow", None, flow.as_bytes()),
        ],
    )
}

/// Sends the head and the start of a large body, nothing more: an answer
/// that arrives proves the body was not waited for.
fn dictate_partially(port: u16) -> TcpStream {
    stage("partial POST /pumice/v1/dictations");
    let deadline = Instant::now() + WAIT;
    let body = form(&[
        ("file", Some("a.wav"), &[0u8; 100_000]),
        ("flow", None, b"clean"),
    ]);
    let mut stream = connect(port, deadline);
    let mut bytes = head(
        port,
        b"POST",
        "/pumice/v1/dictations",
        &[multipart_type(), ("Content-Length", body.len().to_string())],
    );
    bytes.extend_from_slice(&body[..64]);
    send(&mut stream, &bytes, deadline);
    stream
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn assert_error(response: &HttpResponse, status: u16, code: &str, stage: &str) {
    let (got, headers, body) = response;
    assert_eq!(*got, status, "{body}");
    assert_eq!(body["schema_version"], 1, "{body}");
    assert_eq!(body["module"], "orchestrator");
    assert_eq!(body["ok"], false);
    assert!(body.get("result").is_none(), "no result on failure: {body}");
    assert_eq!(body["error"]["code"], code, "{body}");
    assert_eq!(body["error"]["stage"], stage, "{body}");
    assert_eq!(body["error"]["retryable"], false);
    assert!(body["error"]["message"].is_string());
    assert!(body["error"]["type"].is_string());
    assert!(body["error"].get("param").is_some());
    assert!(body["error"].get("location").is_none(), "no server paths");
    for stage in ["prepare", "transcription", "formatter", "relay"] {
        assert!(
            body["stages"][stage]["status"].is_string(),
            "{stage}: {body}"
        );
    }
    assert!(body["timings_ms"]["total"].is_number());
    assert_eq!(header(headers, "cache-control"), Some("no-store"));
    assert_eq!(header(headers, "content-type"), Some("application/json"));
    assert!(header(headers, "access-control-allow-origin").is_none());
    let text = body.to_string();
    for sentinel in [
        "S3NT1NEL-NAME",
        "S3NT1NEL-AUDIO",
        "S3NT1NEL-STUB",
        "S3NT1NEL-FLOW",
    ] {
        assert!(!text.contains(sentinel), "{sentinel} echoed: {text}");
    }
}

/// Stderr under `--json`: every line a metadata-only event, never content.
/// Without SIGTERM (not Unix) the service is killed, which can cut its last
/// line short: only that one may be incomplete there.
fn assert_events_metadata_only(stderr: &str) {
    assert!(!stderr.contains("S3NT1NEL"), "content in stderr: {stderr}");
    let lines: Vec<&str> = stderr.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            assert!(
                cfg!(not(unix)) && index + 1 == lines.len(),
                "not an event: {line}"
            );
            continue;
        };
        for key in event.as_object().unwrap().keys() {
            assert!(
                [
                    "schema_version",
                    "operation_id",
                    "seq",
                    "module",
                    "stage",
                    "event",
                    "elapsed_ms",
                    "code",
                    "generation"
                ]
                .contains(&key.as_str()),
                "unexpected event field {key}: {line}"
            );
        }
    }
}

fn events(stderr: &str, operation_id: &str) -> Vec<Value> {
    stderr
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event["operation_id"] == operation_id)
        .collect()
}

// --- The happy paths ---------------------------------------------------------

#[test]
fn a_dictation_runs_the_whole_flow_with_the_exact_texts() {
    let rig = rig(
        &flows(30_000, 10_000, 10_000),
        json!([child(json!({"body": {
            "schema_version": 1, "text": RAW, "language": "pt",
            "audio_seconds": 2.5, "inference_ms": 40
        }}))]),
    );
    // The child also writes what it received.
    let stub = scenario_path(&rig.install.join(exe("pumice-transcribe")));
    let mut scenario: Value = serde_json::from_str(&fs::read_to_string(&stub).unwrap()).unwrap();
    scenario["serve"]["generations"][0]["transcription"]["body_path"] = json!(rig.body_path());
    fs::write(&stub, scenario.to_string()).unwrap();
    rig.set_fake(json!({
        "stdout": claude_envelope(false, CLEANED),
        "report_arg_files": ["--system-prompt-file"],
    }));
    let running = rig.start();

    let (status, headers, body) = dictate_flow(running.port, "clean");
    assert_eq!(status, 200, "{body}");
    assert_eq!(header(&headers, "cache-control"), Some("no-store"));
    assert_eq!(body["schema_version"], 1);
    assert_eq!(body["module"], "orchestrator");
    assert_eq!(body["ok"], true);
    assert_eq!(body["flow"], "clean");
    assert_eq!(body["transcript"]["text"], RAW, "raw byte for byte");
    assert_eq!(body["transcript"]["language"], "pt");
    assert_eq!(body["transcript"]["audio_seconds"], 2.5);
    assert_eq!(body["result"]["text"], CLEANED);
    assert!(body.get("error").is_none());
    for stage in ["prepare", "transcription", "formatter", "relay"] {
        assert_eq!(body["stages"][stage]["status"], "ok", "{stage}: {body}");
        assert!(body["stages"][stage]["elapsed_ms"].is_number());
    }
    assert_eq!(body["stages"]["transcription"]["inference_ms"], 40);
    assert!(
        body["stages"]["transcription"]
            .get("model_load_ms")
            .is_none()
    );
    assert!(body["timings_ms"]["total"].is_number());

    // One provider call: the literal transcript as data, the formatter's
    // exact rules as the system prompt.
    assert_eq!(rig.fake_calls(), 1);
    let report = rig.fake_report().expect("the fake ran");
    assert_eq!(
        report["stdin"],
        json!(RAW),
        "tags are data, never an envelope"
    );
    assert_eq!(
        report["arg_files"]["--system-prompt-file"]["contents"],
        json!(pumice_formatter::builtin_clean().system)
    );

    // The child got a rebuilt form: the exact audio bytes, the flow's
    // model, a fixed file name, its literal Host and no Origin.
    let sent = &rig.log_events("transcription")[0];
    assert_eq!(sent["host_ok"], true, "{sent}");
    assert_eq!(sent["origin"], false);
    assert!(
        sent["content_type"]
            .as_str()
            .unwrap()
            .starts_with("multipart/form-data; boundary=")
    );
    let received = fs::read(rig.body_path()).unwrap();
    let start = received
        .windows(AUDIO.len())
        .position(|window| window == AUDIO)
        .expect("the audio bytes, unchanged");
    assert!(received[start + AUDIO.len()..].starts_with(b"\r\n--"));
    let text = String::from_utf8_lossy(&received);
    assert!(text.contains("name=\"model\"\r\n\r\nturbo\r\n"), "{text}");
    assert!(text.contains("filename=\"audio\""), "{text}");
    assert!(
        !text.contains("S3NT1NEL-NAME"),
        "the client's file name stays"
    );
    assert!(!text.contains("flow"), "only file and model: {text}");

    let status = running.wait_idle();
    assert_eq!(status["dictations"]["admission"], "open");
    assert_eq!(status["transcription"]["generation"], 1);

    let operation_id = body["operation_id"].as_str().unwrap().to_owned();
    let (code, _, stderr) = running.stop();
    if cfg!(unix) {
        assert_eq!(code, Some(143));
    }
    assert_events_metadata_only(&stderr);
    let mine = events(&stderr, &operation_id);
    let names: Vec<&str> = mine
        .iter()
        .map(|event| event["event"].as_str().unwrap())
        .collect();
    assert_eq!(names.first(), Some(&"started"), "{stderr}");
    assert_eq!(names.last(), Some(&"finished"), "{stderr}");
    for stage in ["prepare", "transcription", "formatter", "relay"] {
        assert!(
            mine.iter()
                .any(|event| event["stage"] == stage && event["event"] == "stage_finished"),
            "{stage}: {stderr}"
        );
    }
    rig.assert_children_gone();
}

#[test]
fn raw_only_and_whitespace_dictations_never_call_a_provider() {
    let rig = rig(&flows(30_000, 10_000, 10_000), json!([{}]));
    let running = rig.start();
    let (status, _, body) = dictate_flow(running.port, "raw");
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["transcript"]["text"], "stub transcript");
    assert_eq!(body["result"]["text"], "stub transcript", "passthrough");
    assert_eq!(body["stages"]["formatter"]["status"], "skipped");
    assert_eq!(body["stages"]["relay"]["status"], "ok");
    assert_eq!(rig.fake_calls(), 0, "passthrough never calls a provider");
    drop(running.stop());

    let rig = rig_whitespace();
    let running = rig.start();
    let (status, _, body) = dictate_flow(running.port, "clean");
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["transcript"]["text"], "  \n\t ");
    assert_eq!(body["result"]["text"], "  \n\t ", "unchanged");
    assert_eq!(body["stages"]["formatter"]["status"], "skipped");
    assert_eq!(body["stages"]["relay"]["status"], "skipped");
    assert_eq!(rig.fake_calls(), 0, "no provider call for whitespace");
    drop(running.stop());
    rig.assert_children_gone();
}

fn rig_whitespace() -> Rig {
    rig(
        &flows(30_000, 10_000, 10_000),
        json!([child(json!({"body": {
            "schema_version": 1, "text": "  \n\t ", "audio_seconds": 1.0, "inference_ms": 1
        }}))]),
    )
}

// --- Failures after transcription keep the transcript --------------------

#[test]
fn a_relay_failure_keeps_the_transcript_and_has_no_result() {
    let rig = rig(&flows(30_000, 10_000, 10_000), json!([{}]));
    let running = rig.start();
    for (stdout, exit_code, status, code) in [
        ("garbage S3NT1NEL-STUB".to_owned(), 3, 502, "relay_failed"),
        (
            claude_envelope(false, "   "),
            0,
            502,
            "relay_invalid_output",
        ),
        (
            claude_envelope(true, "Not logged in · Please run /login"),
            0,
            503,
            "relay_not_logged_in",
        ),
        (
            claude_envelope(true, "You've hit your weekly limit · resets Oct 8"),
            0,
            429,
            "relay_quota_exceeded",
        ),
    ] {
        let before = rig.fake_calls();
        rig.set_fake(json!({"stdout": stdout, "exit_code": exit_code}));
        let response = dictate_flow(running.port, "clean");
        assert_error(&response, status, code, "relay");
        let body = &response.2;
        assert_eq!(body["flow"], "clean");
        assert_eq!(body["transcript"]["text"], "stub transcript", "{body}");
        assert_eq!(body["stages"]["transcription"]["status"], "ok");
        assert_eq!(body["stages"]["relay"]["status"], "error");
        assert_eq!(rig.fake_calls(), before + 1, "{code}: one call, no retry");
        assert!(
            header(&response.1, "retry-after").is_none(),
            "never invented"
        );
    }
    // The child was never recycled for a provider failure.
    assert_eq!(running.status()["transcription"]["generation"], 1);
    drop(running.stop());
    rig.assert_children_gone();
}

#[test]
fn a_child_crash_after_its_answer_keeps_the_transcript() {
    let rig = rig(
        &flows(30_000, 10_000, 10_000),
        json!([child(json!({"crash_after_answer": true})), {}]),
    );
    let running = rig.start();
    let (status, _, body) = dictate_flow(running.port, "clean");
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["transcript"]["text"], "stub transcript");
    assert_eq!(body["result"]["text"], CLEANED);
    // The supervisor restarts the crashed child; the next dictation works.
    running.wait_status("generation 2 ready", |status| {
        status["transcription"]["generation"] == 2 && status["transcription"]["state"] == "ready"
    });
    let (status, _, body) = dictate_flow(running.port, "raw");
    assert_eq!(status, 200, "{body}");
    drop(running.stop());
    rig.assert_children_gone();
}

#[test]
fn child_errors_map_to_stable_codes_without_their_messages() {
    let error = |status: u16, code: &str| {
        json!({"status": status, "body": {"error": {
            "message": "S3NT1NEL-STUB child text", "type": "x", "param": null, "code": code
        }}})
    };
    for (script, status, code, cause) in [
        (
            error(400, "unsupported_format"),
            400,
            "transcription_invalid_input",
            Some("unsupported_format"),
        ),
        (
            error(400, "audio_too_long"),
            400,
            "transcription_invalid_input",
            Some("audio_too_long"),
        ),
        (
            error(400, "invalid_audio"),
            400,
            "transcription_invalid_input",
            Some("invalid_input"),
        ),
        (
            error(413, "file_too_large"),
            413,
            "transcription_invalid_input",
            Some("input_too_large"),
        ),
        (
            error(500, "transcription_failed"),
            500,
            "transcription_engine_failed",
            Some("engine_failed"),
        ),
        (
            error(429, "rate_limit_exceeded"),
            503,
            "transcription_busy",
            None,
        ),
        (
            error(400, "missing_model"),
            502,
            "transcription_protocol_error",
            None,
        ),
        (
            json!({"raw_body": "S3NT1NEL-STUB not json"}),
            502,
            "transcription_protocol_error",
            None,
        ),
        (
            json!({"body": {"schema_version": 2, "text": "S3NT1NEL-STUB", "audio_seconds": 1, "inference_ms": 1}}),
            502,
            "transcription_protocol_error",
            None,
        ),
        (
            json!({"raw_body": "{", "repeat_bytes": 1_100_000}),
            502,
            "transcription_protocol_error",
            None,
        ),
    ] {
        let rig = rig(
            &flows(30_000, 10_000, 10_000),
            json!([child(script.clone())]),
        );
        let running = rig.start();
        let response = dictate_flow(running.port, "clean");
        assert_error(&response, status, code, "transcription");
        let body = &response.2;
        assert_eq!(
            body["error"]["cause_code"].as_str(),
            cause,
            "{script}: {body}"
        );
        assert!(body.get("transcript").is_none(), "{body}");
        assert_eq!(body["stages"]["transcription"]["status"], "error");
        assert_eq!(body["stages"]["relay"]["status"], "skipped");
        assert_eq!(rig.fake_calls(), 0, "no provider call");
        let (_, _, stderr) = running.stop();
        assert_events_metadata_only(&stderr);
        rig.assert_children_gone();
    }
}

// --- Deadlines and recycle ----------------------------------------------------

#[test]
fn a_transcription_timeout_recycles_the_child_and_a_relay_timeout_does_not() {
    let rig = rig(
        &flows(30_000, 1_500, 1_500),
        json!([child(json!({"hang": true})), {}]),
    );
    let running = rig.start();
    let started = Instant::now();
    let response = dictate_flow(running.port, "clean");
    assert_error(&response, 504, "transcription_timeout", "transcription");
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(response.2["error"]["type"], "server_error");
    assert!(response.2.get("transcript").is_none());
    // The abandoned generation was killed and reaped before the next one.
    let status = running.wait_status("generation 2 ready", |status| {
        status["transcription"]["generation"] == 2 && status["transcription"]["state"] == "ready"
    });
    assert_eq!(status["dictations"]["admission"], "open");
    #[cfg(unix)]
    wait_gone(rig.log_events("spawn")[0]["pid"].as_u64().unwrap() as u32);

    // A provider that outlives the relay budget: 504 at the relay stage,
    // the transcript kept, the provider reaped, the child left alone.
    rig.set_fake(json!({"stdout": claude_envelope(false, CLEANED), "sleep_ms": 30_000}));
    let response = dictate_flow(running.port, "clean");
    assert_error(&response, 504, "relay_timeout", "relay");
    assert_eq!(response.2["transcript"]["text"], "stub transcript");
    #[cfg(unix)]
    wait_gone(rig.fake_report().unwrap()["pid"].as_u64().unwrap() as u32);
    let status = running.wait_idle();
    assert_eq!(status["transcription"]["generation"], 2, "no recycle");
    assert_eq!(rig.log_events("spawn").len(), 2);
    let (_, _, stderr) = running.stop();
    assert_events_metadata_only(&stderr);
    assert_eq!(
        stderr.matches("child_recycled").count(),
        1,
        "only the transcription timeout recycles: {stderr}"
    );
    rig.assert_children_gone();
}

// --- Admission ---------------------------------------------------------------

#[test]
fn a_concurrent_dictation_is_refused_before_its_body_is_read() {
    let rig = rig(
        &flows(30_000, 10_000, 10_000),
        json!([child(json!({"delay_ms": 1_500}))]),
    );
    let running = rig.start();
    let port = running.port;
    let first = std::thread::spawn(move || dictate_flow(port, "clean"));
    rig.wait_transcriptions(1);

    // Status never waits for the permit, and shows the running stage.
    let started = Instant::now();
    let status = running.status();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "status is cheap"
    );
    assert_eq!(status["dictations"]["active"], true, "{status}");
    assert_eq!(status["dictations"]["admission"], "busy");
    assert_eq!(status["dictations"]["stage"], "transcription");

    // Only the head and 64 of ~100 kB were sent: the answer proves the body
    // was never waited for.
    let mut stream = dictate_partially(port);
    let response = read_response(&mut stream).expect("an answer without the body");
    assert_error(&response, 429, "rate_limit_exceeded", "prepare");
    assert_eq!(header(&response.1, "retry-after"), Some("1"));
    assert_eq!(response.2["error"]["type"], "rate_limit_error");
    assert!(response.2.get("flow").is_none(), "no flow was selected");
    for stage in ["prepare", "transcription", "formatter", "relay"] {
        assert_eq!(response.2["stages"][stage]["status"], "skipped", "{stage}");
    }

    let (status, _, body) = first.join().unwrap();
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        rig.log_events("transcription").len(),
        1,
        "one transcription"
    );
    drop(running.stop());
    rig.assert_children_gone();
}

#[test]
fn an_unready_service_refuses_before_reading_the_body() {
    let rig = rig(
        &flows(30_000, 10_000, 10_000),
        json!([{"crash_after_ready_ms": 300}, {"ready_delay_ms": 60_000}]),
    );
    let running = rig.start();
    running.wait_status("restarting", |status| {
        status["transcription"]["generation"] == 2
    });
    let mut stream = dictate_partially(running.port);
    let response = read_response(&mut stream).expect("an answer without the body");
    assert_error(&response, 503, "transcription_restarting", "prepare");
    assert_eq!(response.2["error"]["type"], "server_error");
    assert!(header(&response.1, "retry-after").is_none());
    assert_eq!(rig.log_events("transcription").len(), 0);
    drop(running.stop());
    rig.assert_children_gone();
}

// --- Request validation -------------------------------------------------------

#[test]
fn invalid_uploads_get_fixed_errors_before_any_work() {
    let rig = rig(&flows(4_000, 1_000, 1_000), json!([{}]));
    let running = rig.start();
    let port = running.port;
    let file: Part = ("file", Some("S3NT1NEL-NAME.wav"), AUDIO);
    let flow: Part = ("flow", None, b"clean");
    for (parts, status, code, param) in [
        (vec![flow], 400, "missing_file", Some("file")),
        (vec![file], 400, "missing_flow", Some("flow")),
        (
            vec![file, flow, file],
            400,
            "duplicate_parameter",
            Some("file"),
        ),
        (
            vec![file, flow, flow],
            400,
            "duplicate_parameter",
            Some("flow"),
        ),
        (
            vec![file, flow, ("S3NT1NEL-FIELD", None, b"x")],
            400,
            "unsupported_parameter",
            None,
        ),
        (
            vec![("file", Some("S3NT1NEL-NAME.wav"), b""), flow],
            400,
            "empty_file",
            Some("file"),
        ),
        (
            vec![("file", None, AUDIO), flow],
            400,
            "file_not_upload",
            Some("file"),
        ),
        (
            vec![file, ("flow", Some("S3NT1NEL-NAME.txt"), b"clean")],
            400,
            "flow_not_text",
            Some("flow"),
        ),
        (
            vec![file, ("flow", None, b"\xffclean")],
            400,
            "invalid_utf8",
            Some("flow"),
        ),
        (
            vec![file, ("flow", None, b"cle\0an")],
            400,
            "nul_in_text",
            Some("flow"),
        ),
        (
            vec![file, ("flow", None, b"S3NT1NEL-FLOW")],
            404,
            "flow_not_found",
            Some("flow"),
        ),
        (
            vec![file, ("flow", None, b"off")],
            400,
            "flow_disabled",
            Some("flow"),
        ),
        (
            vec![file, ("flow", None, &[b'a'; 129])],
            413,
            "field_too_large",
            Some("flow"),
        ),
    ] {
        let response = dictate(port, &parts);
        assert_error(&response, status, code, "prepare");
        assert_eq!(response.2["error"]["param"].as_str(), param, "{code}");
        assert_eq!(response.2["stages"]["prepare"]["status"], "error");
        assert_eq!(response.2["stages"]["transcription"]["status"], "skipped");
        assert!(response.2.get("flow").is_none(), "{code}: never echoed");
    }
    // The client's own reordering is fine: flow first.
    let (status, _, body) = dictate(port, &[flow, ("file", Some("x.wav"), AUDIO)]);
    assert_eq!(status, 200, "{body}");

    // Not multipart, and the wrong method.
    let response = request(port, b"POST", "/pumice/v1/dictations", &[], b"{}");
    assert_error(&response, 400, "not_multipart", "prepare");
    // Every refusal on this route is its envelope, nothing attempted.
    let refused = |response: &HttpResponse, status: u16, code: &str| {
        assert_error(response, status, code, "prepare");
        assert_eq!(response.2["error"]["type"], "invalid_request_error");
        assert!(response.2.get("flow").is_none());
        for stage in ["prepare", "transcription", "formatter", "relay"] {
            assert_eq!(response.2["stages"][stage]["status"], "skipped");
        }
    };
    let response = request(port, b"GET", "/pumice/v1/dictations", &[], b"");
    refused(&response, 405, "method_not_allowed");
    assert_eq!(header(&response.1, "allow"), Some("POST"));
    // The origin guard comes first, before the method too.
    for (method, headers, code) in [
        (
            &b"POST"[..],
            vec![("Origin", "http://localhost".to_owned())],
            "origin_not_allowed",
        ),
        (
            b"GET",
            vec![("Origin", "http://localhost".to_owned())],
            "origin_not_allowed",
        ),
        (
            b"POST",
            vec![("Sec-Fetch-Site", "cross-site".to_owned())],
            "cross_site_not_allowed",
        ),
    ] {
        let mut headers = headers;
        headers.push(multipart_type());
        let response = request(
            port,
            method,
            "/pumice/v1/dictations",
            &headers,
            &form(&[file, flow]),
        );
        refused(&response, 403, code);
        assert!(header(&response.1, "allow").is_none());
    }
    // A wrong Host, written by hand: `head` always sends the right one.
    stage("POST with a wrong Host");
    let deadline = Instant::now() + WAIT;
    let mut stream = connect(port, deadline);
    send(
        &mut stream,
        b"POST /pumice/v1/dictations HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n",
        deadline,
    );
    let response = read_response_until(&mut stream, deadline).expect("an answer");
    refused(&response, 403, "host_not_allowed");

    // A declared length over the limit is refused without the body.
    stage("POST with an oversized Content-Length");
    let deadline = Instant::now() + WAIT;
    let mut stream = connect(port, deadline);
    send(
        &mut stream,
        &head(
            port,
            b"POST",
            "/pumice/v1/dictations",
            &[multipart_type(), ("Content-Length", "26000001".to_owned())],
        ),
        deadline,
    );
    let response = read_response_until(&mut stream, deadline).expect("an early answer");
    assert_error(&response, 413, "body_too_large", "prepare");

    // A chunked body is counted as it arrives.
    let response = chunked_upload(port, 26_000_001);
    assert_error(&response, 413, "body_too_large", "prepare");

    // A file over 25,000,000 bytes inside a body under the total limit.
    let big = vec![b'a'; 25_000_001];
    let response = dictate(port, &[("file", Some("a.wav"), &big), flow]);
    assert_error(&response, 413, "file_too_large", "prepare");

    // A stalled upload: 408 at the upload bound (4 s total − 2.5 s).
    let started = Instant::now();
    let mut stream = dictate_partially(port);
    let response = read_response(&mut stream).expect("an answer");
    assert_error(&response, 408, "upload_timeout", "prepare");
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(1_400) && elapsed < Duration::from_secs(10),
        "{elapsed:?}"
    );

    assert_eq!(
        rig.log_events("transcription").len(),
        1,
        "only the valid one"
    );
    assert_eq!(rig.fake_calls(), 1);
    let (_, _, stderr) = running.stop();
    assert_events_metadata_only(&stderr);
    rig.assert_children_gone();
}

/// A chunked multipart upload of `total` file bytes; the server may refuse
/// it before the end.
fn chunked_upload(port: u16, total: usize) -> HttpResponse {
    stage("chunked POST /pumice/v1/dictations");
    let deadline = Instant::now() + WAIT;
    let mut stream = connect(port, deadline);
    send(
        &mut stream,
        &head(
            port,
            b"POST",
            "/pumice/v1/dictations",
            &[
                multipart_type(),
                ("Transfer-Encoding", "chunked".to_owned()),
            ],
        ),
        deadline,
    );
    let mut send = |data: &[u8]| -> bool {
        let mut chunk = format!("{:x}\r\n", data.len()).into_bytes();
        chunk.extend_from_slice(data);
        chunk.extend_from_slice(b"\r\n");
        bounded::send(&mut stream, &chunk, deadline).is_ok()
    };
    let opening = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"flow\"\r\n\r\nclean\r\n--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\n\r\n"
    );
    let mut ok = send(opening.as_bytes());
    let chunk = vec![b'a'; 1 << 20];
    let mut sent = 0;
    while ok && sent < total {
        let n = chunk.len().min(total - sent);
        ok = send(&chunk[..n]);
        sent += n;
    }
    if ok {
        let _ = send(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
        let _ = bounded::send(&mut stream, b"0\r\n\r\n", deadline);
    }
    read_response_until(&mut stream, deadline).expect("an answer")
}

// --- Client disconnects over real sockets --------------------------------

/// Resets the connection (RST) instead of closing it (FIN).
#[cfg(unix)]
fn reset(stream: TcpStream) {
    use std::os::fd::AsRawFd;
    let linger = libc::linger {
        l_onoff: 1,
        l_linger: 0,
    };
    // SAFETY: a valid socket and a correctly sized `linger`.
    let rc = unsafe {
        libc::setsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_LINGER,
            (&linger as *const libc::linger).cast(),
            std::mem::size_of::<libc::linger>() as libc::socklen_t,
        )
    };
    assert_eq!(rc, 0);
    drop(stream);
}

/// Ends only the write side (FIN) after a complete request and keeps
/// reading: half-close is not supported (p13.4-dictations/D1), so the
/// service treats the client as gone and ends the connection without an
/// answer.
fn half_close(mut stream: TcpStream) {
    stage("half-close");
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    assert!(
        read_response(&mut stream).is_none(),
        "no answer after a half-close"
    );
}

/// How a test ends its side of a connection.
type Close = fn(TcpStream);

/// Sends a whole dictation, then leaves the connection open for the test
/// to close or reset.
fn dictate_and_hold(port: u16, flow: &str) -> TcpStream {
    stage(format!("held POST /pumice/v1/dictations ({flow})"));
    let deadline = Instant::now() + WAIT;
    let body = form(&[
        ("file", Some("a.wav"), AUDIO),
        ("flow", None, flow.as_bytes()),
    ]);
    let mut stream = connect(port, deadline);
    let mut bytes = head(
        port,
        b"POST",
        "/pumice/v1/dictations",
        &[multipart_type(), ("Content-Length", body.len().to_string())],
    );
    bytes.extend_from_slice(&body);
    send(&mut stream, &bytes, deadline);
    stream
}

#[test]
fn a_client_gone_during_upload_cancels_the_dictation() {
    let rig = rig(&flows(30_000, 10_000, 10_000), json!([{}]));
    let running = rig.start();
    let stream = dictate_partially(running.port);
    running.wait_status("an active dictation", |status| {
        status["dictations"]["active"] == true
    });
    drop(stream);
    running.wait_idle();
    assert_eq!(rig.log_events("transcription").len(), 0);
    // The service takes the next dictation at once.
    let (status, _, body) = dictate_flow(running.port, "raw");
    assert_eq!(status, 200, "{body}");
    let (_, _, stderr) = running.stop();
    assert_events_metadata_only(&stderr);
    assert!(
        stderr
            .lines()
            .any(|line| line.contains("\"stage\":\"prepare\"")
                && line.contains("\"code\":\"cancelled\"")),
        "the upload was cancelled: {stderr}"
    );
    rig.assert_children_gone();
}

#[test]
fn a_client_gone_during_transcription_recycles_the_child() {
    let rig = rig(
        &flows(60_000, 50_000, 10_000),
        json!([child(json!({"hang": true}))]),
    );
    let running = rig.start();
    // A reset is only produced portably on Unix.
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut closes: Vec<(&str, Close)> =
        vec![("close", drop::<TcpStream>), ("half-close", half_close)];
    #[cfg(unix)]
    closes.push(("reset", reset));
    for (round, (how, close)) in closes.into_iter().enumerate() {
        let generation = round as u64 + 1;
        let stream = dictate_and_hold(running.port, "clean");
        rig.wait_transcriptions(round + 1);
        let started = Instant::now();
        close(stream);
        // The abandoned generation is killed, reaped and replaced well
        // before any deadline (50 s).
        running.wait_status("the next generation ready", |status| {
            status["transcription"]["generation"] == generation + 1
                && status["transcription"]["state"] == "ready"
                && status["dictations"]["active"] == false
        });
        assert!(started.elapsed() < Duration::from_secs(10), "{how}");
        #[cfg(unix)]
        wait_gone(rig.log_events("spawn")[round]["pid"].as_u64().unwrap() as u32);
    }
    assert_eq!(rig.fake_calls(), 0);
    let (_, _, stderr) = running.stop();
    assert_events_metadata_only(&stderr);
    assert!(stderr.contains("child_recycled"), "{stderr}");
    rig.assert_children_gone();
}

#[cfg(unix)]
#[test]
fn a_client_gone_during_the_provider_call_reaps_it_without_recycling() {
    // Unix only: the provider's PID is checked with kill(2).
    let rig = rig(&flows(60_000, 10_000, 50_000), json!([{}]));
    rig.set_fake(json!({"stdout": claude_envelope(false, CLEANED), "sleep_ms": 60_000}));
    let running = rig.start();
    for (round, close) in [drop::<TcpStream> as Close, half_close, reset]
        .into_iter()
        .enumerate()
    {
        let _ = fs::remove_file(rig.fake_dir().join("report.json"));
        let stream = dictate_and_hold(running.port, "clean");
        let pid = rig.wait_provider_pid();
        running.wait_status("the relay stage", |status| {
            status["dictations"]["stage"] == "relay"
        });
        close(stream);
        wait_gone(pid);
        let status = running.wait_idle();
        assert_eq!(
            status["transcription"]["generation"], 1,
            "round {round}: no recycle"
        );
        assert_eq!(status["dictations"]["admission"], "open");
    }
    let (_, _, stderr) = running.stop();
    assert_events_metadata_only(&stderr);
    assert!(!stderr.contains("child_recycled"), "{stderr}");
    assert!(
        stderr.contains("\"stage\":\"relay\"") && stderr.contains("\"code\":\"cancelled\""),
        "{stderr}"
    );
    rig.assert_children_gone();
}

// --- Shutdown -----------------------------------------------------------------

#[cfg(unix)]
#[test]
fn a_shutdown_answers_a_running_dictation_with_its_transcript() {
    // Unix only: SIGTERM.
    let rig = rig(&flows(60_000, 10_000, 50_000), json!([{}]));
    rig.set_fake(json!({"stdout": claude_envelope(false, CLEANED), "sleep_ms": 60_000}));
    let running = rig.start();
    let mut stream = dictate_and_hold(running.port, "clean");
    running.wait_status("the relay stage", |status| {
        status["dictations"]["stage"] == "relay"
    });
    let pid = rig.wait_provider_pid();
    running.signal(libc::SIGTERM);
    let response = read_response(&mut stream).expect("an answer before the stop");
    assert_error(&response, 503, "cancelled", "relay");
    assert_eq!(response.2["error"]["cause_code"], "service_shutdown");
    assert_eq!(response.2["transcript"]["text"], "stub transcript");
    assert_eq!(response.2["stages"]["relay"]["status"], "cancelled");
    let (code, elapsed, stderr) = running.finish();
    assert_eq!(code, Some(143), "{stderr}");
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    assert!(!pid_alive(pid));
    assert_events_metadata_only(&stderr);
    rig.assert_children_gone();
}

// --- Cleanup failures -------------------------------------------------------

/// Leftovers of [`Rig::use_locking_provider`]: the temporary roots it left
/// behind are made writable again and removed when the test ends, also
/// when it fails.
#[cfg(unix)]
struct Locked {
    record: PathBuf,
}

#[cfg(unix)]
impl Drop for Locked {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let text = fs::read_to_string(&self.record).unwrap_or_default();
        for cwd in text.lines().map(PathBuf::from) {
            let _ = fs::set_permissions(cwd.join("locked"), fs::Permissions::from_mode(0o700));
            // The provider ran in `<root>/workspace`; the root is Pumice's.
            if let Some(root) = cwd.parent() {
                let _ = fs::remove_dir_all(root);
            }
        }
    }
}

#[cfg(unix)]
impl Rig {
    /// Replaces the provider with a script that answers `stdout` with
    /// `exit_code` and leaves, in its working directory, a directory
    /// Pumice cannot delete (no write permission on it, so its entry
    /// cannot be unlinked). Unix only: file permissions and `sh`.
    fn use_locking_provider(&self, stdout: &str, exit_code: i32) -> Locked {
        use std::os::unix::fs::PermissionsExt;
        let fake_dir = self.fake_dir();
        let record = fake_dir.join("locked-cwds.txt");
        let reply = fake_dir.join("reply.txt");
        fs::write(&reply, stdout).unwrap();
        // Written as a plain file and copied into place by a `cp` child, so
        // this process never holds the script open for writing: a test
        // thread forking meanwhile could make running it fail with "text
        // file busy" (ETXTBSY).
        let source = fake_dir.join("locking-claude.sh");
        fs::write(
            &source,
            format!(
                "#!/bin/sh\ncat >/dev/null\necho x >> '{count}'\npwd >> '{record}'\nmkdir -p locked/inner\nchmod 500 locked\ncat '{reply}'\nexit {exit_code}\n",
                count = fake_dir.join("count.txt").display(),
                record = record.display(),
                reply = reply.display(),
            ),
        )
        .unwrap();
        let script = fake_dir.join("locking-claude");
        install::copy_executable(&source, &script);
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            self.cfg.join("relay.yml"),
            format!(
                "providers:\n  - id: claude\n    enabled: true\n    model: haiku-test\n    binary: '{}'\n",
                script.display()
            ),
        )
        .unwrap();
        Locked { record }
    }
}

#[cfg(unix)]
#[test]
fn a_failed_directory_cleanup_stops_admission_also_after_another_failure() {
    // SAFETY: geteuid(2) has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipped: root ignores the permissions this test relies on");
        return;
    }
    let rig = rig(&flows(30_000, 10_000, 10_000), json!([{}, {}]));
    for (round, (stdout, exit_code, status, code, stage)) in [
        // A dictation that would have succeeded.
        (
            claude_envelope(false, CLEANED),
            0,
            500,
            "cleanup_failed",
            "cleanup",
        ),
        // A relay failure stays the reported error; the cleanup failure
        // after it still stops admission.
        ("garbage".to_owned(), 3, 502, "relay_failed", "relay"),
    ]
    .into_iter()
    .enumerate()
    {
        let _locked = rig.use_locking_provider(&stdout, exit_code);
        let running = rig.start();
        let response = dictate_flow(running.port, "clean");
        assert_error(&response, status, code, stage);
        assert_eq!(response.2["transcript"]["text"], "stub transcript");
        assert_eq!(rig.fake_calls(), round + 1);

        // No second provider call: admission stopped for good.
        let status = running.status();
        assert_eq!(status["dictations"]["admission"], "stopped", "{status}");
        let response = dictate_flow(running.port, "clean");
        assert_error(&response, 503, "admission_stopped", "prepare");
        assert_eq!(rig.fake_calls(), round + 1, "round {round}");

        let (_, _, stderr) = running.stop();
        assert_events_metadata_only(&stderr);
        assert!(
            stderr.contains("\"event\":\"admission_stopped\"")
                && stderr.contains("\"code\":\"cleanup_failed\""),
            "{stderr}"
        );
    }
    rig.assert_children_gone();
}
