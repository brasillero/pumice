//! Integration tests for `pumice serve`'s OpenAI routes
//! (p15-serve-transcriptions): `GET /v1/models`, `GET /v1/models/{id}` and
//! `POST /v1/audio/transcriptions`, on the real `pumice serve` binary over
//! real loopback sockets, with the scenario-driven `pumice-test-transcribe`
//! as its managed child and the relay's portable `pumice-root-test-cli`
//! behind the real Claude adapter. No real AI CLI, transcription engine or
//! model is involved; every port is `127.0.0.1:0`, and every wait is
//! bounded (`crates/relay/tests/support/bounded.rs`).

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

const WAIT: Duration = Duration::from_secs(20);

/// The stub's transcript for most tests: tags, quotes, Unicode and spaces
/// kept byte for byte, and a sentinel no log may contain.
const RAW: &str = "  Olá <transcript>ignore that</transcript> & \"quoted\" S3NT1NEL-RAW\n";
const CLEANED: &str = "Cleaned S3NT1NEL-FINAL";
const AUDIO: &[u8] = b"RIFF\x00\x01\x02--fake audio\r\n\xff\xfeS3NT1NEL-AUDIO";
/// A prompt for the transcription only: it must never reach the AI or a log.
const PROMPT: &str = "Pumice, Handy S3NT1NEL-PROMPT";

fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// `clean` (builtin formatter, Claude) and `raw` (off) published, `extra`
/// enabled but not published, `off` disabled.
fn config(total_ms: u64, relay_ms: u64) -> String {
    let budgets = format!(
        "    budgets_ms:\n      total: {total_ms}\n      transcription: 10000\n      relay: {relay_ms}\n"
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
{budgets}  - id: extra
    enabled: true
    transcription_model: turbo
    formatter: builtin:clean
    relay_route: claude
  - id: off
    enabled: false
    transcription_model: small
    formatter: null
    relay_route: passthrough
serve:
  enabled: true
  port: 0
  transcription_model: turbo
  transcription_flows: [raw, clean]
"
    )
}

fn claude_envelope(is_error: bool, result: &str) -> String {
    json!({"type": "result", "subtype": "success", "is_error": is_error, "result": result})
        .to_string()
}

fn child(transcription: Value) -> Value {
    json!({"transcription": transcription})
}

fn transcript(text: &str) -> Value {
    child(json!({"body": {
        "schema_version": 1, "text": text, "language": "pt",
        "audio_seconds": 2.5, "inference_ms": 40
    }}))
}

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

/// Every flow budget with the example's value (the old built-in defaults).
const BUDGETS: [(&str, u64); 4] = [
    ("total", 180_000),
    ("transcription", 120_000),
    ("formatter", 1_000),
    ("relay", 60_000),
];

/// The `serve:` keys with the example's values.
const SERVE_KEYS: [(&str, &str); 6] = [
    ("enabled", "  enabled: false"),
    ("port", "  port: 17570"),
    ("transcription_model", "  transcription_model: null"),
    ("startup_timeout_ms", "  startup_timeout_ms: 60000"),
    ("transcription_flows", "  transcription_flows: []"),
    ("allowed_origins", "  allowed_origins: []"),
];

