//! Integration tests for the `pumice listen` listener: raw HTTP over TCP.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::thread;

use serde_json::Value;

use pumice::listen::{ListenOptions, serve};

/// Starts the listener on an ephemeral port and returns the bound port.
fn start_server(options: ListenOptions) -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral port");
    let port = listener.local_addr().expect("local addr").port();
    thread::spawn(move || {
        let _ = serve(listener, &options);
    });
    port
}

/// Sends a raw HTTP request, shuts down the write side, and reads the full
/// response (the server always closes the connection).
fn exchange(port: u16, request: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream.write_all(request.as_bytes()).expect("write request");
    stream.shutdown(Shutdown::Write).expect("shutdown write");
    let mut response = Vec::new();
    stream.read_to_end(&mut response).expect("read response");
    String::from_utf8_lossy(&response).into_owned()
}

fn post_chat(body: &str) -> String {
    format!(
        "POST /v1/chat/completions HTTP/1.1\r\n\
         Host: localhost\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        len = body.len()
    )
}

fn response_body(response: &str) -> &str {
    response
        .split("\r\n\r\n")
        .nth(1)
        .expect("response has a body")
}

fn response_status(response: &str) -> &str {
    response.lines().next().expect("status line")
}

#[test]
fn models_endpoint_returns_model_list() {
    let port = start_server(ListenOptions {
        reply_prefix: String::new(),
        log_file: None,
    });
    let response = exchange(
        port,
        "GET /v1/models HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(response_status(&response).starts_with("HTTP/1.1 200"));
    assert!(response.contains("Content-Type: application/json"));
    let body: Value = serde_json::from_str(response_body(&response)).expect("valid JSON body");
    assert_eq!(body["object"], "list");
    assert_eq!(body["data"][0]["id"], "pumice-echo");
    assert_eq!(body["data"][0]["owned_by"], "pumice");
}

#[test]
fn chat_completions_echoes_last_user_message() {
    let port = start_server(ListenOptions {
        reply_prefix: String::new(),
        log_file: None,
    });
    let body = r#"{"model":"x","messages":[{"role":"system","content":"s"},{"role":"user","content":"hello"}]}"#;
    let response = exchange(port, &post_chat(body));
    assert!(response_status(&response).starts_with("HTTP/1.1 200"));
    let body: Value = serde_json::from_str(response_body(&response)).expect("valid JSON body");
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["model"], "x");
    assert_eq!(body["choices"][0]["message"]["content"], "hello");
    assert_eq!(body["choices"][0]["finish_reason"], "stop");
}

#[test]
fn chat_completions_prepends_reply_prefix() {
    let port = start_server(ListenOptions {
        reply_prefix: "[pumice] ".to_string(),
        log_file: None,
    });
    let body = r#"{"messages":[{"role":"user","content":"hi"}]}"#;
    let response = exchange(port, &post_chat(body));
    let body: Value = serde_json::from_str(response_body(&response)).expect("valid JSON body");
    assert_eq!(body["choices"][0]["message"]["content"], "[pumice] hi");
    // No model in the request: falls back to pumice-echo.
    assert_eq!(body["model"], "pumice-echo");
}

#[test]
fn chat_completions_streams_sse() {
    let port = start_server(ListenOptions {
        reply_prefix: String::new(),
        log_file: None,
    });
    let body = r#"{"model":"x","stream":true,"messages":[{"role":"user","content":"stream me"}]}"#;
    let response = exchange(port, &post_chat(body));
    assert!(response_status(&response).starts_with("HTTP/1.1 200"));
    assert!(response.contains("Content-Type: text/event-stream"));
    let payload = response_body(&response);
    assert!(payload.ends_with("data: [DONE]\n\n"));
    let first_line = payload.lines().next().expect("first SSE event");
    let chunk: Value = serde_json::from_str(&first_line["data: ".len()..]).expect("chunk JSON");
    assert_eq!(chunk["object"], "chat.completion.chunk");
    assert_eq!(chunk["choices"][0]["delta"]["content"], "stream me");
}

