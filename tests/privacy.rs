//! Privacy and debug-log tests (S1.4): the JSONL sink is off by default,
//! records full payloads only when `debug_log.enabled` is true, redacts
//! credential-bearing headers, and never leaks dictated text into the
//! ordinary metadata log. Two tests spawn the real `pumice` binary to check
//! startup behavior; providers are always the fake CLI.

mod support;

use std::fs;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use pumice::api::{self, RequestLog};
use pumice::config;
use pumice::logging::DebugLog;
use pumice::pipeline::Pipeline;
use pumice::process::ProcessRunner;
use pumice::providers;
use serde_json::{Value, json};
use support::FakeCli;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Minimal YAML configuring `claude` with its binary at the fake CLI and
/// `codex` disabled. `{binary}` is replaced by the fake's path.
const CLAUDE_AT_FAKE: &str =
    "providers:\n  claude:\n    binary: '{binary}'\n  codex:\n    enabled: false\n";

/// Captured metadata lines, one per completion request.
#[derive(Default)]
struct LogCapture {
    lines: Mutex<Vec<String>>,
}

impl RequestLog for LogCapture {
    fn write_line(&self, line: &str) {
        self.lines.lock().unwrap().push(line.to_owned());
    }
}

impl LogCapture {
    fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }
}

/// A running in-process server with everything its pipeline needs kept
/// alive: the fake CLI, the config directory and the resolved config.
struct TestServer {
    port: u16,
    log: Arc<LogCapture>,
    fake: FakeCli,
    config: config::Config,
    _config_dir: TempDir,
}

/// Builds a pipeline from `yaml` (`{binary}` replaced by the fake's path),
/// serves it on an ephemeral loopback port and returns the running server.
/// The debug log sink is opened exactly like `pumice serve` does.
async fn start_server(yaml: &str, scenario: Value) -> TestServer {
    let fake = FakeCli::new(scenario);
    let yaml = yaml.replace("{binary}", &fake.path().display().to_string());
    let config_dir = TempDir::new().expect("temp dir");
    let config_path = config_dir.path().join("pumice.yaml");
    fs::write(&config_path, yaml).expect("write config");
    let config = config::load_with_env(Some(&config_path), |_| None)
        .expect("config loads")
        .config;
    let debug_log = DebugLog::open(&config.debug_log).expect("debug log opens");
    let runner = Arc::new(ProcessRunner::new());
    let built = providers::build_from_config(&config, runner).expect("provider builds");
    let pipeline = Arc::new(Pipeline::new(&config, built));

    let listener = tokio::net::TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind ephemeral loopback port");
    let port = listener.local_addr().unwrap().port();
    let log = Arc::new(LogCapture::default());
    let serve_log = Arc::clone(&log);
    tokio::spawn(async move {
        let _ = api::serve(listener, pipeline, serve_log, Arc::new(debug_log)).await;
    });
    TestServer {
        port,
        log,
        fake,
        config,
        _config_dir: config_dir,
    }
}

/// A parsed HTTP response (status, headers, body).
struct RawResponse {
    status: u16,
    body: Vec<u8>,
}

impl RawResponse {
    fn body_json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("response body is valid JSON")
    }

    fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Sends `request` verbatim over raw HTTP/1.1 and reads the whole response.
async fn raw_http(port: u16, request: Vec<u8>) -> RawResponse {
    let mut stream = TcpStream::connect(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
        .await
        .expect("connect to server");
    let _ = stream.write_all(&request).await;
    let read = async {
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .await
            .expect("read response");
        response
    };
    let response = tokio::time::timeout(Duration::from_secs(30), read)
        .await
        .expect("request timed out");
    parse_http(&response)
}

/// Builds a request with fixed framing: HTTP/1.1, loopback host,
/// `connection: close`, explicit `Content-Length`.
fn http_request(method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut request =
        format!("{method} {path} HTTP/1.1\r\nhost: 127.0.0.1\r\nconnection: close\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str(&format!("content-length: {}\r\n\r\n", body.len()));
    let mut request = request.into_bytes();
    request.extend_from_slice(body);
    request
}

fn parse_http(bytes: &[u8]) -> RawResponse {
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("header/body separator present");
    let head = String::from_utf8_lossy(&bytes[..split]);
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .expect("status line has a code")
        .parse()
        .expect("status code is numeric");
    RawResponse {
        status,
        body: bytes[split + 4..].to_vec(),
    }
}

/// The stdout of a fake CLI replying with a Claude success envelope holding
/// `result`.
fn success_envelope(result: &str) -> String {
    json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "result": result,
    })
    .to_string()
}

