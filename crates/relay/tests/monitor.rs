//! Tests for the live request monitor (S4.6/S4.7): event order, text gating,
//! request/response capture and the terminal lifecycle events emitted by the
//! API layer.

mod support;

use std::fs;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pumice::api;
use pumice::config;
use pumice::logging::DebugLog;
use pumice::monitor::{Event, EventKind};
use pumice::pipeline::{OutcomeKind, Pipeline};
use pumice::process::ProcessRunner;
use pumice::providers;
use serde_json::{Value, json};
use support::FakeCli;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const CLAUDE_AT_FAKE: &str =
    "providers:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: '{binary}'\n";

fn success_envelope(result: &str) -> String {
    json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "result": result,
    })
    .to_string()
}

fn success_scenario(result: &str) -> Value {
    json!({"stdout": success_envelope(result), "exit_code": 0})
}

/// Captures every monitor event delivered to it.
#[derive(Default)]
struct EventCapture {
    events: Mutex<Vec<Event>>,
}

impl pumice::monitor::Monitor for EventCapture {
    fn event(&self, event: Event) {
        self.events.lock().unwrap().push(event);
    }
}

impl EventCapture {
    fn events(&self) -> Vec<Event> {
        self.events.lock().unwrap().clone()
    }
}

async fn start_monitored_server(
    yaml: &str,
    scenario: Value,
    debug_log: Arc<DebugLog>,
) -> (u16, Arc<EventCapture>, FakeCli) {
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

    let listener = tokio::net::TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind ephemeral loopback port");
    let port = listener.local_addr().unwrap().port();
    let monitor = Arc::new(EventCapture::default());
    let serve_monitor = Arc::clone(&monitor);
    tokio::spawn(async move {
        let _ = api::serve_monitored(
            listener,
            pipeline,
            Arc::new(api::NullLog),
            debug_log,
            serve_monitor,
            std::future::pending(),
        )
        .await;
    });
    (port, monitor, fake)
}

fn handy_request(model: &str, text: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "model": model,
        "messages": [{"role": "user", "content": format!("<transcript>\n{}\n</transcript>", text)}],
        "stream": false,
    }))
    .unwrap()
}