#[test]
fn chat_completions_supports_array_content_parts() {
    let port = start_server(ListenOptions {
        reply_prefix: String::new(),
        log_file: None,
    });
    let body = r#"{"messages":[{"role":"user","content":[{"type":"text","text":"part one "},{"type":"text","text":"part two"}]}]}"#;
    let response = exchange(port, &post_chat(body));
    let body: Value = serde_json::from_str(response_body(&response)).expect("valid JSON body");
    assert_eq!(
        body["choices"][0]["message"]["content"],
        "part one part two"
    );
}

#[test]
fn chat_completions_rejects_invalid_json_with_400() {
    let port = start_server(ListenOptions {
        reply_prefix: String::new(),
        log_file: None,
    });
    let response = exchange(port, &post_chat("this is not json"));
    assert!(response_status(&response).starts_with("HTTP/1.1 400"));
    let body: Value =
        serde_json::from_str(response_body(&response)).expect("valid JSON error body");
    assert_eq!(body["error"]["type"], "invalid_request_error");
}

#[test]
fn chat_completions_rejects_missing_user_message_with_400() {
    let port = start_server(ListenOptions {
        reply_prefix: String::new(),
        log_file: None,
    });
    let body = r#"{"messages":[{"role":"system","content":"s"}]}"#;
    let response = exchange(port, &post_chat(body));
    assert!(response_status(&response).starts_with("HTTP/1.1 400"));
}

#[test]
fn unknown_path_returns_404_with_error_json() {
    let port = start_server(ListenOptions {
        reply_prefix: String::new(),
        log_file: None,
    });
    let response = exchange(
        port,
        "GET /v1/banana HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(response_status(&response).starts_with("HTTP/1.1 404"));
    let body: Value =
        serde_json::from_str(response_body(&response)).expect("valid JSON error body");
    assert_eq!(body["error"]["type"], "invalid_request_error");
}

#[test]
fn accepts_chunked_request_body() {
    let port = start_server(ListenOptions {
        reply_prefix: String::new(),
        log_file: None,
    });
    let body = r#"{"messages":[{"role":"user","content":"hello"}]}"#;
    let (first, second) = body.split_at(10);
    let chunks = format!(
        "{:x}\r\n{first}\r\n{:x}\r\n{second}\r\n0\r\n\r\n",
        first.len(),
        second.len()
    );
    let request = format!(
        "POST /v1/chat/completions HTTP/1.1\r\n\
         Host: localhost\r\n\
         Transfer-Encoding: chunked\r\n\
         Connection: close\r\n\
         \r\n\
         {chunks}"
    );
    let response = exchange(port, &request);
    assert!(response_status(&response).starts_with("HTTP/1.1 200"));
    let body: Value = serde_json::from_str(response_body(&response)).expect("valid JSON body");
    assert_eq!(body["choices"][0]["message"]["content"], "hello");
}

#[test]
fn any_path_suffix_matching_routes() {
    // Handy may use a non-/v1 prefix; routing is by path suffix.
    let port = start_server(ListenOptions {
        reply_prefix: String::new(),
        log_file: None,
    });
    let response = exchange(
        port,
        "GET /api/openai/models HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(response_status(&response).starts_with("HTTP/1.1 200"));
}

#[test]
fn logs_request_with_redacted_headers_and_body_to_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    let log_path = dir.path().join("pumice.log");
    let port = start_server(ListenOptions {
        reply_prefix: String::new(),
        log_file: Some(log_path.clone()),
    });

    let body = r#"{"messages":[{"role":"user","content":"log me"}]}"#;
    let request = format!(
        "POST /v1/chat/completions HTTP/1.1\r\n\
         Host: localhost\r\n\
         Authorization: Bearer abc\r\n\
         X-Custom-Api-Key: supersecretvalue\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        len = body.len()
    );
    exchange(port, &request);

    // The server logs before responding, so the block is complete by the time
    // the response has been read.
    let logged = std::fs::read_to_string(&log_path).expect("log file written");
    assert!(
        logged.contains("POST /v1/chat/completions"),
        "logged request line: {logged}"
    );
    assert!(
        logged.contains("<redacted, 10 chars>"),
        "authorization redacted: {logged}"
    );
    assert!(
        logged.contains("<redacted, 16 chars>"),
        "api key header redacted: {logged}"
    );
    assert!(logged.contains("log me"), "body logged: {logged}");
}
