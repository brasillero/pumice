//! Tests for the HTTP API (S1.1) and the loopback binding (S5.3): the
//! recorded Handy request, error envelopes, body limits, SSE streaming and
//! the model list, all over raw HTTP/1.1 through an in-process server.
//! Requests never reach a real AI CLI — the Claude adapter points at the
//! portable fake. Two tests spawn the real `pumice` binary to check startup
//! behavior; they never invoke a provider.

mod support;

use std::fs;
use std::io::Read;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use pumice::api::{self, RequestLog};
use pumice::config;
use pumice::pipeline::Pipeline;
use pumice::process::ProcessRunner;
use pumice::providers;
use serde_json::{Value, json};
use support::FakeCli;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Minimal YAML configuring `claude` with its binary at the fake CLI and
/// `codex` disabled, so the enabled model list is exactly `claude`.
/// `{binary}` is replaced by the fake's path.
const CLAUDE_AT_FAKE: &str =
    "providers:\n  claude:\n    binary: '{binary}'\n  codex:\n    enabled: false\n";

/// The recorded Handy transcript, for raw-fallback assertions.
const FIXTURE_TRANSCRIPT: &str = "Reunião com a equipe às nove horas, não esquecer de enviar o relatório para o João e revisar o orçamento.";

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
/// alive: the fake CLI and the config directory.
struct TestServer {
    port: u16,
    log: Arc<LogCapture>,
    fake: FakeCli,
    _config_dir: TempDir,
}

impl TestServer {
    fn log_lines(&self) -> Vec<String> {
        self.log.lines()
    }
}

/// Builds a pipeline from `yaml` (`{binary}` replaced by the fake's path),
/// serves it on an ephemeral loopback port and returns the running server.
async fn start_server(yaml: &str, scenario: Value) -> TestServer {
    let fake = FakeCli::new(scenario);
    let yaml = yaml.replace("{binary}", &fake.path().display().to_string());
    let config_dir = TempDir::new().expect("temp dir");
    let config_path = config_dir.path().join("pumice.yaml");
    fs::write(&config_path, yaml).expect("write config");
    let config = config::load_with_env(Some(&config_path), |_| None)
        .expect("config loads")
        .config;
    let runner = Arc::new(ProcessRunner::new());
    let built = providers::build_from_config(&config, runner).expect("provider builds");
    let pipeline = Arc::new(Pipeline::new(&config, built));
    let enabled: Vec<String> = config
        .providers
        .iter()
        .filter(|(_, settings)| settings.enabled)
        .map(|(id, _)| id.clone())
        .collect();

    // Bind before spawning so a taken ephemeral port fails here, not in the
    // background task.
    let listener = tokio::net::TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind ephemeral loopback port");
    let port = listener.local_addr().unwrap().port();
    let log = Arc::new(LogCapture::default());
    let serve_log = Arc::clone(&log);
    tokio::spawn(async move {
        let _ = api::serve(listener, pipeline, enabled, serve_log).await;
    });
    TestServer {
        port,
        log,
        fake,
        _config_dir: config_dir,
    }
}