async fn raw_http(port: u16, request: Vec<u8>) -> (u16, String) {
    let mut stream = TcpStream::connect(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
        .await
        .expect("connect");
    stream.write_all(&request).await.expect("send");
    let mut response = Vec::new();
    let mut chunk = [0u8; 8 * 1024];
    loop {
        match stream.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&response).into_owned();
    let status = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (status, text)
}

fn http_request(path: &str, body: &[u8]) -> Vec<u8> {
    http_request_with_headers(path, body, &[])
}

fn http_request_with_headers(path: &str, body: &[u8], headers: &[(&str, &str)]) -> Vec<u8> {
    let mut header_lines = String::new();
    for (name, value) in headers {
        header_lines.push_str(&format!("{name}: {value}\r\n"));
    }
    format!(
        "POST {path} HTTP/1.1\r\nhost: 127.0.0.1\r\nconnection: close\r\n{header_lines}content-length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes()
    .into_iter()
    .chain(body.iter().copied())
    .collect()
}

#[tokio::test]
async fn formatted_request_emits_arrived_parsed_started_attempt_ended_responded() {
    let (port, monitor, _fake) = start_monitored_server(
        CLAUDE_AT_FAKE,
        success_scenario("Texto formatado."),
        Arc::new(DebugLog::disabled()),
    )
    .await;

    let (status, _) = raw_http(
        port,
        http_request("/v1/chat/completions", &handy_request("claude", "ola")),
    )
    .await;
    assert_eq!(status, 200);

    // Give the response handler time to emit the final event.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let events = monitor.events();
    let kinds: Vec<_> = events.iter().map(|e| e.number).collect();
    assert!(
        kinds.windows(2).all(|w| w[0] == w[1]),
        "same number throughout"
    );
    let number = events.first().expect("arrived").number;

    let kind_names: Vec<_> = events
        .iter()
        .map(|e| match &e.kind {
            EventKind::Arrived { .. } => "Arrived",
            EventKind::Received { .. } => "Received",
            EventKind::Parsed { .. } => "Parsed",
            EventKind::Queued => "Queued",
            EventKind::Started { .. } => "Started",
            EventKind::AttemptEnded { .. } => "AttemptEnded",
            EventKind::Responded { .. } => "Responded",
            EventKind::Sent { .. } => "Sent",
            EventKind::Dropped => "Dropped",
        })
        .collect();
    assert_eq!(
        kind_names,
        ["Arrived", "Parsed", "Started", "AttemptEnded", "Responded"]
    );

    let parsed = events.iter().find_map(|e| match &e.kind {
        EventKind::Parsed { model, text } => Some((model.clone(), text.is_some())),
        _ => None,
    });
    assert_eq!(parsed, Some((Some("claude".to_owned()), false)));

    let responded = events.iter().find_map(|e| match &e.kind {
        EventKind::Responded {
            status,
            outcome,
            reply,
            ..
        } => Some((*status, *outcome, reply.clone())),
        _ => None,
    });
    assert_eq!(responded, Some((200, Some(OutcomeKind::Formatted), None)));

    assert!(events.iter().all(|e| e.number == number));
}

#[tokio::test]
async fn debug_log_enabled_shows_input_and_reply_in_events() {
    let (port, monitor, _fake) =
        start_monitored_server(CLAUDE_AT_FAKE, success_scenario("Resposta."), {
            let dir = TempDir::new().expect("temp dir");
            let path = dir.path().join("debug.log");
            Arc::new(
                DebugLog::open(&config::DebugLogSettings {
                    enabled: true,
                    path,
                })
                .expect("open debug log"),
            )
        })
        .await;

    let text = "dictado especial";
    let (status, _) = raw_http(
        port,
        http_request("/v1/chat/completions", &handy_request("claude", text)),
    )
    .await;
    assert_eq!(status, 200);
    tokio::time::sleep(Duration::from_millis(50)).await;

    let events = monitor.events();
    let arrived = events.iter().find_map(|e| match &e.kind {
        EventKind::Arrived { client } => client.clone(),
        _ => None,
    });
    assert_eq!(arrived, None, "no X-Title means no client");

    let parsed = events.iter().find_map(|e| match &e.kind {
        EventKind::Parsed { text, .. } => text.as_ref().map(|t| t.input.clone()),
        _ => None,
    });
    assert_eq!(parsed, Some(text.to_owned()));

    let reply = events.iter().find_map(|e| match &e.kind {
        EventKind::Responded { reply, .. } => reply.clone(),
        _ => None,
    });
    assert_eq!(reply, Some("Resposta.".to_owned()));

    let received = events
        .iter()
        .any(|e| matches!(e.kind, EventKind::Received { .. }));
    assert!(received, "debug log on emits Received");

    let sent = events
        .iter()
        .any(|e| matches!(e.kind, EventKind::Sent { .. }));
    assert!(sent, "debug log on emits Sent");
}

#[tokio::test]
async fn debug_log_on_with_x_title_and_handy_request_captures_all_tabs() {
    let (port, monitor, _fake) =
        start_monitored_server(CLAUDE_AT_FAKE, success_scenario("Resposta."), {
            let dir = TempDir::new().expect("temp dir");
            let path = dir.path().join("debug.log");
            Arc::new(
                DebugLog::open(&config::DebugLogSettings {
                    enabled: true,
                    path,
                })
                .expect("open debug log"),
            )
        })
        .await;

    let text = "dictado especial";
    let body = handy_request("claude", text);
    let (status, response_text) = raw_http(
        port,
        http_request_with_headers("/v1/chat/completions", &body, &[("x-title", "Handy")]),
    )
    .await;
    assert_eq!(status, 200);
    tokio::time::sleep(Duration::from_millis(50)).await;

    let events = monitor.events();

    let arrived = events.iter().find_map(|e| match &e.kind {
        EventKind::Arrived { client } => client.clone(),
        _ => None,
    });
    assert_eq!(arrived, Some("Handy".to_owned()));

    let received = events.iter().find_map(|e| match &e.kind {
        EventKind::Received { headers, body } => Some((headers.clone(), body.clone())),
        _ => None,
    });
    let (headers, received_body) = received.expect("Received event");
    assert!(headers.iter().any(|(n, _)| n == "x-title"));
    assert!(received_body.contains("dictado especial"));

    let parsed = events.iter().find_map(|e| match &e.kind {
        EventKind::Parsed { text, .. } => text.clone(),
        _ => None,
    });
    let text = parsed.expect("Parsed.text");
    assert!(text.system.is_empty());
    assert!(
        text.before.starts_with("<transcript>"),
        "before starts with opening tag: {:?}",
        text.before
    );
    assert_eq!(text.input, "dictado especial");
    assert!(
        text.after.ends_with("</transcript>"),
        "after ends with closing tag: {:?}",
        text.after
    );

    let sent = events.iter().find_map(|e| match &e.kind {
        EventKind::Sent {
            status,
            headers,
            body,
        } => Some((*status, headers.clone(), body.clone())),
        _ => None,
    });
    let (sent_status, sent_headers, sent_body) = sent.expect("Sent event");
    // Same request: the Sent event holds exactly what the client received.
    assert_eq!(sent_status, 200);
    assert_eq!(sent_body, response_body(&response_text));
    for (name, value) in &sent_headers {
        assert!(
            response_text.contains(&format!("{name}: {value}\r\n")),
            "client got header {name}: {value}"
        );
    }
}

/// Returns the body of an HTTP/1.1 response, i.e. everything after the first
/// blank line. If there is no blank line, returns the whole string.
fn response_body(text: &str) -> &str {
    text.split_once("\r\n\r\n")
        .or_else(|| text.split_once("\n\n"))
        .map(|(_, body)| body)
        .unwrap_or(text)
}

/// Sets every `"created":` number in a body to 0: the one value two
/// otherwise identical responses differ in.
fn zero_created(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some(index) = rest.find("\"created\":") {
        let (head, tail) = rest.split_at(index + "\"created\":".len());
        out.push_str(head);
        out.push('0');
        rest = tail.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    out.push_str(rest);
    out
}

fn debug_log(enabled: bool) -> (Arc<DebugLog>, Option<TempDir>) {
    if !enabled {
        return (Arc::new(DebugLog::disabled()), None);
    }
    let dir = TempDir::new().expect("temp dir");
    let log = DebugLog::open(&config::DebugLogSettings {
        enabled: true,
        path: dir.path().join("debug.log"),
    })
    .expect("open debug log");
    (Arc::new(log), Some(dir))
}

/// Status, headers (without `date`) and body of the response a fresh server
/// gives `request`, with the debug log on or off.
async fn exchange(debug: bool, request: &[u8]) -> (u16, Vec<String>, String) {
    let (log, _dir) = debug_log(debug);
    let (port, _monitor, _fake) =
        start_monitored_server(CLAUDE_AT_FAKE, success_scenario("Resposta."), log).await;
    let (status, text) = raw_http(port, request.to_vec()).await;
    let (head, body) = text.split_once("\r\n\r\n").expect("a full response");
    let headers = head
        .lines()
        .skip(1)
        .filter(|line| !line.to_ascii_lowercase().starts_with("date:"))
        .map(str::to_owned)
        .collect();
    (status, headers, zero_created(body))
}

#[tokio::test]
async fn responses_are_identical_with_the_debug_log_on_and_off() {
    let streaming = serde_json::to_vec(&json!({
        "model": "claude",
        "messages": [{"role": "user", "content": "<transcript>\nola\n</transcript>"}],
        "stream": true,
    }))
    .unwrap();
    let cases: [(&str, Vec<u8>); 5] = [
        ("formatted", handy_request("claude", "ola")),
        ("streamed", streaming),
        ("inspect", handy_request("inspect", "ola")),
        ("invalid json", b"{not json".to_vec()),
        ("unknown model", handy_request("nope", "ola")),
    ];
    for (name, body) in cases {
        let request = http_request("/v1/chat/completions", &body);
        let off = exchange(false, &request).await;
        let on = exchange(true, &request).await;
        assert_eq!(on, off, "{name}");
    }
}

#[tokio::test]
async fn a_huge_model_name_is_capped_in_events_with_the_debug_log_on_and_off() {
    for debug in [false, true] {
        let (log, _dir) = debug_log(debug);
        let (port, monitor, _fake) =
            start_monitored_server(CLAUDE_AT_FAKE, success_scenario("ok"), log).await;
        let model = "m".repeat(100_000);
        let (status, _) = raw_http(
            port,
            http_request("/v1/chat/completions", &handy_request(&model, "ola")),
        )
        .await;
        assert_eq!(status, 404, "debug {debug}");
        tokio::time::sleep(Duration::from_millis(50)).await;
        let events = monitor.events();
        let parsed_model = events
            .iter()
            .find_map(|e| match &e.kind {
                EventKind::Parsed { model, .. } => model.clone(),
                _ => None,
            })
            .expect("Parsed model");
        assert!(parsed_model.len() < 70_000, "debug {debug}");
        assert!(parsed_model.contains("[cut:"), "debug {debug}");
        for event in &events {
            if let EventKind::Responded { detail, .. } = &event.kind {
                assert!(detail.len() < 70_000, "debug {debug}");
            }
        }
    }
}

#[tokio::test]
async fn debug_log_off_hides_received_and_sent_and_parsed_text() {
    let (port, monitor, _fake) = start_monitored_server(
        CLAUDE_AT_FAKE,
        success_scenario("Resposta."),
        Arc::new(DebugLog::disabled()),
    )
    .await;

    let (status, _) = raw_http(
        port,
        http_request_with_headers(
            "/v1/chat/completions",
            &handy_request("claude", "ola"),
            &[("x-title", "Handy")],
        ),
    )
    .await;
    assert_eq!(status, 200);
    tokio::time::sleep(Duration::from_millis(50)).await;

    let events = monitor.events();
    let arrived = events.iter().find_map(|e| match &e.kind {
        EventKind::Arrived { client } => client.clone(),
        _ => None,
    });
    assert_eq!(
        arrived,
        Some("Handy".to_owned()),
        "client is always present"
    );

    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, EventKind::Received { .. })),
        "no Received when debug log is off"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, EventKind::Sent { .. })),
        "no Sent when debug log is off"
    );
    let parsed_has_text = events.iter().find_map(|e| match &e.kind {
        EventKind::Parsed { text, .. } => Some(text.is_some()),
        _ => None,
    });
    assert_eq!(
        parsed_has_text,
        Some(false),
        "Parsed.text is None when debug log is off"
    );
}