/// Posts one dictation with the given extra headers, returning the response.
async fn post_dictation(port: u16, dictation: &str, headers: &[(&str, &str)]) -> RawResponse {
    let request = json!({
        "messages": [{"role": "user", "content": dictation}],
        "model": "claude",
        "stream": false,
    });
    raw_http(
        port,
        http_request(
            "POST",
            "/v1/chat/completions",
            headers,
            &serde_json::to_vec(&request).unwrap(),
        ),
    )
    .await
}

#[tokio::test]
async fn debug_log_disabled_creates_no_file_at_the_default_path() {
    let server = start_server(
        CLAUDE_AT_FAKE,
        json!({"stdout": success_envelope("Formatado."), "exit_code": 0}),
    )
    .await;
    let response = post_dictation(
        server.port,
        "Notas de reunião.",
        &[("content-type", "application/json")],
    )
    .await;
    assert_eq!(response.status, 200, "body: {}", response.body_text());
    assert!(server.fake.report_path().exists(), "the fake ran");

    let default_path = server._config_dir.path().join("pumice-debug.jsonl");
    assert!(
        !default_path.exists(),
        "a disabled debug log must not create {}",
        default_path.display()
    );
    assert_eq!(server.log.lines().len(), 1, "only the metadata line");
}

#[tokio::test]
async fn inspect_debug_log_disabled_creates_no_file_and_no_text_in_metadata() {
    let server = start_server(
        CLAUDE_AT_FAKE,
        json!({"stdout": success_envelope("unused"), "exit_code": 0}),
    )
    .await;
    let marker = "pumice-inspect-marker-7d4a";
    let body = format!(r#"{{"model":"inspect","custom":123,"marker":"{marker}","messages":[]}}"#);
    let response = raw_http(
        server.port,
        http_request(
            "POST",
            "/v1/chat/completions",
            &[("content-type", "application/json")],
            body.as_bytes(),
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body_text());
    let response_body = response.body_json();
    assert_eq!(response_body["model"], "inspect");
    let echoed = response_body["choices"][0]["message"]["content"]
        .as_str()
        .expect("content is a string");
    assert_eq!(echoed, body, "inspect echoes the exact request body");

    let default_path = server._config_dir.path().join("pumice-debug.jsonl");
    assert!(
        !default_path.exists(),
        "a disabled debug log must not create {}",
        default_path.display()
    );
    let lines = server.log.lines();
    assert_eq!(lines.len(), 1, "one log entry: {lines:?}");
    assert!(lines[0].contains(" inspect "), "line: {}", lines[0]);
    assert!(
        !lines[0].contains(marker),
        "metadata line leaked the marker: {}",
        lines[0]
    );
}

#[tokio::test]
async fn inspect_debug_log_enabled_records_exact_body_and_redacts_headers() {
    let yaml =
        CLAUDE_AT_FAKE.to_owned() + "debug_log:\n  enabled: true\n  path: inspect-debug.jsonl\n";
    let server = start_server(
        &yaml,
        json!({"stdout": success_envelope("unused"), "exit_code": 0}),
    )
    .await;
    let marker = "pumice-inspect-marker-7d4a";
    let body = format!(
        r#"{{"model":"inspect","arbitrary_option":true,"messages":[{{"role":"assistant","content":{}}}]}}"#,
        serde_json::to_string(marker).unwrap()
    );
    let response = raw_http(
        server.port,
        http_request(
            "POST",
            "/v1/chat/completions",
            &[
                ("content-type", "application/json"),
                ("authorization", "Bearer secret-token"),
                ("cookie", "session=abc"),
                ("user-agent", "pumice-inspect-test"),
            ],
            body.as_bytes(),
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body_text());

    let log_path = server.config.debug_log.path.clone();
    let contents = fs::read_to_string(&log_path)
        .unwrap_or_else(|e| panic!("debug log {} not readable: {e}", log_path.display()));
    assert!(
        !contents.contains("secret-token"),
        "authorization leaked into the debug log: {contents}"
    );
    assert!(
        !contents.contains("session=abc"),
        "cookie leaked into the debug log: {contents}"
    );
    let lines: Vec<&str> = contents.lines().collect();
    assert_eq!(lines.len(), 1, "exactly one JSONL line: {contents}");
    let record: Value = serde_json::from_str(lines[0]).expect("the line is JSON");

    assert_eq!(record["request_id"], response.body_json()["id"]);
    assert_eq!(record["request"]["model"], "inspect");
    assert_eq!(record["request"]["arbitrary_option"], true);
    assert_eq!(
        record["request"]["messages"][0]["content"], marker,
        "the marker is in the recorded request"
    );
    assert_eq!(
        record["request"]["headers"]["user-agent"],
        "pumice-inspect-test"
    );
    assert_eq!(
        record["request"]["headers"]["content-type"],
        "application/json"
    );
    assert_eq!(
        record["request"]["headers"]["authorization"],
        Value::Null,
        "credential-bearing headers are never recorded"
    );
    assert_eq!(record["raw_text"], "");
    assert_eq!(
        record["response_text"], body,
        "response_text is the exact echoed body"
    );
    assert_eq!(record["outcome"]["kind"], "inspect");
    assert_eq!(record["outcome"]["provider"], Value::Null);
    assert_eq!(record["outcome"]["reason"], Value::Null);
    assert!(record["outcome"]["elapsed_ms"].is_number());

    let lines = server.log.lines();
    assert_eq!(lines.len(), 1, "one log entry: {lines:?}");
    assert!(lines[0].contains(" inspect "), "line: {}", lines[0]);
    assert!(
        !lines[0].contains(marker),
        "metadata line leaked the marker: {}",
        lines[0]
    );
}

#[tokio::test]
async fn debug_log_records_one_redacted_line_when_enabled() {
    let yaml =
        CLAUDE_AT_FAKE.to_owned() + "debug_log:\n  enabled: true\n  path: nested/debug.jsonl\n";
    let formatted = "Reunião de alinhamento amanhã às dez.";
    let server = start_server(
        &yaml,
        json!({"stdout": success_envelope(formatted), "exit_code": 0}),
    )
    .await;
    let log_path = server.config.debug_log.path.clone();
    assert!(
        log_path.ends_with(Path::new("nested").join("debug.jsonl")),
        "relative path resolves against the config file: {}",
        log_path.display()
    );

    let marker = "pumice-marker-9f31";
    let dictation = format!("Reunião de alinhamento {marker} amanhã às dez.");
    let response = post_dictation(
        server.port,
        &dictation,
        &[
            ("content-type", "application/json"),
            ("authorization", "Bearer secret-token"),
            ("user-agent", "pumice-privacy-test"),
        ],
    )
    .await;
    assert_eq!(response.status, 200, "body: {}", response.body_text());
    assert_eq!(
        response.body_json()["choices"][0]["message"]["content"],
        formatted
    );

    let contents = fs::read_to_string(&log_path)
        .unwrap_or_else(|e| panic!("debug log {} not readable: {e}", log_path.display()));
    assert!(
        !contents.contains("secret-token"),
        "authorization leaked into the debug log: {contents}"
    );
    let lines: Vec<&str> = contents.lines().collect();
    assert_eq!(lines.len(), 1, "exactly one JSONL line: {contents}");
    let record: Value = serde_json::from_str(lines[0]).expect("the line is JSON");

    assert!(
        record["time"]
            .as_str()
            .expect("time is a string")
            .ends_with('Z'),
        "time is RFC 3339: {}",
        record["time"]
    );
    assert_eq!(
        record["request_id"],
        response.body_json()["id"],
        "the record names the completion it belongs to"
    );
    assert!(
        record["request"]["messages"][0]["content"]
            .as_str()
            .expect("content is a string")
            .contains(marker),
        "the marker is in the recorded request: {}",
        record["request"]
    );
    assert_eq!(
        record["request"]["headers"]["user-agent"],
        "pumice-privacy-test"
    );
    assert_eq!(
        record["request"]["headers"]["content-type"],
        "application/json"
    );
    assert_eq!(
        record["request"]["headers"]["authorization"],
        Value::Null,
        "credential-bearing headers are never recorded"
    );
    assert_eq!(record["raw_text"], dictation, "the marker is in raw_text");
    assert_eq!(record["outcome"]["kind"], "formatted");
    assert_eq!(record["outcome"]["reason"], Value::Null);
    assert_eq!(record["outcome"]["provider"], "claude");
    assert_eq!(record["outcome"]["attempts"], 1);
    assert!(
        record["outcome"]["elapsed_ms"].is_number(),
        "elapsed_ms present: {}",
        record["outcome"]
    );
    assert_eq!(record["response_text"], formatted);

    // Normal operation is unchanged: the metadata line still never carries
    // dictated text.
    let lines = server.log.lines();
    assert_eq!(lines.len(), 1, "one log entry: {lines:?}");
    assert!(
        !lines[0].contains(marker),
        "the metadata line leaked the dictation: {}",
        lines[0]
    );
    assert!(
        !lines[0].contains(dictation.trim()),
        "the metadata line leaked the dictation: {}",
        lines[0]
    );
}

/// Writes `yaml` to a fresh config file and returns its directory (kept
/// alive by the caller) and path.
fn write_config(yaml: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("pumice.yaml");
    fs::write(&path, yaml).expect("write config");
    (dir, path)
}

/// YAML with every registered provider's binary at its own disposable fake,
/// so spawned-`pumice` tests never look a real CLI up on PATH during startup
/// detection. `debug_log` is appended verbatim. Returns the fakes — they
/// must outlive the served process — and the YAML.
fn hermetic_serve_yaml(port: u16, debug_log: &str) -> (Vec<FakeCli>, String) {
    let mut fakes: Vec<FakeCli> = Vec::new();
    let mut yaml = format!("port: {port}\nproviders:\n");
    for id in ["claude", "codex", "opencode", "antigravity"] {
        let fake = FakeCli::new(json!({}));
        yaml.push_str(&format!(
            "  {id}:\n    binary: '{}'\n",
            fake.path().display()
        ));
        fakes.push(fake);
    }
    yaml.push_str(debug_log);
    (fakes, yaml)
}

#[test]
fn serve_exits_1_when_the_debug_log_path_is_unopenable() {
    let (dir, config_path) = write_config("");
    // A file where a directory is expected: creating the parent fails on
    // every OS, so the debug log cannot be opened.
    let blocker = dir.path().join("blocker");
    fs::write(&blocker, "not a directory").expect("write blocker file");
    let debug_path = blocker.join("nested.jsonl");
    // The port never binds: the debug log open fails first. Any valid port
    // keeps the config loadable.
    let (_fakes, mut yaml) = hermetic_serve_yaml(grab_free_port(), "");
    yaml.push_str(&format!(
        "debug_log:\n  enabled: true\n  path: '{}'\n",
        debug_path.display()
    ));
    fs::write(&config_path, yaml).expect("write config");

    let output = Command::new(env!("CARGO_BIN_EXE_pumice"))
        .arg("serve")
        .arg("--config")
        .arg(&config_path)
        .output()
        .expect("run pumice serve");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(1),
        "stderr: {stderr}\nstdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        stderr.contains("debug_log.path"),
        "message must name the setting: {stderr}"
    );
    assert!(
        stderr.contains("nested.jsonl"),
        "message must name the path: {stderr}"
    );
}

/// A port that was free at observation time; binding it again can in theory
/// race, so the caller retries.
fn grab_free_port() -> u16 {
    let listener = std::net::TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .expect("bind ephemeral port");
    listener.local_addr().unwrap().port()
}

/// Spawns `pumice serve` and returns the child plus a receiver forwarding
/// every stderr line (`None` marks end of output), read on a helper thread.
fn spawn_serve_stderr(config_path: &Path) -> (Child, mpsc::Receiver<Option<String>>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pumice"))
        .arg("serve")
        .arg("--config")
        .arg(config_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pumice serve");
    let stderr = child.stderr.take().expect("piped stderr");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = std::io::BufReader::new(stderr);
        let mut line = String::new();
        loop {
            line.clear();
            match std::io::BufRead::read_line(&mut reader, &mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if tx
                        .send(Some(line.trim_end_matches(['\r', '\n']).to_owned()))
                        .is_err()
                    {
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(None);
    });
    (child, rx)
}

#[test]
fn serve_warns_when_the_debug_log_is_enabled() {
    for attempt in 1..=3 {
        let port = grab_free_port();
        let (_fakes, yaml) = hermetic_serve_yaml(port, "debug_log:\n  enabled: true\n");
        let (_dir, config_path) = write_config(&yaml);
        let (mut child, lines_rx) = spawn_serve_stderr(&config_path);

        // Startup detection prints provider status lines first; scan until
        // the debug-log warning appears.
        let mut seen: Vec<String> = Vec::new();
        let mut warning: Option<String> = None;
        let mut exited_early = false;
        loop {
            match lines_rx.recv_timeout(Duration::from_secs(15)) {
                Ok(Some(line)) => {
                    if line.contains("warning: debug log enabled: dictated text is written to ") {
                        warning = Some(line);
                        break;
                    }
                    seen.push(line);
                }
                // EOF (child gone) or timeout: no warning is coming.
                Ok(None) | Err(_) => {
                    exited_early = true;
                    break;
                }
            }
        }

        let Some(warning) = warning else {
            // The child exited before the warning: only a lost port race is
            // tolerable, and only a few times.
            assert!(
                exited_early,
                "the warning never printed; stderr so far: {seen:?}"
            );
            let status = child.wait().expect("child exits");
            assert!(
                status.code() == Some(1),
                "pumice exited unexpectedly (attempt {attempt}); stderr: {seen:?}"
            );
            continue;
        };

        assert!(
            warning.contains("pumice-debug.jsonl"),
            "the warning must name the path: {warning}"
        );

        child.kill().expect("stop pumice");
        child.wait().expect("pumice reaped");
        return;
    }
    panic!("could not start pumice on a free port after 3 attempts");
}