/// A parsed HTTP response (status, headers, body).
struct RawResponse {
    status: u16,
    headers: String,
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
/// Write errors are ignored: the server may stop reading an oversized body
/// (413) before the client finishes sending. The write side stays open
/// while the response is read — half-closing first makes hyper drop the
/// connection without answering.
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

/// Builds a chunked request (no `Content-Length`), one chunk per element.
fn chunked_request(path: &str, chunks: &[&[u8]]) -> Vec<u8> {
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nhost: 127.0.0.1\r\nconnection: close\r\ntransfer-encoding: chunked\r\n\r\n"
    )
    .into_bytes();
    for chunk in chunks {
        request.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
        request.extend_from_slice(chunk);
        request.extend_from_slice(b"\r\n");
    }
    request.extend_from_slice(b"0\r\n\r\n");
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
        headers: head.into_owned(),
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

/// A fake CLI replying successfully with `result`.
fn success_scenario(result: &str) -> Value {
    json!({"stdout": success_envelope(result), "exit_code": 0})
}

/// The recorded Handy request with `model` set to `claude`, since the
/// fixture's `pumice-echo` is intentionally not a configured provider.
fn handy_fixture_with_model(model: &str) -> Vec<u8> {
    let mut body: Value =
        serde_json::from_str(&support::fixture("handy-request.json")).expect("fixture parses");
    body["model"] = json!(model);
    serde_json::to_vec(&body).expect("fixture re-serializes")
}

#[tokio::test]
async fn handy_request_formats_and_returns_fake_output() {
    let result = "Reunião com a equipe às 9:00, não esquecer de enviar o relatório para o João e revisar o orçamento.";
    let server = start_server(CLAUDE_AT_FAKE, success_scenario(result)).await;

    let response = raw_http(
        server.port,
        http_request(
            "POST",
            "/v1/chat/completions",
            &[("content-type", "application/json")],
            &handy_fixture_with_model("claude"),
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body_text());
    assert!(
        response.headers.contains("application/json"),
        "headers: {}",
        response.headers
    );
    let body = response.body_json();
    assert_eq!(body["object"], "chat.completion");
    assert!(
        body["id"]
            .as_str()
            .expect("id is a string")
            .starts_with("chatcmpl-pumice-")
    );
    assert_eq!(body["model"], "claude");
    assert_eq!(body["choices"][0]["index"], 0);
    assert_eq!(body["choices"][0]["finish_reason"], "stop");
    assert_eq!(body["choices"][0]["message"]["role"], "assistant");
    assert_eq!(body["choices"][0]["message"]["content"], result);
    assert!(body.get("usage").is_none(), "usage must be omitted");
    assert!(
        server.fake.report_path().exists(),
        "the fake ran and formatted"
    );
    let lines = server.log_lines();
    assert_eq!(lines.len(), 1, "one metadata line: {lines:?}");
    assert!(lines[0].contains("kind=formatted"), "line: {}", lines[0]);
    assert!(lines[0].contains("provider=claude"), "line: {}", lines[0]);
}

#[tokio::test]
async fn handy_fixture_verbatim_keeps_unknown_model_dictation_raw() {
    // As recorded, the fixture's model is `pumice-echo`, which no
    // configuration knows: the dictation must come back byte for byte and
    // no CLI may run.
    let server = start_server(CLAUDE_AT_FAKE, success_scenario("unused")).await;
    let response = raw_http(
        server.port,
        http_request(
            "POST",
            "/v1/chat/completions",
            &[("content-type", "application/json")],
            support::fixture("handy-request.json").as_bytes(),
        ),
    )
    .await;

    assert_eq!(response.status, 200);
    let body = response.body_json();
    assert_eq!(body["choices"][0]["message"]["content"], FIXTURE_TRANSCRIPT);
    assert_eq!(body["model"], "pumice-echo");
    assert!(
        !server.fake.report_path().exists(),
        "no provider may run for an unknown model"
    );
    let lines = server.log_lines();
    assert!(lines[0].contains("kind=raw"), "line: {}", lines[0]);
    assert!(
        lines[0].contains("reason=UnknownProvider"),
        "line: {}",
        lines[0]
    );
}

#[tokio::test]
async fn failing_provider_returns_the_raw_transcript_with_200() {
    let server = start_server(
        CLAUDE_AT_FAKE,
        json!({
            "stdout": support::fixture("claude/not-logged-in.json"),
            "exit_code": 1,
        }),
    )
    .await;
    let raw = "  Olá, Mundo! Ação número três… fim.  ";
    let request = json!({
        "messages": [{
            "role": "user",
            "content": format!("<transcript>\n{raw}\n</transcript>"),
        }],
        "model": "claude",
        "stream": false,
    });
    let response = raw_http(
        server.port,
        http_request(
            "POST",
            "/v1/chat/completions",
            &[("content-type", "application/json")],
            &serde_json::to_vec(&request).unwrap(),
        ),
    )
    .await;

    assert_eq!(response.status, 200, "raw fallback is a normal completion");
    let body = response.body_json();
    assert_eq!(
        body["choices"][0]["message"]["content"], raw,
        "raw text is byte for byte"
    );
    let lines = server.log_lines();
    assert!(
        lines[0].contains("reason=ProviderFailed(NotLoggedIn)"),
        "line: {}",
        lines[0]
    );
}

#[tokio::test]
async fn stream_true_answers_with_sse_frames() {
    let result = "Reunião marcada.";
    let server = start_server(CLAUDE_AT_FAKE, success_scenario(result)).await;
    let mut body: Value =
        serde_json::from_str(&support::fixture("handy-request.json")).expect("fixture parses");
    body["model"] = json!("claude");
    body["stream"] = json!(true);
    let response = raw_http(
        server.port,
        http_request(
            "POST",
            "/v1/chat/completions",
            &[("content-type", "application/json")],
            &serde_json::to_vec(&body).unwrap(),
        ),
    )
    .await;

    assert_eq!(response.status, 200);
    assert!(
        response.headers.contains("text/event-stream"),
        "headers: {}",
        response.headers
    );
    let text = response.body_text();
    let frames: Vec<&str> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .collect();
    assert_eq!(frames.len(), 3, "two chunks then [DONE]: {text}");
    assert_eq!(frames[2], "[DONE]");

    let first: Value = serde_json::from_str(frames[0]).expect("chunk is JSON");
    assert_eq!(first["object"], "chat.completion.chunk");
    assert_eq!(first["choices"][0]["delta"]["role"], "assistant");
    assert_eq!(first["choices"][0]["delta"]["content"], result);
    assert_eq!(first["choices"][0]["finish_reason"], Value::Null);

    let last: Value = serde_json::from_str(frames[1]).expect("chunk is JSON");
    assert_eq!(last["choices"][0]["delta"], json!({}));
    assert_eq!(last["choices"][0]["finish_reason"], "stop");
    assert!(text.ends_with("data: [DONE]\n\n"), "SSE terminator: {text}");
}

#[tokio::test]
async fn malformed_json_is_400_without_running_the_fake() {
    let server = start_server(CLAUDE_AT_FAKE, success_scenario("unused")).await;
    let response = raw_http(
        server.port,
        http_request(
            "POST",
            "/v1/chat/completions",
            &[("content-type", "application/json")],
            b"{\"messages\": [",
        ),
    )
    .await;

    assert_eq!(response.status, 400);
    let body = response.body_json();
    assert_eq!(body["error"]["type"], "invalid_request_error");
    assert!(
        body["error"]["message"]
            .as_str()
            .expect("message is a string")
            .contains("not valid JSON")
    );
    assert!(!server.fake.report_path().exists(), "the fake must not run");
}

#[tokio::test]
async fn conversation_history_is_400_without_running_the_fake() {
    let server = start_server(CLAUDE_AT_FAKE, success_scenario("unused")).await;
    let request = json!({
        "messages": [
            {"role": "user", "content": "first"},
            {"role": "user", "content": "second"},
        ],
        "stream": false,
    });
    let response = raw_http(
        server.port,
        http_request(
            "POST",
            "/v1/chat/completions",
            &[("content-type", "application/json")],
            &serde_json::to_vec(&request).unwrap(),
        ),
    )
    .await;

    assert_eq!(response.status, 400);
    let body = response.body_json();
    assert_eq!(body["error"]["type"], "invalid_request_error");
    assert!(
        body["error"]["message"]
            .as_str()
            .expect("message is a string")
            .contains("more than one user message")
    );
    assert!(!server.fake.report_path().exists(), "the fake must not run");
}

#[tokio::test]
async fn oversized_body_is_413() {
    let server = start_server(CLAUDE_AT_FAKE, success_scenario("unused")).await;
    // A little over the 10 MiB cap, inside one JSON string. The margin keeps
    // the server's post-cap read within one chunk, so the client write
    // completes instead of racing a connection reset.
    let mut text = String::with_capacity(10 * 1024 * 1024 + 16 * 1024);
    text.push_str("{\"messages\":[{\"role\":\"user\",\"content\":\"");
    text.push('x');
    text.push_str(&"x".repeat(10 * 1024 * 1024 + 8 * 1024));
    text.push_str("\"}]}");
    let response = raw_http(
        server.port,
        http_request(
            "POST",
            "/v1/chat/completions",
            &[("content-type", "application/json")],
            text.as_bytes(),
        ),
    )
    .await;

    assert_eq!(response.status, 413, "body: {}", response.body_text());
    assert_eq!(
        response.body_json()["error"]["type"],
        "invalid_request_error"
    );
    assert!(!server.fake.report_path().exists(), "the fake must not run");
}

#[tokio::test]
async fn chunked_body_is_read_fully() {
    let result = "Ditado recebido.";
    let server = start_server(CLAUDE_AT_FAKE, success_scenario(result)).await;
    let body = handy_fixture_with_model("claude");
    let (left, right) = body.split_at(body.len() / 2);
    let response = raw_http(
        server.port,
        chunked_request("/v1/chat/completions", &[left, right]),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body_text());
    let parsed = response.body_json();
    assert_eq!(parsed["choices"][0]["message"]["content"], result);
    assert!(server.fake.report_path().exists(), "the fake ran");
}

#[tokio::test]
async fn unknown_path_is_an_openai_404() {
    let server = start_server(CLAUDE_AT_FAKE, success_scenario("unused")).await;
    let response = raw_http(server.port, http_request("GET", "/v1/nope", &[], b"")).await;

    assert_eq!(response.status, 404);
    let body = response.body_json();
    assert_eq!(body["error"]["type"], "invalid_request_error");
    assert!(body["error"]["message"].as_str().is_some());
}

#[tokio::test]
async fn models_lists_enabled_providers() {
    let server = start_server(CLAUDE_AT_FAKE, success_scenario("unused")).await;
    let response = raw_http(server.port, http_request("GET", "/v1/models", &[], b"")).await;

    assert_eq!(response.status, 200);
    let body = response.body_json();
    assert_eq!(body["object"], "list");
    let ids: Vec<&str> = body["data"]
        .as_array()
        .expect("data is an array")
        .iter()
        .map(|entry| entry["id"].as_str().expect("id is a string"))
        .collect();
    assert_eq!(ids, ["claude"], "codex is disabled in the test config");
    for entry in body["data"].as_array().unwrap() {
        assert_eq!(entry["object"], "model");
        assert_eq!(entry["created"], 0);
        assert_eq!(entry["owned_by"], "pumice");
    }
    assert!(
        !server.fake.report_path().exists(),
        "listing never invokes a CLI"
    );
}

#[tokio::test]
async fn authorization_header_is_ignored_and_never_logged() {
    let result = "Autorizado.";
    let server = start_server(CLAUDE_AT_FAKE, success_scenario(result)).await;
    let response = raw_http(
        server.port,
        http_request(
            "POST",
            "/v1/chat/completions",
            &[
                ("content-type", "application/json"),
                ("authorization", "Bearer secret"),
            ],
            &handy_fixture_with_model("claude"),
        ),
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(
        response.body_json()["choices"][0]["message"]["content"],
        result
    );
    assert!(server.fake.report_path().exists(), "the fake ran");
    let lines = server.log_lines();
    assert!(!lines.is_empty(), "a metadata line was written");
    for line in &lines {
        assert!(
            !line.contains("secret"),
            "authorization value leaked into log: {line}"
        );
        assert!(
            !line.contains(FIXTURE_TRANSCRIPT),
            "dictated text leaked into log: {line}"
        );
    }
}

/// Writes `yaml` to a fresh config file and returns its directory (kept
/// alive by the caller) and path.
fn write_config(yaml: &str) -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("pumice.yaml");
    fs::write(&path, yaml).expect("write config");
    (dir, path)
}

#[test]
fn occupied_port_exits_1_naming_port() {
    let blocker = std::net::TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .expect("bind a port to occupy");
    let port = blocker.local_addr().unwrap().port();
    let (_dir, config_path) = write_config(&format!("port: {port}\n"));

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
        stderr.contains(&format!("port {port}")),
        "message must name the port: {stderr}"
    );
    assert!(
        stderr.contains("already in use"),
        "message must explain the conflict: {stderr}"
    );
    assert!(
        stderr.contains("Change \"port\" in"),
        "message must point at the setting: {stderr}"
    );
}

/// A port that was free at observation time; binding it again can in theory
/// race, so the caller retries.
fn grab_free_port() -> u16 {
    let listener = std::net::TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .expect("bind ephemeral port");
    listener.local_addr().unwrap().port()
}

/// Spawns `pumice serve` and returns the child plus a receiver holding the
/// first stdout line, read on a helper thread.
fn spawn_serve(config_path: &std::path::Path) -> (Child, mpsc::Receiver<Option<String>>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pumice"))
        .arg("serve")
        .arg("--config")
        .arg(config_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pumice serve");
    let mut stdout = child.stdout.take().expect("piped stdout");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut line = String::new();
        let mut byte = [0u8; 1];
        loop {
            match stdout.read(&mut byte) {
                Ok(0) => break,
                Ok(_) => {
                    line.push(byte[0] as char);
                    if byte[0] == b'\n' {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(if line.is_empty() { None } else { Some(line) });
    });
    (child, rx)
}

#[test]
fn serve_prints_the_loopback_address() {
    for attempt in 1..=3 {
        let port = grab_free_port();
        let (_dir, config_path) = write_config(&format!("port: {port}\n"));
        let (mut child, line_rx) = spawn_serve(&config_path);

        let line = line_rx
            .recv_timeout(Duration::from_secs(15))
            .unwrap_or_else(|_| {
                let _ = child.kill();
                panic!("pumice did not print its startup line (attempt {attempt})")
            });

        let Some(line) = line else {
            // The child exited before printing: only a lost port race is
            // tolerable, and only a few times.
            let status = child.wait().expect("child exits");
            let stderr = child
                .stderr
                .take()
                .map(|mut err| {
                    let mut text = String::new();
                    let _ = err.read_to_string(&mut text);
                    text
                })
                .unwrap_or_default();
            assert!(
                status.code() == Some(1) && stderr.contains("already in use"),
                "pumice exited unexpectedly (attempt {attempt}): {stderr}"
            );
            continue;
        };

        assert!(
            line.contains(&format!("http://127.0.0.1:{port}/v1")),
            "startup line must name the IPv4 loopback address: {line}"
        );
        assert!(
            !line.contains("0.0.0.0"),
            "never any other interface: {line}"
        );

        child.kill().expect("stop pumice");
        child.wait().expect("pumice reaped");
        return;
    }
    panic!("could not start pumice on a free port after 3 attempts");
}