#[tokio::test]
async fn invalid_json_body_emits_arrived_and_responded_400() {
    let (port, monitor, _fake) = start_monitored_server(
        CLAUDE_AT_FAKE,
        success_scenario("unused"),
        Arc::new(DebugLog::disabled()),
    )
    .await;

    let (status, _) = raw_http(
        port,
        http_request("/v1/chat/completions", b"{\"messages\": ["),
    )
    .await;
    assert_eq!(status, 400);
    tokio::time::sleep(Duration::from_millis(50)).await;

    let kinds: Vec<_> = monitor
        .events()
        .iter()
        .map(|e| match &e.kind {
            EventKind::Arrived { .. } => "Arrived",
            EventKind::Parsed { .. } => "Parsed",
            EventKind::Responded { .. } => "Responded",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["Arrived", "Responded"]);

    let responded = monitor.events().iter().find_map(|e| match &e.kind {
        EventKind::Responded {
            status, outcome, ..
        } => Some((*status, *outcome)),
        _ => None,
    });
    assert_eq!(responded, Some((400, None)));
}

#[tokio::test]
async fn invalid_json_body_with_debug_log_emits_received_before_responded_400() {
    let (port, monitor, _fake) =
        start_monitored_server(CLAUDE_AT_FAKE, success_scenario("unused"), {
            let dir = TempDir::new().expect("temp dir");
            let path = dir.path().join("debug.log");
            Arc::new(
                DebugLog::open(&config::DebugLogSettings {
                    enabled: true,
                    path,
                })
                .expect("open debug log"),
            )
        })
        .await;

    let (status, _) = raw_http(
        port,
        http_request("/v1/chat/completions", b"{\"messages\": ["),
    )
    .await;
    assert_eq!(status, 400);
    tokio::time::sleep(Duration::from_millis(50)).await;

    let kinds: Vec<_> = monitor
        .events()
        .iter()
        .map(|e| match &e.kind {
            EventKind::Arrived { .. } => "Arrived",
            EventKind::Received { .. } => "Received",
            EventKind::Parsed { .. } => "Parsed",
            EventKind::Responded { .. } => "Responded",
            EventKind::Sent { .. } => "Sent",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["Arrived", "Received", "Responded", "Sent"]);
}

#[tokio::test]
async fn provider_failure_emits_error_status_and_no_reply_even_with_debug_log() {
    let (port, monitor, _fake) = start_monitored_server(
        CLAUDE_AT_FAKE,
        json!({
            "stdout": support::fixture("claude/not-logged-in.json"),
            "exit_code": 1,
        }),
        {
            let dir = TempDir::new().expect("temp dir");
            let path = dir.path().join("debug.log");
            Arc::new(
                DebugLog::open(&config::DebugLogSettings {
                    enabled: true,
                    path,
                })
                .expect("open debug log"),
            )
        },
    )
    .await;

    let (status, _) = raw_http(
        port,
        http_request("/v1/chat/completions", &handy_request("claude", "ola")),
    )
    .await;
    assert_eq!(status, 502);
    tokio::time::sleep(Duration::from_millis(50)).await;

    let responded = monitor.events().iter().find_map(|e| match &e.kind {
        EventKind::Responded {
            status,
            outcome,
            reply,
            ..
        } => Some((*status, *outcome, reply.clone())),
        _ => None,
    });
    assert_eq!(
        responded,
        Some((
            502,
            Some(OutcomeKind::Raw(
                pumice::pipeline::RawReason::ProviderFailed(
                    pumice::providers::ProviderError::NotLoggedIn
                )
            )),
            None
        ))
    );
}

#[tokio::test]
async fn client_disconnect_while_cli_runs_emits_dropped() {
    let (port, monitor, fake) = start_monitored_server(
        CLAUDE_AT_FAKE,
        json!({"stdout": "unused", "exit_code": 0, "sleep_ms": 5_000}),
        Arc::new(DebugLog::disabled()),
    )
    .await;

    let request = http_request("/v1/chat/completions", &handy_request("claude", "ola"));
    let mut stream = TcpStream::connect(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
        .await
        .expect("connect");
    stream.write_all(&request).await.expect("send");

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !fake.report_path().exists() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    drop(stream);

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while monitor.events().len() < 4 && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let events = monitor.events();
    let last = events.last().expect("at least one event");
    assert!(matches!(last.kind, EventKind::Dropped));
}

/// The detail of the diagnostic carried by the `AttemptEnded` event.
fn diagnostic_detail(events: &[Event]) -> Option<String> {
    events.iter().find_map(|e| match &e.kind {
        EventKind::AttemptEnded { diagnostic, .. } => diagnostic
            .as_ref()
            .map(|diagnostic| diagnostic.detail.clone()),
        _ => None,
    })
}

#[tokio::test]
async fn diagnostic_output_reaches_events_only_with_the_debug_log() {
    // The failing CLI echoes the dictation on stderr, as a real CLI might.
    let echoing_failure = json!({
        "stderr": "error while handling: ditado-secreto",
        "exit_code": 1,
    });

    let (port, monitor, _fake) = start_monitored_server(
        CLAUDE_AT_FAKE,
        echoing_failure.clone(),
        Arc::new(DebugLog::disabled()),
    )
    .await;
    let (status, _) = raw_http(
        port,
        http_request(
            "/v1/chat/completions",
            &handy_request("claude", "ditado-secreto"),
        ),
    )
    .await;
    assert_eq!(status, 502);
    assert_eq!(
        diagnostic_detail(&monitor.events()).as_deref(),
        Some(""),
        "debug log off: the diagnostic keeps only its safe fields"
    );

    let dir = TempDir::new().expect("temp dir");
    let debug_log = Arc::new(
        DebugLog::open(&config::DebugLogSettings {
            enabled: true,
            path: dir.path().join("debug.log"),
        })
        .expect("open debug log"),
    );
    let (port, monitor, _fake) =
        start_monitored_server(CLAUDE_AT_FAKE, echoing_failure, debug_log).await;
    let (status, _) = raw_http(
        port,
        http_request(
            "/v1/chat/completions",
            &handy_request("claude", "ditado-secreto"),
        ),
    )
    .await;
    assert_eq!(status, 502);
    let detail = diagnostic_detail(&monitor.events()).expect("a diagnostic");
    assert!(
        detail.contains("ditado-secreto"),
        "debug log on: the CLI output is kept: {detail:?}"
    );
}

#[tokio::test]
async fn large_input_is_capped_in_events() {
    let (port, monitor, _fake) = start_monitored_server(CLAUDE_AT_FAKE, success_scenario("ok"), {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("debug.log");
        Arc::new(
            DebugLog::open(&config::DebugLogSettings {
                enabled: true,
                path,
            })
            .expect("open debug log"),
        )
    })
    .await;

    let big = "x".repeat(100_000);
    let body = handy_request("claude", &big);
    let (status, _) = raw_http(port, http_request("/v1/chat/completions", &body)).await;
    assert_eq!(status, 200);
    tokio::time::sleep(Duration::from_millis(50)).await;

    let events = monitor.events();
    let received_body = events.iter().find_map(|e| match &e.kind {
        EventKind::Received { body, .. } => Some(body.clone()),
        _ => None,
    });
    let received = received_body.expect("Received event");
    assert!(
        received.contains("[cut:"),
        "Received body over MAX_TEXT is capped"
    );

    let input = events.iter().find_map(|e| match &e.kind {
        EventKind::Parsed { text, .. } => text.as_ref().map(|t| t.input.clone()),
        _ => None,
    });
    let input = input.expect("Parsed input");
    assert!(
        input.contains("[cut:"),
        "Parsed input over MAX_TEXT is capped"
    );
}