/// `config` (a `flows:` list, then a `serve:` section, both with the keys
/// under test) with every setting it does not write: each flow's budgets
/// and the section's keys get the example's values (every setting is
/// required). A test writes `budgets_ms` last in a flow.
fn complete(config: &str) -> String {
    let (flows, serve) = config.split_once("serve:\n").expect("a serve section");
    let mut out = String::new();
    let mut entry: Vec<&str> = Vec::new();
    let mut flush = |entry: &mut Vec<&str>| {
        if entry.is_empty() {
            return;
        }
        for line in entry.iter() {
            out.push_str(line);
            out.push('\n');
        }
        if entry[0].starts_with("  - ") {
            if !entry.iter().any(|line| line.trim() == "budgets_ms:") {
                out.push_str("    budgets_ms:\n");
            }
            for (key, value) in BUDGETS {
                let written = format!("      {key}:");
                if !entry.iter().any(|line| line.starts_with(&written)) {
                    out.push_str(&format!("{written} {value}\n"));
                }
            }
        }
        entry.clear();
    };
    for line in flows.lines() {
        if line.starts_with("  - ") {
            flush(&mut entry);
        }
        entry.push(line);
    }
    flush(&mut entry);
    out.push_str("serve:\n");
    out.push_str(serve);
    for (key, line) in SERVE_KEYS {
        if !serve
            .lines()
            .any(|written| written.starts_with(&format!("  {key}:")))
        {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn rig(config: &str, generations: Value) -> Rig {
    let config = &complete(config);
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
    install_executable(Path::new(env!("CARGO_BIN_EXE_pumice-root-test-cli")), &fake);

    let log = dir.path().join("spawns.jsonl");
    let models = json!({
        "schema_version": 1,
        "ok": true,
        "result": [{"id": "turbo", "path": "/models/turbo.gguf", "exists": true, "size": 1, "default": true}]
    });
    // Every generation also writes the form it received.
    let mut generations = generations;
    for generation in generations.as_array_mut().unwrap() {
        let transcription = generation
            .as_object_mut()
            .unwrap()
            .entry("transcription")
            .or_insert_with(|| json!({}));
        transcription["body_path"] = json!(dir.path().join("child-request.bin"));
    }
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
            "port: 7567\ntotal_timeout_secs: 30\nmax_parallel: 4\nproviders:\n  - id: claude\n    enabled: true\n    model: haiku-test\n    binary: '{}'\n    timeout_secs: 30\ndebug_log:\n  enabled: false\n  path: pumice-debug.jsonl\n",
            fake.display()
        ),
    )
    .unwrap();
    fs::write(
        cfg.join("formatter.yml"),
        "schema_version: 1\nformatters: []\ndefault: null\n",
    )
    .unwrap();
    fs::write(cfg.join("transcribe.yml"), "schema_version: 1\ndefault_model: null\nlanguage: null\nthreads: auto\nbackend: cpu\nserver:\n  port: null\nmodels_dir: null\nmodels: []\n").unwrap();
    fs::write(
        cfg.join("orchestrator.yml"),
        format!(
            "schema_version: 1\nmodules:\n  transcription:\n    config: ./transcribe.yml\n  formatter:\n    config: ./formatter.yml\n  relay:\n    location: in_process\n    config: ./relay.yml\n{config}"
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
    rig.set_fake(json!({
        "stdout": claude_envelope(false, CLEANED),
        "report_arg_files": ["--system-prompt-file"],
    }));
    rig
}

impl Rig {
    fn fake_dir(&self) -> PathBuf {
        self.fake.parent().unwrap().to_path_buf()
    }

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

    /// The last form the child received.
    fn child_request(&self) -> String {
        String::from_utf8_lossy(&fs::read(self.dir.path().join("child-request.bin")).unwrap())
            .into_owned()
    }

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
        let ready = running.record();
        assert_eq!(ready["event"], "ready", "{ready}");
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

    fn wait_log(&self, event: &str, count: usize) {
        stage(format!("wait for {count} {event} event(s)"));
        let deadline = Instant::now() + WAIT;
        while self.log_events(event).len() < count {
            assert!(Instant::now() < deadline, "no {event}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

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
            .expect("a lifecycle record");
        serde_json::from_str(&line).unwrap_or_else(|_| panic!("not a JSON record: {line}"))
    }

    fn get(&self, path: &str) -> (u16, Value) {
        let (status, _, body) = request(self.port, b"GET", path, &[], b"");
        (status, json_body(&body))
    }

    fn wait_status(&self, what: &str, condition: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + WAIT;
        loop {
            let (_, body) = self.get("/pumice/v1/status");
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

    /// Stops the service (SIGTERM on Unix) and returns its stderr.
    #[cfg_attr(unix, allow(unused_mut))]
    fn stop(mut self) -> String {
        stage("stop pumice serve");
        #[cfg(unix)]
        // SAFETY: plain kill(2) on the child this test owns.
        assert_eq!(
            unsafe { libc::kill(self.process.id() as libc::pid_t, libc::SIGTERM) },
            0
        );
        #[cfg(not(unix))]
        self.process.kill();
        let deadline = Instant::now() + WAIT;
        self.process.wait_until(deadline);
        let output = self.process.collect_until(deadline);
        String::from_utf8_lossy(&output.stderr).into_owned()
    }
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks for existence.
    if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
        return false;
    }
    let out = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("ps");
    !String::from_utf8_lossy(&out.stdout).trim().starts_with('Z')
}

// --- HTTP over real sockets --------------------------------------------------

/// Status, lowercased headers and the raw body.
type HttpResponse = (u16, Vec<(String, String)>, Vec<u8>);
type Part<'a> = (&'a str, Option<&'a str>, &'a [u8]);

const BOUNDARY: &str = "test-boundary-p15";

fn form(parts: &[Part]) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, file_name, data) in parts {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        let disposition = match file_name {
            Some(file_name) => format!(
                "Content-Disposition: form-data; name=\"{name}\"; filename=\"{file_name}\"\r\nContent-Type: audio/wav\r\n\r\n"
            ),
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
    head.extend_from_slice(format!(" {path} HTTP/1.1\r\n").as_bytes());
    // The loopback Host unless the test names its own.
    if !headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("host"))
    {
        head.extend_from_slice(format!("Host: 127.0.0.1:{port}\r\n").as_bytes());
    }
    for (name, value) in headers {
        head.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    head.extend_from_slice(b"\r\n");
    head
}

fn parse_response(response: &[u8]) -> Option<HttpResponse> {
    let split = response.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&response[..split]).ok()?;
    let mut lines = head.lines();
    let status = lines.next()?.split(' ').nth(1)?.parse().ok()?;
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    Some((status, headers, response[split + 4..].to_vec()))
}

fn read_response(stream: &mut TcpStream, deadline: Instant) -> Option<HttpResponse> {
    match bounded::read_response(stream, deadline) {
        Ok(response) => parse_response(&response),
        Err(ReadError::NoAnswer) => None,
        Err(ReadError::Timeout(got)) => panic!(
            "no complete response in time: {got} (last stage: {})",
            last_stage()
        ),
    }
}

fn connect(port: u16, deadline: Instant) -> TcpStream {
    bounded::connect(port, deadline)
        .unwrap_or_else(|error| panic!("connect: {error} (last stage: {})", last_stage()))
}

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
    // A refusal can come before the body is read, and the connection then
    // closes with the body unread, which resets it. The request goes out in
    // one write, so a small one reaches the service in one piece and leaves
    // nothing unread; a broken write still reads the answer.
    let mut request = head(port, method, path, &all);
    request.extend_from_slice(body);
    let _ = bounded::send(&mut stream, &request, deadline);
    read_response(&mut stream, deadline).expect("a complete response")
}

fn json_body(body: &[u8]) -> Value {
    serde_json::from_slice(body)
        .unwrap_or_else(|_| panic!("not JSON: {}", String::from_utf8_lossy(body)))
}

fn multipart_type() -> (&'static str, String) {
    (
        "Content-Type",
        format!("multipart/form-data; boundary={BOUNDARY}"),
    )
}

fn transcribe(port: u16, parts: &[Part]) -> HttpResponse {
    request(
        port,
        b"POST",
        "/v1/audio/transcriptions",
        &[multipart_type()],
        &form(parts),
    )
}

fn transcribe_model(port: u16, model: &str) -> HttpResponse {
    transcribe(
        port,
        &[
            ("file", Some("S3NT1NEL-NAME.wav"), AUDIO),
            ("model", None, model.as_bytes()),
        ],
    )
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// An OpenAI error: exactly the four fields, no echo of the request.
fn assert_openai_error(response: &HttpResponse, status: u16, code: &str, param: Option<&str>) {
    let (got, headers, body) = response;
    let body = json_body(body);
    assert_eq!(*got, status, "{body}");
    let error = body["error"].as_object().expect("an error object");
    assert_eq!(
        error.keys().map(String::as_str).collect::<Vec<_>>(),
        ["message", "type", "param", "code"],
        "{body}"
    );
    assert_eq!(body.as_object().unwrap().len(), 1, "only `error`: {body}");
    assert_eq!(body["error"]["code"], code, "{body}");
    assert_eq!(body["error"]["param"].as_str(), param, "{body}");
    assert!(body["error"]["message"].is_string());
    let kind = body["error"]["type"].as_str().unwrap();
    match status {
        429 => assert_eq!(kind, "rate_limit_error"),
        400..=499 => assert_eq!(kind, "invalid_request_error"),
        _ => assert_eq!(kind, "server_error"),
    }
    assert_eq!(header(headers, "cache-control"), Some("no-store"));
    assert_eq!(header(headers, "content-type"), Some("application/json"));
    assert!(header(headers, "pumice-postprocessing-status").is_none());
    assert!(header(headers, "access-control-allow-origin").is_none());
    assert!(!body.to_string().contains("S3NT1NEL"), "echoed: {body}");
}

/// Stderr under `--json`: metadata-only events, never content.
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

// --- Models ------------------------------------------------------------------

#[test]
fn the_models_are_the_published_flows() {
    let rig = rig(&config(30_000, 10_000), json!([{}]));
    let running = rig.start();

    let (status, body) = running.get("/v1/models");
    assert_eq!(status, 200);
    let model = |id: &str, preset: Value, route: &str| {
        json!({
            "id": id,
            "object": "model",
            "created": 0,
            "owned_by": "pumice",
            "pumice": {
                "schema_version": 1,
                "flow": id,
                "transcription_model": "turbo",
                "post_processing": {"preset": preset, "relay_route": route},
                "response_formats": ["json", "text"],
                "ready": true,
            },
        })
    };
    // In `serve.transcription_flows` order; `extra` (enabled, unpublished)
    // and `off` (disabled) are not models.
    assert_eq!(
        body,
        json!({"object": "list", "data": [
            model("raw", Value::Null, "passthrough"),
            model("clean", json!("builtin:clean"), "claude"),
        ]})
    );
    let (status, body) = running.get("/v1/models/clean");
    assert_eq!(status, 200);
    assert_eq!(body, model("clean", json!("builtin:clean"), "claude"));
    for path in ["/v1/models/extra", "/v1/models/off", "/v1/models/CLEAN"] {
        let response = request(running.port, b"GET", path, &[], b"");
        assert_openai_error(&response, 404, "model_not_found", Some("model"));
    }
    let (_, status) = running.get("/pumice/v1/status");
    assert_eq!(status["flows"], json!({"enabled": 3, "published": 2}));
    // No API key: an OpenAI client's Authorization header is ignored.
    let (status, _, body) = request(
        running.port,
        b"GET",
        "/v1/models",
        &[("Authorization", "Bearer sk-anything".to_owned())],
        b"",
    );
    assert_eq!(status, 200);
    assert_eq!(json_body(&body)["data"].as_array().unwrap().len(), 2);
    drop(running.stop());
    // Listing never started a transcription or a provider.
    assert!(rig.log_events("transcription").is_empty());
    assert_eq!(rig.fake_calls(), 0);
    rig.assert_children_gone();
}

#[test]
fn a_model_list_past_its_bound_is_refused_at_startup() {
    // Every field within its own limit (128 published flows, 128-character
    // ids, a 512-byte served model), but `/v1/models` in its largest state
    // would exceed 128 KiB: a located configuration error, before any child.
    let model = "m".repeat(512);
    let ids: Vec<String> = (0..128)
        .map(|index| format!("{index:03}{}", "f".repeat(125)))
        .collect();
    let mut config = String::from("flows:\n");
    for id in &ids {
        config.push_str(&format!(
            "  - id: {id}\n    enabled: true\n    transcription_model: {model}\n    formatter: null\n    relay_route: passthrough\n"
        ));
    }
    config.push_str(&format!(
        "serve:\n  enabled: true\n  port: 0\n  transcription_model: {model}\n  transcription_flows:\n"
    ));
    for id in &ids {
        config.push_str(&format!("    - {id}\n"));
    }
    let rig = rig(&config, json!([{}]));
    let child = Command::new(rig.install.join(exe("pumice")))
        .arg("serve")
        .arg("--config")
        .arg(rig.cfg.join("orchestrator.yml"))
        .arg("--json")
        .env("XDG_CONFIG_HOME", rig.dir.path())
        .env("APPDATA", rig.dir.path())
        .env("HOME", rig.dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start pumice serve");
    let output = OwnedChild::new("pumice serve", child).finish(WAIT);
    assert_eq!(output.status.code(), Some(2));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let record: Value = serde_json::from_str(stdout.trim()).expect("one record");
    assert_eq!(record["event"], "error", "{record}");
    assert_eq!(record["code"], "invalid_config", "{record}");
    let message = record["message"].as_str().unwrap();
    assert!(message.contains("GET /v1/models"), "{message}");
    // Located at a `serve.transcription_flows` entry: `- <id>` on its line.
    let (location, _) = message.split_once(": ").unwrap();
    let line: usize = location.rsplit(':').nth(1).unwrap().parse().unwrap();
    let text = fs::read_to_string(rig.cfg.join("orchestrator.yml")).unwrap();
    let located = text.lines().nth(line - 1).unwrap();
    assert!(
        located.trim_start().starts_with("- ") && ids.contains(&located.trim()[2..].to_owned()),
        "{located}"
    );
    assert!(rig.log_events("spawn").is_empty(), "no child spawned");
}

#[test]
fn an_unready_service_lists_its_models_as_unready_and_refuses_work() {
    let rig = rig(
        &config(30_000, 10_000),
        json!([{"crash_after_ready_ms": 300}, {"ready_delay_ms": 60_000}]),
    );
    let running = rig.start();
    running.wait_status("restarting", |status| {
        status["transcription"]["generation"] == 2
    });
    rig.wait_log("spawn", 2);
    let (status, body) = running.get("/v1/models");
    assert_eq!(status, 200);
    for model in body["data"].as_array().unwrap() {
        assert_eq!(model["pumice"]["ready"], false, "{model}");
        assert_eq!(model["pumice"]["unavailable"], "transcription_restarting");
    }
    let response = transcribe_model(running.port, "clean");
    assert_openai_error(&response, 503, "transcription_restarting", None);
    assert!(rig.log_events("transcription").is_empty());
    drop(running.stop());
    rig.assert_children_gone();
}

// --- Transcriptions ----------------------------------------------------------

#[test]
fn a_transcription_answers_the_final_text_and_forwards_its_options() {
    let rig = rig(&config(30_000, 10_000), json!([transcript(RAW)]));
    let running = rig.start();
    let port = running.port;

    let (status, headers, body) = transcribe(
        port,
        &[
            ("model", None, b"clean"),
            ("language", None, b"PT"),
            ("prompt", None, PROMPT.as_bytes()),
            ("temperature", None, b"0.2"),
            ("response_format", None, b"json"),
            ("stream", None, b"false"),
            ("file", Some("S3NT1NEL-NAME.wav"), AUDIO),
        ],
    );
    assert_eq!(status, 200);
    assert_eq!(header(&headers, "content-type"), Some("application/json"));
    assert_eq!(header(&headers, "cache-control"), Some("no-store"));
    assert_eq!(
        header(&headers, "pumice-postprocessing-status"),
        Some("applied")
    );
    let body = json_body(&body);
    assert_eq!(body["text"], CLEANED);
    let pumice = &body["pumice"];
    assert_eq!(pumice["schema_version"], 1);
    assert!(pumice["operation_id"].is_string());
    assert_eq!(pumice["flow"], "clean");
    assert_eq!(
        pumice["transcript"],
        json!({"text": RAW, "language": "pt", "audio_seconds": 2.5})
    );
    assert_eq!(
        pumice["post_processing"],
        json!({"status": "applied", "preset": "builtin:clean", "relay_route": "claude"})
    );
    for stage in ["prepare", "transcription", "formatter", "relay"] {
        assert_eq!(pumice["stages"][stage]["status"], "ok", "{stage}: {body}");
    }
    assert_eq!(pumice["stages"]["transcription"]["inference_ms"], 40);
    assert!(pumice["timings_ms"]["total"].is_number());

    // The child got the options as its own fields; the client's file name
    // and the model id stay behind.
    let sent = rig.child_request();
    for (name, value) in [
        ("model", "turbo"),
        ("language", "pt"),
        ("prompt", PROMPT),
        ("temperature", "0.2"),
    ] {
        assert!(
            sent.contains(&format!("name=\"{name}\"\r\n\r\n{value}\r\n")),
            "{name}: {sent}"
        );
    }
    assert!(
        !sent.contains("S3NT1NEL-NAME") && !sent.contains("clean"),
        "{sent}"
    );
    assert!(!sent.contains("name=\"response_format\"") && !sent.contains("name=\"stream\""));
    // The AI got the raw transcript and the formatter's rules, never the
    // prompt or the language.
    assert_eq!(rig.fake_calls(), 1);
    let report = rig.fake_report().expect("the fake ran");
    assert_eq!(report["stdin"], json!(RAW));
    assert_eq!(
        report["arg_files"]["--system-prompt-file"]["contents"],
        json!(pumice_formatter::builtin_clean().system)
    );
    assert!(!report.to_string().contains("S3NT1NEL-PROMPT"), "{report}");

    // `text`: the exact final text, the status in its header. Without
    // options, none reach the child.
    let (status, headers, body) = transcribe(
        port,
        &[
            ("file", Some("a.wav"), AUDIO),
            ("model", None, b"clean"),
            ("response_format", None, b"text"),
        ],
    );
    assert_eq!(status, 200);
    assert_eq!(
        header(&headers, "content-type"),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(
        header(&headers, "pumice-postprocessing-status"),
        Some("applied")
    );
    assert_eq!(body, CLEANED.as_bytes());
    for absent in ["language", "prompt", "temperature"] {
        assert!(!rig.child_request().contains(absent), "{absent}");
    }

    running.wait_idle();
    let stderr = running.stop();
    assert_events_metadata_only(&stderr);
    rig.assert_children_gone();
}

/// What a tailnet browser sends through `tailscale serve`: its Host and
/// Origin, with the listener still on loopback.
#[test]
fn a_listed_origin_reaches_every_route_through_a_proxy() {
    const ORIGIN: &str = "https://server-1.tail59f332.ts.net:17570";
    let text = format!("{}  allowed_origins: [{ORIGIN}]\n", config(30_000, 10_000));
    let rig = rig(&text, json!([transcript(RAW)]));
    let running = rig.start();
    let port = running.port;
    let proxied = |site: &str| {
        vec![
            ("Host", "server-1.tail59f332.ts.net:17570".to_owned()),
            ("Origin", ORIGIN.to_owned()),
            ("Sec-Fetch-Site", site.to_owned()),
        ]
    };

    // The page on loopback, GET only.
    let (status, headers, _) = request(port, b"GET", "/page.js", &[], b"");
    assert_eq!(status, 200);
    assert_eq!(
        header(&headers, "content-type"),
        Some("text/javascript; charset=utf-8")
    );
    let response = request(port, b"HEAD", "/", &[], b"");
    assert_eq!(response.0, 405);
    assert_eq!(header(&response.1, "allow"), Some("GET"));

    let (status, headers, body) = request(port, b"GET", "/", &proxied("none")[..1], b"");
    assert_eq!(status, 200);
    assert_eq!(
        header(&headers, "content-type"),
        Some("text/html; charset=utf-8")
    );
    assert!(header(&headers, "content-security-policy").is_some());
    assert!(String::from_utf8_lossy(&body).starts_with("<!doctype html>"));
    let (status, _, body) = request(port, b"GET", "/v1/models", &proxied("same-origin"), b"");
    assert_eq!(status, 200);
    assert_eq!(json_body(&body)["data"][0]["id"], "raw");
    let (status, _, body) = request(
        port,
        b"GET",
        "/pumice/v1/status",
        &proxied("same-origin"),
        b"",
    );
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));

    let mut headers = proxied("same-origin");
    headers.push(multipart_type());
    let (status, response_headers, body) = request(
        port,
        b"POST",
        "/v1/audio/transcriptions",
        &headers,
        &form(&[("file", Some("clip.wav"), AUDIO), ("model", None, b"raw")]),
    );
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(
        header(&response_headers, "pumice-postprocessing-status"),
        Some("off")
    );
    assert_eq!(json_body(&body)["text"], RAW);

    // The listed host with a foreign Origin, and an unlisted host, are
    // refused before any work, on the transcription route as elsewhere.
    for (host, origin, code) in [
        (
            "server-1.tail59f332.ts.net:17570",
            "https://evil.example",
            "origin_not_allowed",
        ),
        (
            "other.tail59f332.ts.net:17570",
            "https://other.tail59f332.ts.net:17570",
            "host_not_allowed",
        ),
    ] {
        for path in ["/", "/v1/models", "/v1/audio/transcriptions"] {
            let headers = [
                ("Host", host.to_owned()),
                ("Origin", origin.to_owned()),
                multipart_type(),
            ];
            let method: &[u8] = if path.ends_with("transcriptions") {
                b"POST"
            } else {
                b"GET"
            };
            let response = request(port, method, path, &headers, b"");
            assert_openai_error(&response, 403, code, None);
        }
        // The dictation route answers its own envelope, from the same guard.
        let headers = [("Host", host.to_owned()), ("Origin", origin.to_owned())];
        let (status, _, body) = request(port, b"POST", "/pumice/v1/dictations", &headers, b"");
        assert_eq!(status, 403);
        assert_eq!(json_body(&body)["error"]["code"], code);
    }
    drop(running.stop());
    rig.assert_children_gone();
}

#[test]
fn off_flows_and_silence_never_call_the_ai() {
    let rig = rig(&config(30_000, 10_000), json!([transcript(RAW)]));
    let running = rig.start();
    let (status, headers, body) = transcribe_model(running.port, "raw");
    assert_eq!(status, 200);
    assert_eq!(
        header(&headers, "pumice-postprocessing-status"),
        Some("off")
    );
    let body = json_body(&body);
    assert_eq!(body["text"], RAW, "the raw transcript, byte for byte");
    assert_eq!(
        body["pumice"]["post_processing"],
        json!({"status": "off", "preset": null, "relay_route": "passthrough"})
    );
    assert_eq!(rig.fake_calls(), 0);
    drop(running.stop());

    let rig = self::rig(&config(30_000, 10_000), json!([transcript("  \n\t ")]));
    let running = rig.start();
    let (status, headers, body) = transcribe_model(running.port, "clean");
    assert_eq!(status, 200);
    assert_eq!(
        header(&headers, "pumice-postprocessing-status"),
        Some("skipped")
    );
    let body = json_body(&body);
    assert_eq!(body["text"], "  \n\t ");
    assert_eq!(body["pumice"]["post_processing"]["status"], "skipped");
    assert_eq!(body["pumice"]["stages"]["relay"]["status"], "skipped");
    assert_eq!(rig.fake_calls(), 0, "no AI call for silence");
    drop(running.stop());
    rig.assert_children_gone();
}

#[test]
fn a_post_processing_failure_answers_the_raw_transcript() {
    let rig = rig(&config(30_000, 1_500), json!([transcript(RAW)]));
    let running = rig.start();
    for (fake, code) in [
        (
            json!({"stdout": "garbage S3NT1NEL-FAKE", "exit_code": 3}),
            "relay_failed",
        ),
        (
            json!({"stdout": claude_envelope(false, "   ")}),
            "relay_invalid_output",
        ),
        (
            json!({"stdout": claude_envelope(true, "You've hit your weekly limit · resets Oct 8")}),
            "relay_quota_exceeded",
        ),
        (
            json!({"stdout": claude_envelope(true, "Not logged in · Please run /login")}),
            "relay_not_logged_in",
        ),
        (
            json!({"stdout": claude_envelope(false, CLEANED), "sleep_ms": 30_000}),
            "relay_timeout",
        ),
    ] {
        let before = rig.fake_calls();
        rig.set_fake(fake);
        for format in ["json", "text"] {
            let (status, headers, body) = transcribe(
                running.port,
                &[
                    ("file", Some("a.wav"), AUDIO),
                    ("model", None, b"clean"),
                    ("response_format", None, format.as_bytes()),
                ],
            );
            assert_eq!(status, 200, "{code}");
            assert_eq!(
                header(&headers, "pumice-postprocessing-status"),
                Some("failed"),
                "{code}"
            );
            assert!(header(&headers, "retry-after").is_none());
            if format == "text" {
                assert_eq!(body, RAW.as_bytes(), "{code}");
                continue;
            }
            let body = json_body(&body);
            assert_eq!(body["text"], RAW, "{code}");
            let failure = &body["pumice"]["post_processing"];
            assert_eq!(failure["status"], "failed");
            assert_eq!(failure["error"]["stage"], "relay");
            assert_eq!(failure["error"]["code"], code, "{body}");
            assert!(failure["error"]["message"].is_string());
            assert_eq!(body["pumice"]["transcript"]["text"], RAW);
            assert!(!body.to_string().contains("S3NT1NEL-FAKE"), "{body}");
        }
        assert_eq!(
            rig.fake_calls(),
            before + 2,
            "{code}: one call each, no retry"
        );
        running.wait_idle();
    }
    // A provider failure never recycles the transcription child.
    let (_, status) = running.get("/pumice/v1/status");
    assert_eq!(status["transcription"]["generation"], 1);
    let stderr = running.stop();
    assert_events_metadata_only(&stderr);
    rig.assert_children_gone();
}

#[test]
fn transcription_failures_are_openai_errors() {
    let error = |status: u16, code: &str| {
        child(json!({"status": status, "body": {"error": {
            "message": "S3NT1NEL-STUB child text", "type": "x", "param": null, "code": code
        }}}))
    };
    for (generation, status, code, param) in [
        (
            error(400, "unsupported_format"),
            400,
            "transcription_invalid_input",
            None,
        ),
        (
            error(400, "invalid_language"),
            400,
            "invalid_language",
            Some("language"),
        ),
        (
            error(413, "file_too_large"),
            413,
            "transcription_invalid_input",
            None,
        ),
        (
            error(500, "transcription_failed"),
            500,
            "transcription_engine_failed",
            None,
        ),
        (
            child(json!({"raw_body": "S3NT1NEL-STUB not json"})),
            502,
            "transcription_protocol_error",
            None,
        ),
    ] {
        let rig = rig(&config(30_000, 10_000), json!([generation]));
        let running = rig.start();
        let response = transcribe(
            running.port,
            &[
                ("file", Some("a.wav"), AUDIO),
                ("model", None, b"clean"),
                ("language", None, b"xx"),
            ],
        );
        assert_openai_error(&response, status, code, param);
        assert_eq!(rig.fake_calls(), 0, "{code}: no AI call");
        let stderr = running.stop();
        assert_events_metadata_only(&stderr);
        rig.assert_children_gone();
    }
}

#[test]
fn a_transcription_timeout_is_a_504_and_recycles_the_child() {
    let rig = rig(
        "flows:
  - id: raw
    enabled: true
    transcription_model: turbo
    formatter: null
    relay_route: passthrough
    budgets_ms:
      total: 30000
      transcription: 1500
serve:
  enabled: true
  port: 0
  transcription_model: turbo
  transcription_flows: [raw]
",
        json!([child(json!({"hang": true})), {}]),
    );
    let running = rig.start();
    let started = Instant::now();
    let response = transcribe_model(running.port, "raw");
    assert_openai_error(&response, 504, "transcription_timeout", None);
    assert!(started.elapsed() < Duration::from_secs(10));
    running.wait_status("generation 2 ready", |status| {
        status["transcription"]["generation"] == 2
            && status["transcription"]["state"] == "ready"
            && status["dictations"]["active"] == false
    });
    let (status, _, body) = transcribe_model(running.port, "raw");
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let stderr = running.stop();
    assert!(stderr.contains("child_recycled"), "{stderr}");
    rig.assert_children_gone();
}

// --- Request validation -------------------------------------------------------

#[test]
fn invalid_requests_are_refused_before_any_work() {
    let rig = rig(&config(4_000, 1_000), json!([{}]));
    let running = rig.start();
    let port = running.port;
    let file: Part = ("file", Some("S3NT1NEL-NAME.wav"), AUDIO);
    let model: Part = ("model", None, b"clean");
    for (parts, status, code, param) in [
        (vec![model], 400, "missing_file", Some("file")),
        (vec![file], 400, "missing_model", Some("model")),
        (
            vec![file, ("model", None, b"S3NT1NEL-MODEL")],
            404,
            "model_not_found",
            Some("model"),
        ),
        (
            vec![file, ("model", None, b"extra")],
            404,
            "model_not_found",
            Some("model"),
        ),
        (
            vec![file, ("model", None, b"off")],
            404,
            "model_not_found",
            Some("model"),
        ),
        (
            vec![file, ("model", None, b"turbo")],
            404,
            "model_not_found",
            Some("model"),
        ),
        (
            vec![file, model, model],
            400,
            "duplicate_parameter",
            Some("model"),
        ),
        (
            vec![file, model, ("flow", None, b"clean")],
            400,
            "unsupported_parameter",
            None,
        ),
        (
            vec![file, model, ("S3NT1NEL-FIELD", None, b"x")],
            400,
            "unsupported_parameter",
            None,
        ),
        (
            vec![file, model, ("stream", None, b"true")],
            400,
            "stream_unsupported",
            Some("stream"),
        ),
        (
            vec![file, model, ("response_format", None, b"verbose_json")],
            400,
            "invalid_response_format",
            Some("response_format"),
        ),
        (
            vec![file, model, ("temperature", None, b"2")],
            400,
            "invalid_temperature",
            Some("temperature"),
        ),
        (
            vec![file, model, ("language", None, b"S3NT1NEL")],
            400,
            "invalid_language",
            Some("language"),
        ),
        (
            vec![file, ("model", Some("m.txt"), b"clean")],
            400,
            "text_param_as_file",
            Some("model"),
        ),
        (
            vec![("file", Some("a.wav"), b""), model],
            400,
            "empty_file",
            Some("file"),
        ),
        (
            vec![file, model, ("prompt", None, &[b'a'; 16 * 1024 + 1])],
            413,
            "field_too_large",
            Some("prompt"),
        ),
    ] {
        let response = transcribe(port, &parts);
        assert_openai_error(&response, status, code, param);
    }
    // Not multipart.
    let response = request(port, b"POST", "/v1/audio/transcriptions", &[], b"{}");
    assert_openai_error(&response, 400, "not_multipart", None);
    // POST only, with Allow; the guard first, before the method too.
    let response = request(port, b"GET", "/v1/audio/transcriptions", &[], b"");
    assert_openai_error(&response, 405, "method_not_allowed", None);
    assert_eq!(header(&response.1, "allow"), Some("POST"));
    for (method, name, value, code) in [
        (
            &b"POST"[..],
            "Origin",
            "http://localhost",
            "origin_not_allowed",
        ),
        (b"GET", "Origin", "http://localhost", "origin_not_allowed"),
        (
            b"POST",
            "Sec-Fetch-Site",
            "cross-site",
            "cross_site_not_allowed",
        ),
    ] {
        let response = request(
            port,
            method,
            "/v1/audio/transcriptions",
            &[multipart_type(), (name, value.to_owned())],
            &form(&[file, model]),
        );
        assert_openai_error(&response, 403, code, None);
    }
    // A declared length over the limit, refused without the body.
    stage("POST with an oversized Content-Length");
    let deadline = Instant::now() + WAIT;
    let mut stream = connect(port, deadline);
    bounded::send(
        &mut stream,
        &head(
            port,
            b"POST",
            "/v1/audio/transcriptions",
            &[multipart_type(), ("Content-Length", "26000001".to_owned())],
        ),
        deadline,
    )
    .unwrap();
    let response = read_response(&mut stream, deadline).expect("an early answer");
    assert_openai_error(&response, 413, "body_too_large", None);
    // A stalled upload: 408 at the upload bound (4 s total − 2.5 s).
    let mut stream = transcribe_partially(port);
    let response = read_response(&mut stream, Instant::now() + WAIT).expect("an answer");
    assert_openai_error(&response, 408, "upload_timeout", None);

    assert!(
        rig.log_events("transcription").is_empty(),
        "no work started"
    );
    assert_eq!(rig.fake_calls(), 0);
    let stderr = running.stop();
    assert_events_metadata_only(&stderr);
    rig.assert_children_gone();
}

/// The head and the start of a large body, nothing more.
fn transcribe_partially(port: u16) -> TcpStream {
    stage("partial POST /v1/audio/transcriptions");
    let deadline = Instant::now() + WAIT;
    let body = form(&[
        ("model", None, b"clean"),
        ("file", Some("a.wav"), &[0u8; 100_000]),
    ]);
    let mut stream = connect(port, deadline);
    let mut bytes = head(
        port,
        b"POST",
        "/v1/audio/transcriptions",
        &[multipart_type(), ("Content-Length", body.len().to_string())],
    );
    bytes.extend_from_slice(&body[..64]);
    bounded::send(&mut stream, &bytes, deadline).unwrap();
    stream
}

// --- Admission ------------------------------------------------------------------

fn dictate(port: u16, flow: &str) -> HttpResponse {
    request(
        port,
        b"POST",
        "/pumice/v1/dictations",
        &[multipart_type()],
        &form(&[
            ("file", Some("a.wav"), AUDIO),
            ("flow", None, flow.as_bytes()),
        ]),
    )
}

#[test]
fn dictations_and_transcriptions_share_one_permit() {
    let rig = rig(
        &config(30_000, 10_000),
        json!([child(json!({"delay_ms": 1_500}))]),
    );
    let running = rig.start();
    let port = running.port;

    // A dictation runs: a transcription is refused before its body is read.
    let first = std::thread::spawn(move || dictate(port, "raw"));
    rig.wait_log("transcription", 1);
    let mut stream = transcribe_partially(port);
    let response = read_response(&mut stream, Instant::now() + WAIT).expect("an early answer");
    assert_openai_error(&response, 429, "rate_limit_exceeded", None);
    assert_eq!(header(&response.1, "retry-after"), Some("1"));
    let (status, _, body) = first.join().unwrap();
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    running.wait_idle();

    // A transcription runs: a dictation gets its own envelope's 429.
    let second = std::thread::spawn(move || transcribe_model(port, "raw"));
    rig.wait_log("transcription", 2);
    let status = running.get("/pumice/v1/status").1;
    assert_eq!(status["dictations"]["admission"], "busy", "{status}");
    assert_eq!(status["dictations"]["stage"], "transcription");
    let (status, headers, body) = dictate(port, "raw");
    assert_eq!(status, 429);
    assert_eq!(header(&headers, "retry-after"), Some("1"));
    let body = json_body(&body);
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"]["code"], "rate_limit_exceeded");
    let (status, _, body) = second.join().unwrap();
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));

    assert_eq!(rig.log_events("transcription").len(), 2);
    drop(running.stop());
    rig.assert_children_gone();
}

#[test]
fn a_client_gone_during_transcription_recycles_the_child() {
    let rig = rig(
        &config(60_000, 10_000),
        json!([child(json!({"hang": true})), {}]),
    );
    let running = rig.start();
    stage("held POST /v1/audio/transcriptions");
    let deadline = Instant::now() + WAIT;
    let body = form(&[("file", Some("a.wav"), AUDIO), ("model", None, b"clean")]);
    let mut stream = connect(running.port, deadline);
    let mut bytes = head(
        running.port,
        b"POST",
        "/v1/audio/transcriptions",
        &[multipart_type(), ("Content-Length", body.len().to_string())],
    );
    bytes.extend_from_slice(&body);
    bounded::send(&mut stream, &bytes, deadline).unwrap();
    rig.wait_log("transcription", 1);
    let started = Instant::now();
    drop(stream);
    running.wait_status("the next generation ready", |status| {
        status["transcription"]["generation"] == 2
            && status["transcription"]["state"] == "ready"
            && status["dictations"]["active"] == false
    });
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(rig.fake_calls(), 0);
    let stderr = running.stop();
    assert_events_metadata_only(&stderr);
    assert!(stderr.contains("child_recycled"), "{stderr}");
    assert!(
        stderr
            .lines()
            .any(|line| line.contains("\"code\":\"cancelled\"")),
        "{stderr}"
    );
    rig.assert_children_gone();
}

#[cfg(unix)]
#[test]
fn a_shutdown_during_the_ai_call_answers_the_raw_transcript() {
    // Unix only: SIGTERM, and the provider's PID checked with kill(2).
    let rig = rig(&config(60_000, 50_000), json!([transcript(RAW)]));
    rig.set_fake(json!({"stdout": claude_envelope(false, CLEANED), "sleep_ms": 60_000}));
    let running = rig.start();
    stage("held POST /v1/audio/transcriptions");
    let deadline = Instant::now() + WAIT;
    let body = form(&[("file", Some("a.wav"), AUDIO), ("model", None, b"clean")]);
    let mut stream = connect(running.port, deadline);
    let mut bytes = head(
        running.port,
        b"POST",
        "/v1/audio/transcriptions",
        &[multipart_type(), ("Content-Length", body.len().to_string())],
    );
    bytes.extend_from_slice(&body);
    bounded::send(&mut stream, &bytes, deadline).unwrap();
    running.wait_status("the relay stage", |status| {
        status["dictations"]["stage"] == "relay"
    });
    stage("wait for the provider to start");
    let pid = loop {
        if let Some(report) = rig.fake_report() {
            break report["pid"].as_u64().unwrap() as u32;
        }
        assert!(Instant::now() < deadline, "the provider never started");
        std::thread::sleep(Duration::from_millis(10));
    };
    // SAFETY: plain kill(2) on the child this test owns.
    assert_eq!(
        unsafe { libc::kill(running.process.id() as libc::pid_t, libc::SIGTERM) },
        0
    );
    let (status, headers, body) =
        read_response(&mut stream, Instant::now() + WAIT).expect("an answer before the stop");
    assert_eq!(status, 200);
    assert_eq!(
        header(&headers, "pumice-postprocessing-status"),
        Some("failed")
    );
    let body = json_body(&body);
    assert_eq!(body["text"], RAW);
    let error = &body["pumice"]["post_processing"]["error"];
    assert_eq!(error["stage"], "relay", "{body}");
    assert_eq!(error["code"], "cancelled");
    assert_eq!(error["cause_code"], "service_shutdown");
    assert_eq!(body["pumice"]["stages"]["relay"]["status"], "cancelled");
    let stderr = running.stop();
    assert!(!pid_alive(pid), "the provider was reaped");
    assert_events_metadata_only(&stderr);
    rig.assert_children_gone();
}
