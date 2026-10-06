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
use pumice::logging::DebugLog;
use pumice::pipeline::Pipeline;
use pumice::process::ProcessRunner;
use pumice::providers::{self, discovery};
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

/// A running in-process server with a separate fake CLI per provider, so a
/// test can prove which provider ran.
struct DualServer {
    port: u16,
    log: Arc<LogCapture>,
    claude: FakeCli,
    codex: FakeCli,
    _config_dir: TempDir,
}

impl DualServer {
    fn log_lines(&self) -> Vec<String> {
        self.log.lines()
    }
}

/// YAML configuring `claude` and `codex`, each at its own fake CLI.
/// `{claude}` and `{codex}` are replaced by the fakes' paths.
const BOTH_AT_FAKES: &str =
    "providers:\n  claude:\n    binary: '{claude}'\n  codex:\n    binary: '{codex}'\n";

/// Builds a pipeline from `yaml` with one fake per provider (`{claude}` and
/// `{codex}` replaced by the fakes' paths), serves it on an ephemeral
/// loopback port and returns the running server.
async fn start_dual_server(
    yaml: &str,
    claude_scenario: Value,
    codex_scenario: Value,
) -> DualServer {
    let claude = FakeCli::new(claude_scenario);
    let codex = FakeCli::new(codex_scenario);
    let yaml = yaml
        .replace("{claude}", &claude.path().display().to_string())
        .replace("{codex}", &codex.path().display().to_string());
    let config_dir = TempDir::new().expect("temp dir");
    let config_path = config_dir.path().join("pumice.yaml");
    fs::write(&config_path, yaml).expect("write config");
    let config = config::load_with_env(Some(&config_path), |_| None)
        .expect("config loads")
        .config;
    let runner = Arc::new(ProcessRunner::new());
    let built =
        providers::build_from_config(&config, Arc::clone(&runner)).expect("providers build");
    let pipeline = Arc::new(Pipeline::new(&config, built));

    let listener = tokio::net::TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind ephemeral loopback port");
    let port = listener.local_addr().unwrap().port();
    let log = Arc::new(LogCapture::default());
    let serve_log = Arc::clone(&log);
    tokio::spawn(async move {
        let _ = api::serve(
            listener,
            pipeline,
            serve_log,
            Arc::new(DebugLog::disabled()),
        )
        .await;
    });
    DualServer {
        port,
        log,
        claude,
        codex,
        _config_dir: config_dir,
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

    // Bind before spawning so a taken ephemeral port fails here, not in the
    // background task.
    let listener = tokio::net::TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind ephemeral loopback port");
    let port = listener.local_addr().unwrap().port();
    let log = Arc::new(LogCapture::default());
    let serve_log = Arc::clone(&log);
    tokio::spawn(async move {
        let _ = api::serve(
            listener,
            pipeline,
            serve_log,
            Arc::new(DebugLog::disabled()),
        )
        .await;
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

/// The stdout of a fake CLI replying with a Codex JSONL success stream whose
/// agent message is `result`, mirroring `tests/fixtures/codex/success.jsonl`.
fn codex_success_stdout(result: &str) -> String {
    let text = serde_json::to_string(result).expect("result serializes");
    format!(
        "{{\"type\":\"thread.started\",\"thread_id\":\"00000000-0000-4000-8000-000000000003\"}}\n\
         {{\"type\":\"turn.started\"}}\n\
         {{\"type\":\"item.completed\",\"item\":{{\"id\":\"item_0\",\"type\":\"agent_message\",\"text\":{text}}}}}\n\
         {{\"type\":\"turn.completed\",\"usage\":{{\"input_tokens\":5310,\"cached_input_tokens\":0,\"output_tokens\":6}}}}\n"
    )
}

/// A fake Codex CLI replying successfully with `result`.
fn codex_success_scenario(result: &str) -> Value {
    json!({"stdout": codex_success_stdout(result), "exit_code": 0})
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
    assert_eq!(lines.len(), 1, "one log entry: {lines:?}");
    assert!(lines[0].contains(" formatted "), "line: {}", lines[0]);
    assert!(lines[0].contains(" claude ("), "line: {}", lines[0]);
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
    assert!(lines[0].contains(" RAW "), "line: {}", lines[0]);
    assert!(lines[0].contains("no provider named"), "line: {}", lines[0]);
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
        lines[0].contains("original text returned: not logged in"),
        "line: {}",
        lines[0]
    );
    assert!(
        lines[0].contains("✗ claude") && lines[0].contains("(exit "),
        "the attempt line carries the CLI's exit code: {}",
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
async fn health_reports_ok_and_the_version() {
    let server = start_server(CLAUDE_AT_FAKE, success_scenario("unused")).await;
    let response = raw_http(server.port, http_request("GET", "/health", &[], b"")).await;

    assert_eq!(response.status, 200, "body: {}", response.body_text());
    let body = response.body_json();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    assert!(
        !server.fake.report_path().exists(),
        "health never invokes a CLI"
    );
}

#[tokio::test]
async fn health_answers_quickly_while_a_dictation_runs() {
    let result = "Reunião com a equipe às 9:00.";
    // The fake sleeps two seconds, so the dictation below is still running
    // (holding the pipeline's single permit) when /health is checked.
    let server = start_server(
        CLAUDE_AT_FAKE,
        json!({"stdout": success_envelope(result), "exit_code": 0, "sleep_ms": 2000}),
    )
    .await;

    let port = server.port;
    let dictation = tokio::spawn(async move {
        raw_http(
            port,
            http_request(
                "POST",
                "/v1/chat/completions",
                &[("content-type", "application/json")],
                &handy_fixture_with_model("claude"),
            ),
        )
        .await
    });
    // Give the request time to reach the pipeline and start the fake.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let started = std::time::Instant::now();
    let response = raw_http(server.port, http_request("GET", "/health", &[], b"")).await;
    let elapsed = started.elapsed();

    assert_eq!(response.status, 200, "body: {}", response.body_text());
    assert_eq!(response.body_json()["status"], "ok");
    assert!(
        elapsed < Duration::from_millis(300),
        "health took {elapsed:?} while a dictation was running"
    );
    assert!(
        !dictation.is_finished(),
        "the dictation must still be running when health answers"
    );

    let dictation = dictation.await.expect("dictation task completes");
    assert_eq!(dictation.status, 200);
    assert_eq!(
        dictation.body_json()["choices"][0]["message"]["content"],
        result
    );
    assert!(server.fake.report_path().exists(), "the fake ran");
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
    assert_eq!(
        ids,
        ["claude", "passthrough", "inspect"],
        "codex is disabled in the test config"
    );
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
async fn models_list_generic_only_when_enabled() {
    // Enabled: listed even though nothing listens at the endpoint. Detection
    // deliberately does no reachability probe for the generic adapter; the
    // call-time check (`EndpointUnavailable`) is what triggers fallback.
    let yaml = "providers:\n  claude:\n    binary: '{binary}'\n  codex:\n    enabled: false\n  generic:\n    enabled: true\n    model: qwen2.5-7b\n    options:\n      base_url: \"http://127.0.0.1:11434/v1\"\n";
    let server = start_server(yaml, success_scenario("unused")).await;
    let response = raw_http(server.port, http_request("GET", "/v1/models", &[], b"")).await;

    assert_eq!(response.status, 200);
    assert_eq!(
        listed_model_ids(&response.body_json()),
        ["claude", "generic", "passthrough", "inspect"],
        "the default provider leads, then registry order"
    );
    assert!(
        !server.fake.report_path().exists(),
        "listing never invokes a CLI"
    );

    // Disabled (the default config): absent from the list.
    let server = start_server(CLAUDE_AT_FAKE, success_scenario("unused")).await;
    let response = raw_http(server.port, http_request("GET", "/v1/models", &[], b"")).await;

    assert_eq!(response.status, 200);
    assert_eq!(
        listed_model_ids(&response.body_json()),
        ["claude", "passthrough", "inspect"],
        "a disabled generic is not listed"
    );
}

/// The model IDs of a `/v1/models` body, in order.
fn listed_model_ids(body: &Value) -> Vec<String> {
    body["data"]
        .as_array()
        .expect("data is an array")
        .iter()
        .map(|entry| entry["id"].as_str().expect("id is a string").to_owned())
        .collect()
}

/// Posts the recorded Handy request with `model` set, returning the response.
async fn post_model(server_port: u16, model: &str) -> RawResponse {
    raw_http(
        server_port,
        http_request(
            "POST",
            "/v1/chat/completions",
            &[("content-type", "application/json")],
            &handy_fixture_with_model(model),
        ),
    )
    .await
}

#[tokio::test]
async fn models_list_the_default_provider_first() {
    let server = start_dual_server(
        BOTH_AT_FAKES,
        success_scenario("unused"),
        codex_success_scenario("unused"),
    )
    .await;
    let response = raw_http(server.port, http_request("GET", "/v1/models", &[], b"")).await;

    assert_eq!(response.status, 200);
    assert_eq!(
        listed_model_ids(&response.body_json()),
        ["claude", "codex", "passthrough", "inspect"],
        "claude is the default, so it leads the list"
    );
    assert!(
        !server.claude.report_path().exists() && !server.codex.report_path().exists(),
        "listing never invokes a CLI"
    );
}

#[tokio::test]
async fn models_list_a_non_claude_default_first() {
    let yaml = "default_provider: codex\n".to_owned() + BOTH_AT_FAKES;
    let server = start_dual_server(
        &yaml,
        success_scenario("unused"),
        codex_success_scenario("unused"),
    )
    .await;
    let response = raw_http(server.port, http_request("GET", "/v1/models", &[], b"")).await;

    assert_eq!(response.status, 200);
    assert_eq!(
        listed_model_ids(&response.body_json()),
        ["codex", "claude", "passthrough", "inspect"],
        "the default provider leads even when it is not claude"
    );
}

#[tokio::test]
async fn model_field_selects_the_named_provider() {
    let server = start_dual_server(
        BOTH_AT_FAKES,
        success_scenario("Texto do Claude."),
        codex_success_scenario("Texto do Codex."),
    )
    .await;

    let codex = post_model(server.port, "codex").await;
    assert_eq!(codex.status, 200, "body: {}", codex.body_text());
    let codex_body = codex.body_json();
    assert_eq!(codex_body["model"], "codex");
    assert_eq!(
        codex_body["choices"][0]["message"]["content"],
        "Texto do Codex."
    );
    assert!(server.codex.report_path().exists(), "codex ran");
    assert!(
        !server.claude.report_path().exists(),
        "selecting codex must not run claude"
    );

    let claude = post_model(server.port, "CLAUDE").await;
    assert_eq!(claude.status, 200, "body: {}", claude.body_text());
    let claude_body = claude.body_json();
    assert_eq!(claude_body["model"], "claude");
    assert_eq!(
        claude_body["choices"][0]["message"]["content"],
        "Texto do Claude."
    );
    assert!(server.claude.report_path().exists(), "claude ran");
}

#[tokio::test]
async fn model_matching_is_case_insensitive_and_trims_whitespace() {
    let server = start_dual_server(
        BOTH_AT_FAKES,
        success_scenario("Texto do Claude."),
        codex_success_scenario("unused"),
    )
    .await;

    // Handy users type the field by hand: "Claude " (trailing space,
    // lowercase-rest mix) must still select claude.
    let response = post_model(server.port, "Claude ").await;

    assert_eq!(response.status, 200, "body: {}", response.body_text());
    assert_eq!(response.body_json()["model"], "claude");
    assert!(server.claude.report_path().exists(), "claude ran");
    assert!(
        !server.codex.report_path().exists(),
        "the codex fake must not run"
    );
}

#[tokio::test]
async fn empty_and_missing_model_select_the_default_provider() {
    let server = start_dual_server(
        BOTH_AT_FAKES,
        success_scenario("Texto do Claude."),
        codex_success_scenario("unused"),
    )
    .await;

    let empty = post_model(server.port, "").await;
    assert_eq!(empty.status, 200, "body: {}", empty.body_text());
    assert_eq!(
        empty.body_json()["choices"][0]["message"]["content"],
        "Texto do Claude."
    );

    // The same fixture without a `model` key at all.
    let mut body: Value =
        serde_json::from_str(&support::fixture("handy-request.json")).expect("fixture parses");
    body.as_object_mut().expect("object").remove("model");
    let missing = raw_http(
        server.port,
        http_request(
            "POST",
            "/v1/chat/completions",
            &[("content-type", "application/json")],
            &serde_json::to_vec(&body).unwrap(),
        ),
    )
    .await;

    assert_eq!(missing.status, 200, "body: {}", missing.body_text());
    assert_eq!(
        missing.body_json()["choices"][0]["message"]["content"],
        "Texto do Claude."
    );
    assert!(
        server.claude.report_path().exists(),
        "the default provider ran for both requests"
    );
    assert!(
        !server.codex.report_path().exists(),
        "the codex fake must not run"
    );
}

#[tokio::test]
async fn empty_model_selects_a_non_claude_default() {
    let yaml = "default_provider: codex\n".to_owned() + BOTH_AT_FAKES;
    let server = start_dual_server(
        &yaml,
        success_scenario("unused"),
        codex_success_scenario("Texto do Codex."),
    )
    .await;

    let response = post_model(server.port, "").await;

    assert_eq!(response.status, 200, "body: {}", response.body_text());
    let body = response.body_json();
    assert_eq!(body["model"], "codex");
    assert_eq!(body["choices"][0]["message"]["content"], "Texto do Codex.");
    assert!(server.codex.report_path().exists(), "codex ran");
    assert!(
        !server.claude.report_path().exists(),
        "the claude fake must not run"
    );
}

#[tokio::test]
async fn unknown_model_returns_raw_text_and_runs_no_provider() {
    let server = start_dual_server(
        BOTH_AT_FAKES,
        success_scenario("unused"),
        codex_success_scenario("unused"),
    )
    .await;

    let response = post_model(server.port, "gpt-4").await;

    assert_eq!(response.status, 200, "raw fallback is a normal completion");
    let body = response.body_json();
    assert_eq!(
        body["choices"][0]["message"]["content"], FIXTURE_TRANSCRIPT,
        "unknown model: dictation comes back byte for byte"
    );
    assert_eq!(body["model"], "gpt-4", "the requested string is echoed");
    assert!(
        !server.claude.report_path().exists() && !server.codex.report_path().exists(),
        "no provider may run for an unknown model"
    );
}

#[tokio::test]
async fn disabled_provider_is_not_listed_and_keeps_dictation_raw() {
    let yaml = "providers:\n  claude:\n    binary: '{claude}'\n  codex:\n    enabled: false\n";
    let server = start_dual_server(
        yaml,
        success_scenario("unused"),
        codex_success_scenario("unused"),
    )
    .await;

    let models = raw_http(server.port, http_request("GET", "/v1/models", &[], b"")).await;
    assert_eq!(models.status, 200);
    assert_eq!(
        listed_model_ids(&models.body_json()),
        ["claude", "passthrough", "inspect"],
        "a disabled provider is not listed"
    );

    let response = post_model(server.port, "codex").await;
    assert_eq!(response.status, 200, "raw fallback is a normal completion");
    let body = response.body_json();
    assert_eq!(
        body["choices"][0]["message"]["content"], FIXTURE_TRANSCRIPT,
        "disabled provider: dictation comes back byte for byte"
    );
    assert_eq!(body["model"], "codex", "the selected provider is named");
    assert!(
        !server.claude.report_path().exists() && !server.codex.report_path().exists(),
        "no provider may run for a disabled selection"
    );
    let lines = server.log_lines();
    assert!(
        lines[0].contains("is disabled in the config"),
        "line: {}",
        lines[0]
    );
}

/// A running in-process server with detection cached in the pipeline, where
/// the selected provider (`claude`) has a missing binary and every other
/// registered provider has its own fake.
struct MissingSelectedServer {
    port: u16,
    log: Arc<LogCapture>,
    codex: FakeCli,
    _config_dir: TempDir,
    _missing_dir: TempDir,
}

impl MissingSelectedServer {
    fn log_lines(&self) -> Vec<String> {
        self.log.lines()
    }
}

/// Starts a detected pipeline where `claude`'s binary does not exist (inside
/// a real temporary directory, so the path is absolute) and the other
/// providers run at their own fakes. `yaml_tail` is appended after the
/// `providers:` section; `{codex}` is replaced by the codex fake's path.
async fn start_missing_selected_server(
    yaml_tail: &str,
    codex_scenario: Value,
) -> MissingSelectedServer {
    let codex = FakeCli::new(codex_scenario);
    let opencode = FakeCli::new(json!({}));
    let antigravity = FakeCli::new(json!({}));
    let missing_dir = TempDir::new().expect("temp dir");
    let missing_binary = missing_dir.path().join("claude");
    let yaml = format!(
        "providers:\n  claude:\n    binary: '{}'\n  opencode:\n    binary: '{}'\n  antigravity:\n    binary: '{}'\n{yaml_tail}",
        missing_binary.display(),
        opencode.path().display(),
        antigravity.path().display(),
    )
    .replace("{codex}", &codex.path().display().to_string());
    let config_dir = TempDir::new().expect("temp dir");
    let config_path = config_dir.path().join("pumice.yaml");
    fs::write(&config_path, yaml).expect("write config");
    let config = config::load_with_env(Some(&config_path), |_| None)
        .expect("config loads")
        .config;
    let runner = Arc::new(ProcessRunner::new());
    let built =
        providers::build_from_config(&config, Arc::clone(&runner)).expect("providers build");
    let detection = discovery::detect(&config, providers::PROVIDERS, &runner).await;
    let pipeline = Arc::new(Pipeline::with_detection(&config, built, detection));

    let listener = tokio::net::TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind ephemeral loopback port");
    let port = listener.local_addr().unwrap().port();
    let log = Arc::new(LogCapture::default());
    let serve_log = Arc::clone(&log);
    tokio::spawn(async move {
        let _ = api::serve(
            listener,
            pipeline,
            serve_log,
            Arc::new(DebugLog::disabled()),
        )
        .await;
    });
    MissingSelectedServer {
        port,
        log,
        codex,
        _config_dir: config_dir,
        _missing_dir: missing_dir,
    }
}

#[tokio::test]
async fn models_omit_an_enabled_provider_whose_binary_is_missing() {
    let server = start_missing_selected_server(
        "  codex:\n    binary: '{codex}'\n",
        codex_success_scenario("unused"),
    )
    .await;

    let response = raw_http(server.port, http_request("GET", "/v1/models", &[], b"")).await;

    assert_eq!(response.status, 200);
    assert_eq!(
        listed_model_ids(&response.body_json()),
        ["codex", "passthrough", "inspect"],
        "claude is enabled but missing, so only codex is available"
    );
}

#[tokio::test]
async fn selecting_a_missing_provider_still_falls_back_through_the_chain() {
    let server = start_missing_selected_server(
        "  codex:\n    binary: '{codex}'\nfallback_order: [codex]\n",
        codex_success_scenario("Texto do Codex."),
    )
    .await;

    let response = post_model(server.port, "claude").await;

    assert_eq!(response.status, 200, "body: {}", response.body_text());
    let body = response.body_json();
    assert_eq!(body["model"], "codex", "the fallback provider answers");
    assert_eq!(
        body["choices"][0]["message"]["content"], "Texto do Codex.",
        "the dictation is formatted by the next provider's fake"
    );
    assert!(
        server.codex.report_path().exists(),
        "codex ran after claude failed as NotInstalled"
    );
    assert!(
        server
            .log_lines()
            .iter()
            .any(|line| line.contains(" formatted ")
                && line.contains(" codex")
                && line.contains("fallback")),
        "a formatted outcome through the fallback: {:?}",
        server.log_lines()
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

/// Builds `pumice serve` YAML whose every registered provider binary points
/// at its own disposable fake, so startup detection never looks a real CLI
/// up on PATH. Returns the fakes — they must outlive the served process —
/// and the YAML.
fn hermetic_serve_yaml(port: u16, claude_scenario: Value) -> (FakeCli, Vec<FakeCli>, String) {
    let claude = FakeCli::new(claude_scenario);
    let mut fakes: Vec<FakeCli> = Vec::new();
    let mut yaml = format!(
        "port: {port}\nproviders:\n  claude:\n    binary: '{}'\n",
        claude.path().display()
    );
    for id in ["codex", "opencode", "antigravity"] {
        let fake = FakeCli::new(json!({}));
        yaml.push_str(&format!(
            "  {id}:\n    binary: '{}'\n",
            fake.path().display()
        ));
        fakes.push(fake);
    }
    (claude, fakes, yaml)
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
    let (_claude, _fakes, yaml) = hermetic_serve_yaml(port, json!({}));
    let (_dir, config_path) = write_config(&yaml);

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
        let (_claude, _fakes, yaml) = hermetic_serve_yaml(port, json!({}));
        let (_dir, config_path) = write_config(&yaml);
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

/// Spawns `pumice serve` like [`spawn_serve`] and additionally forwards every
/// stderr line, so a test can watch the startup provider status lines.
fn spawn_serve_with_stderr(
    config_path: &std::path::Path,
) -> (
    Child,
    mpsc::Receiver<Option<String>>,
    mpsc::Receiver<String>,
) {
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
    let (stdout_tx, stdout_rx) = mpsc::channel();
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
        let _ = stdout_tx.send(if line.is_empty() { None } else { Some(line) });
    });
    let stderr = child.stderr.take().expect("piped stderr");
    let (stderr_tx, stderr_rx) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = std::io::BufReader::new(stderr);
        let mut line = String::new();
        loop {
            line.clear();
            match std::io::BufRead::read_line(&mut reader, &mut line) {
                Ok(0) => break,
                Ok(_) => {
                    let trimmed = line.trim_end_matches(['\r', '\n']).to_owned();
                    if stderr_tx.send(trimmed).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    (child, stdout_rx, stderr_rx)
}

#[test]
fn serve_prints_one_status_line_per_enabled_provider() {
    let port = grab_free_port();
    let (_claude, _fakes, yaml) = hermetic_serve_yaml(
        port,
        json!({"stdout": "2.1.288 (Claude Code)", "exit_code": 0}),
    );
    let (_dir, config_path) = write_config(&yaml);
    let (mut child, _stdout_rx, stderr_rx) = spawn_serve_with_stderr(&config_path);

    // First line: allow generous startup time (process spawn plus probes on
    // a loaded CI box). Then drain the rest with a short idle timeout as the
    // end-of-output signal: after the status lines, serve prints nothing more
    // to stderr.
    let mut lines: Vec<String> = Vec::new();
    match stderr_rx.recv_timeout(Duration::from_secs(15)) {
        Ok(line) => lines.push(line),
        Err(error) => panic!("pumice printed no startup status lines: {error}"),
    }
    while let Ok(line) = stderr_rx.recv_timeout(Duration::from_millis(500)) {
        lines.push(line);
    }
    let _ = child.kill();
    let _ = child.wait();

    for wanted in [
        "provider claude: found 2.1.288",
        "provider codex: found (version unavailable)",
    ] {
        assert!(
            lines.iter().any(|line| line.contains(wanted)),
            "startup must print {wanted:?}; stderr: {lines:?}"
        );
    }
    // Disabled providers were detected but get no line.
    for id in ["opencode", "antigravity"] {
        assert!(
            !lines
                .iter()
                .any(|line| line.starts_with(&format!("provider {id}:"))),
            "disabled provider {id} must get no line: {lines:?}"
        );
    }
}

#[tokio::test]
async fn passthrough_returns_handy_transcript_without_running_any_cli() {
    let server = start_dual_server(
        BOTH_AT_FAKES,
        success_scenario("unused"),
        codex_success_scenario("unused"),
    )
    .await;
    let response = post_model(server.port, " PassThrough ").await;
    assert_eq!(response.status, 200);
    let body = response.body_json();
    assert_eq!(body["model"], "passthrough");
    assert_eq!(body["choices"][0]["message"]["content"], FIXTURE_TRANSCRIPT);
    assert!(!server.claude.report_path().exists());
    assert!(!server.codex.report_path().exists());
}

#[tokio::test]
async fn inspect_echoes_the_exact_request_body_bytes() {
    let server = start_server(CLAUDE_AT_FAKE, success_scenario("unused")).await;
    // Deliberately irregular spacing, an unknown field and an escaped Unicode
    // code point so a deserialize-reserialize round trip would change the text.
    let raw_body =
        br#"{ "model" : "inspect" , "custom_field" : [1,2] , "messages" : [{"role":"user","content":"  \u00e9  "}] , "stream" : false }"#;
    let response = raw_http(
        server.port,
        http_request(
            "POST",
            "/v1/chat/completions",
            &[("content-type", "application/json")],
            raw_body,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body_text());
    let body = response.body_json();
    assert_eq!(body["model"], "inspect");
    let echoed = body["choices"][0]["message"]["content"]
        .as_str()
        .expect("content is a string");
    assert_eq!(echoed.as_bytes(), raw_body, "body echoed byte for byte");
    assert!(
        !server.fake.report_path().exists(),
        "inspect must not run a CLI"
    );

    let parsed: Value = serde_json::from_str(echoed).expect("echoed content parses as JSON");
    assert_eq!(parsed["custom_field"], json!([1, 2]));
    assert_eq!(parsed["messages"][0]["content"], "  \u{00e9}  ");

    let lines = server.log_lines();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains(" inspect "), "line: {}", lines[0]);
    assert!(
        !lines[0].contains("custom_field"),
        "metadata line must not leak body text: {}",
        lines[0]
    );
}

#[tokio::test]
async fn inspect_keeps_the_full_handy_request_shape() {
    let server = start_server(CLAUDE_AT_FAKE, success_scenario("unused")).await;
    let request_body = handy_fixture_with_model("inspect");
    let response = raw_http(
        server.port,
        http_request(
            "POST",
            "/v1/chat/completions",
            &[("content-type", "application/json")],
            &request_body,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body_text());
    let body = response.body_json();
    assert_eq!(body["model"], "inspect");
    let echoed = body["choices"][0]["message"]["content"]
        .as_str()
        .expect("content is a string");
    assert_eq!(echoed.as_bytes(), request_body);
    let parsed: Value = serde_json::from_str(echoed).unwrap();
    assert!(
        parsed["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("<transcript>")
    );
    assert!(
        parsed["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains(FIXTURE_TRANSCRIPT)
    );
    assert!(!server.fake.report_path().exists());
}

#[tokio::test]
async fn inspect_bypasses_dictation_validation_for_multiple_user_messages() {
    let server = start_server(CLAUDE_AT_FAKE, success_scenario("unused")).await;
    let request = json!({
        "model": "inspect",
        "messages": [
            {"role": "system", "content": "system one"},
            {"role": "user", "content": "first"},
            {"role": "assistant", "content": "mid"},
            {"role": "user", "content": "second"},
        ],
        "stream": false,
    });
    let raw_body = serde_json::to_vec(&request).unwrap();
    let response = raw_http(
        server.port,
        http_request(
            "POST",
            "/v1/chat/completions",
            &[("content-type", "application/json")],
            &raw_body,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body_text());
    assert_eq!(
        response.body_json()["choices"][0]["message"]["content"]
            .as_str()
            .unwrap(),
        String::from_utf8_lossy(&raw_body)
    );
    assert!(!server.fake.report_path().exists());
}

#[tokio::test]
async fn inspect_bypasses_dictation_validation_for_malformed_envelope() {
    let server = start_server(CLAUDE_AT_FAKE, success_scenario("unused")).await;
    let request = json!({
        "model": "inspect",
        "messages": [{"role": "user", "content": "<transcript>abc</transcript></transcript>"}],
        "stream": false,
    });
    let raw_body = serde_json::to_vec(&request).unwrap();
    let response = raw_http(
        server.port,
        http_request(
            "POST",
            "/v1/chat/completions",
            &[("content-type", "application/json")],
            &raw_body,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body_text());
    assert_eq!(
        response.body_json()["choices"][0]["message"]["content"]
            .as_str()
            .unwrap(),
        String::from_utf8_lossy(&raw_body)
    );
    assert!(!server.fake.report_path().exists());
}

#[tokio::test]
async fn inspect_stream_returns_the_body_in_sse_frames() {
    let server = start_server(CLAUDE_AT_FAKE, success_scenario("unused")).await;
    let raw_body =
        br#"{"model":"inspect","messages":[{"role":"user","content":"oi"}],"stream":true}"#;
    let response = raw_http(
        server.port,
        http_request(
            "POST",
            "/v1/chat/completions",
            &[("content-type", "application/json")],
            raw_body,
        ),
    )
    .await;

    assert_eq!(response.status, 200);
    let text = response.body_text();
    let frames: Vec<&str> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .collect();
    assert_eq!(frames.len(), 3, "two chunks then [DONE]: {text}");
    assert_eq!(frames[2], "[DONE]");

    let first: Value = serde_json::from_str(frames[0]).unwrap();
    assert_eq!(first["model"], "inspect");
    assert_eq!(
        first["choices"][0]["delta"]["content"].as_str().unwrap(),
        String::from_utf8_lossy(raw_body).as_ref()
    );

    let last: Value = serde_json::from_str(frames[1]).unwrap();
    assert_eq!(last["choices"][0]["delta"], json!({}));
    assert_eq!(last["choices"][0]["finish_reason"], "stop");
    assert!(!server.fake.report_path().exists());
}

#[tokio::test]
async fn inspect_is_not_blocked_by_a_busy_formatting_run() {
    let server = start_server(
        CLAUDE_AT_FAKE,
        json!({"stdout": success_envelope("Olá."), "exit_code": 0, "sleep_ms": 2000}),
    )
    .await;

    let port = server.port;
    let _busy = tokio::spawn(async move {
        raw_http(
            port,
            http_request(
                "POST",
                "/v1/chat/completions",
                &[("content-type", "application/json")],
                &handy_fixture_with_model("claude"),
            ),
        )
        .await
    });

    // Wait until the first request has started the fake and holds the busy permit.
    let started_by = std::time::Instant::now() + Duration::from_secs(5);
    while !server.fake.report_path().exists() {
        assert!(std::time::Instant::now() < started_by, "fake never started");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let raw_body = br#"{"model":"inspect","messages":[{"role":"user","content":"busy test"}]}"#;
    let started = std::time::Instant::now();
    let response = raw_http(
        server.port,
        http_request(
            "POST",
            "/v1/chat/completions",
            &[("content-type", "application/json")],
            raw_body,
        ),
    )
    .await;
    let elapsed = started.elapsed();

    assert_eq!(response.status, 200, "body: {}", response.body_text());
    assert!(
        elapsed < Duration::from_millis(500),
        "inspect must bypass the busy guard, took {elapsed:?}"
    );
    assert_eq!(
        response.body_json()["choices"][0]["message"]["content"]
            .as_str()
            .unwrap(),
        String::from_utf8_lossy(raw_body)
    );
}

#[tokio::test]
async fn inspect_selection_is_case_insensitive_and_trims_whitespace() {
    let server = start_server(CLAUDE_AT_FAKE, success_scenario("unused")).await;
    let request = json!({
        "model": " Inspect ",
        "messages": [{"role": "user", "content": "oi"}],
        "stream": false,
    });
    let raw_body = serde_json::to_vec(&request).unwrap();
    let response = raw_http(
        server.port,
        http_request(
            "POST",
            "/v1/chat/completions",
            &[("content-type", "application/json")],
            &raw_body,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body_text());
    let body = response.body_json();
    assert_eq!(body["model"], "inspect");
    assert!(!server.fake.report_path().exists());
}

#[tokio::test]
async fn inspect_accepts_absent_messages_null_numeric_object_content_and_arbitrary_options() {
    let server = start_server(CLAUDE_AT_FAKE, success_scenario("unused")).await;
    let cases = [
        (
            br#"{"model":"inspect","arbitrary_option":{"nested":null}}"#.as_slice(),
            "absent messages and arbitrary option",
        ),
        (
            br#"{"model":"inspect","messages":[{"role":"user","content":null}]}"#.as_slice(),
            "null content",
        ),
        (
            br#"{"model":"inspect","messages":[{"role":"user","content":123}]}"#.as_slice(),
            "numeric content",
        ),
        (
            br#"{"model":"inspect","messages":[{"role":"assistant","content":{"not":"text"}}]}"#
                .as_slice(),
            "object content",
        ),
    ];
    for (raw_body, label) in cases {
        let response = raw_http(
            server.port,
            http_request(
                "POST",
                "/v1/chat/completions",
                &[("content-type", "application/json")],
                raw_body,
            ),
        )
        .await;

        assert_eq!(response.status, 200, "{label}: {}", response.body_text());
        let body = response.body_json();
        assert_eq!(body["model"], "inspect", "{label}");
        let echoed = body["choices"][0]["message"]["content"]
            .as_str()
            .expect("content is a string");
        assert_eq!(
            echoed.as_bytes(),
            raw_body,
            "{label}: body echoed byte for byte"
        );
    }
    assert!(
        !server.fake.report_path().exists(),
        "inspect must not run a CLI"
    );
}

#[tokio::test]
async fn normal_model_rejects_absent_messages_and_unsupported_content_shapes() {
    let server = start_server(CLAUDE_AT_FAKE, success_scenario("unused")).await;
    let cases = [
        (r#"{"model":"claude","stream":false}"#, "missing messages"),
        (
            r#"{"model":"claude","messages":[{"role":"user","content":null}],"stream":false}"#,
            "null content",
        ),
        (
            r#"{"model":"claude","messages":[{"role":"user","content":123}],"stream":false}"#,
            "numeric content",
        ),
        (
            r#"{"model":"claude","messages":[{"role":"user","content":{"not":"text"}}],"stream":false}"#,
            "object content",
        ),
    ];
    for (body, label) in cases {
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

        assert_eq!(
            response.status,
            400,
            "{label} should be rejected: {}",
            response.body_text()
        );
        let response_body = response.body_json();
        assert_eq!(response_body["error"]["type"], "invalid_request_error");
        assert!(
            response_body["error"]["message"]
                .as_str()
                .expect("message is a string")
                .contains("not valid JSON"),
            "{label}: {}",
            response.body_text()
        );
    }
    assert!(
        !server.fake.report_path().exists(),
        "the fake must not run for rejected requests"
    );
}

#[tokio::test]
async fn inspect_works_with_no_available_providers_and_no_fallback() {
    let server = start_missing_selected_server("  codex:\n    enabled: false\n", json!({})).await;

    let models = raw_http(server.port, http_request("GET", "/v1/models", &[], b"")).await;
    assert_eq!(models.status, 200);
    assert_eq!(
        listed_model_ids(&models.body_json()),
        ["passthrough", "inspect"],
        "no enabled provider is available"
    );

    let raw_body = br#"{"model":"inspect","messages":[],"extra":true}"#;
    let response = raw_http(
        server.port,
        http_request(
            "POST",
            "/v1/chat/completions",
            &[("content-type", "application/json")],
            raw_body,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body_text());
    let body = response.body_json();
    assert_eq!(body["model"], "inspect");
    assert_eq!(
        body["choices"][0]["message"]["content"]
            .as_str()
            .expect("content is a string")
            .as_bytes(),
        raw_body
    );
    assert!(
        !server.codex.report_path().exists(),
        "codex must not run when disabled"
    );
}

#[tokio::test]
async fn inspect_rejects_malformed_json_encoding_and_stream_without_leaking_text() {
    let server = start_server(CLAUDE_AT_FAKE, success_scenario("unused")).await;
    let cases: &[&[u8]] = &[
        b"{\"model\":\"inspect\",\"secret-marker\":\"\xff\"}",
        br#"{"model":"inspect","secret-marker":"text"} trailing"#,
        br#"{"model":"inspect","secret-marker":"text","stream":"yes"}"#,
        br#"{"model":"inspect","secret-marker":"text","stream":1}"#,
    ];
    for bytes in cases {
        let response = raw_http(
            server.port,
            http_request(
                "POST",
                "/v1/chat/completions",
                &[("content-type", "application/json")],
                bytes,
            ),
        )
        .await;
        assert_eq!(response.status, 400);
        assert!(!response.body_text().contains("secret-marker"));
        assert_eq!(
            response.body_json()["error"]["message"],
            "the request body is not valid JSON"
        );
    }
    let bytes = br#"{"model":"inspect","stream":null}"#;
    let response = raw_http(
        server.port,
        http_request(
            "POST",
            "/v1/chat/completions",
            &[("content-type", "application/json")],
            bytes,
        ),
    )
    .await;
    assert_eq!(response.status, 200);
    assert_eq!(
        response.body_json()["choices"][0]["message"]["content"]
            .as_str()
            .unwrap()
            .as_bytes(),
        bytes
    );
    assert!(!server.fake.report_path().exists());
}
