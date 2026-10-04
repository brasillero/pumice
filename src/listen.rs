//! The `pumice listen` logging listener.
//!
//! Accepts any method and path on an already-bound listener, logs a block per
//! request to stdout (and optionally a log file), and answers the two
//! OpenAI-compatible endpoints (`/v1/models`, `/v1/chat/completions`) with
//! echo-style responses so Handy can be pointed at it during development.

use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use crate::http::{self, ParseOutcome, Request};
use crate::time;

/// Configuration for [`serve`].
pub struct ListenOptions {
    /// Prepended to the echoed text of chat completion responses.
    pub reply_prefix: String,
    /// When set, the same per-request log block written to stdout is appended
    /// to this file.
    pub log_file: Option<std::path::PathBuf>,
}

struct ServerState {
    reply_prefix: String,
    counter: AtomicU64,
    log_file: Option<Mutex<BufWriter<File>>>,
}

/// Serves requests on `listener` forever, one thread per connection.
pub fn serve(listener: TcpListener, options: &ListenOptions) -> io::Result<()> {
    let log_file = options
        .log_file
        .as_ref()
        .map(|p| open_log_file(p))
        .transpose()?;
    let state = Arc::new(ServerState {
        reply_prefix: options.reply_prefix.clone(),
        counter: AtomicU64::new(0),
        log_file: log_file.map(Mutex::new),
    });
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let state = Arc::clone(&state);
                thread::spawn(move || {
                    let _ = handle_connection(stream, &state);
                });
            }
            Err(_) => continue,
        }
    }
    Ok(())
}

fn open_log_file(path: &Path) -> io::Result<BufWriter<File>> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map(BufWriter::new)
        .map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("cannot open log file {}: {e}", path.display()),
            )
        })
}

fn handle_connection(stream: TcpStream, state: &ServerState) -> io::Result<()> {
    // An idle or stalled client must not hold its thread forever.
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let remote = stream
        .peer_addr()
        .map(|addr: SocketAddr| addr.to_string())
        .unwrap_or_else(|_| "?".to_string());
    let mut reader = BufReader::new(stream);
    let outcome = http::read_request(&mut reader);
    let mut stream = reader.into_inner();

    let response = match outcome {
        ParseOutcome::Empty => return Ok(()),
        ParseOutcome::Ok(request) => {
            let mut block = http::format_head(&remote, &request.head);
            block.push_str("body:\n");
            block.push_str(&http::format_body(&request.body));
            block.push_str("\n===== end request =====\n");
            write_log(state, &block);
            route(&request, state)
        }
        ParseOutcome::BodyTooLarge(head) => {
            let mut block = http::format_head(&remote, &head);
            block.push_str(&format!(
                "body: <exceeded {0} byte cap>\n===== end request =====\n",
                http::MAX_BODY_BYTES
            ));
            write_log(state, &block);
            Response::error(
                413,
                &format!("request body exceeds the {} byte cap", http::MAX_BODY_BYTES),
            )
        }
        ParseOutcome::BadRequest(message) => Response::error(400, &message),
    };
    response.write_to(&mut stream)
}

fn write_log(state: &ServerState, block: &str) {
    println!("{block}");
    if let Some(file) = &state.log_file
        && let Ok(mut file) = file.lock()
    {
        let _ = file.write_all(block.as_bytes());
        let _ = file.flush();
    }
}

fn route(request: &Request, state: &ServerState) -> Response {
    let method = request.head.method.as_str();
    let path = request.head.path();
    if method == "GET" && path.ends_with("/models") {
        Response::json(200, models_body())
    } else if method == "POST" && path.ends_with("/chat/completions") {
        match extract_last_user_text(&request.body) {
            Ok(extraction) => {
                let reply = format!("{}{}", state.reply_prefix, extraction.text);
                let id = state.counter.fetch_add(1, Ordering::Relaxed);
                let now = time::now_unix_secs();
                if extraction.stream {
                    Response::sse(stream_events(&reply, extraction.model.as_deref(), id, now))
                } else {
                    Response::json(200, chat_body(&reply, extraction.model.as_deref(), id, now))
                }
            }
            Err(message) => Response::error(400, &message),
        }
    } else {
        Response::error(404, &format!("no route for {method} {path}"))
    }
}

/// The extracted fields of a chat completion request that we echo back.
#[derive(Debug)]
pub struct Extraction {
    pub text: String,
    pub model: Option<String>,
    pub stream: bool,
}

/// Extracts the text of the LAST `role == "user"` message. Content may be a
/// plain string or an array of parts like `[{"type":"text","text":"..."}]`,
/// in which case the text parts are concatenated.
pub fn extract_last_user_text(body: &[u8]) -> Result<Extraction, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| "request body is not valid JSON".to_string())?;
    let messages = value
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| "request body has no \"messages\" array".to_string())?;

    let mut last_user_text: Option<String> = None;
    for message in messages {
        if message.get("role").and_then(Value::as_str) == Some("user") {
            let content = message.get("content");
            last_user_text = Some(
                extract_content_text(content)
                    .ok_or_else(|| "user message content is not usable text".to_string())?,
            );
        }
    }
    let text = last_user_text.ok_or_else(|| "no user message found in request".to_string())?;

    Ok(Extraction {
        text,
        model: value.get("model").and_then(Value::as_str).map(String::from),
        stream: value
            .get("stream")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn extract_content_text(content: Option<&Value>) -> Option<String> {
    match content? {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => {
            let mut text = String::new();
            let mut any_text_part = false;
            for part in parts {
                if part.get("type").and_then(Value::as_str) == Some("text") {
                    any_text_part = true;
                    text.push_str(part.get("text").and_then(Value::as_str)?);
                }
            }
            any_text_part.then_some(text)
        }
        _ => None,
    }
}

/// The `GET .../models` response body.
pub fn models_body() -> String {
    json!({
        "object": "list",
        "data": [{
            "id": "pumice-echo",
            "object": "model",
            "created": 0,
            "owned_by": "pumice",
        }],
    })
    .to_string()
}

/// The non-streaming chat completion response body.
pub fn chat_body(reply: &str, model: Option<&str>, id: u64, now_secs: u64) -> String {
    json!({
        "id": format!("chatcmpl-pumice-{id}"),
        "object": "chat.completion",
        "created": now_secs,
        "model": model.unwrap_or("pumice-echo"),
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": reply},
            "finish_reason": "stop",
        }],
        "usage": {
            "prompt_tokens": 0,
            "completion_tokens": 0,
            "total_tokens": 0,
        },
    })
    .to_string()
}

/// The SSE payload for a streaming chat completion: one chunk with the full
/// reply delta, one chunk with an empty delta and `finish_reason: "stop"`,
/// then `data: [DONE]`. Every event ends with a blank line.
pub fn stream_events(reply: &str, model: Option<&str>, id: u64, now_secs: u64) -> String {
    let model = model.unwrap_or("pumice-echo");
    let first = json!({
        "id": format!("chatcmpl-pumice-{id}"),
        "object": "chat.completion.chunk",
        "created": now_secs,
        "model": model,
        "choices": [{
            "index": 0,
            "delta": {"role": "assistant", "content": reply},
            "finish_reason": null,
        }],
    });
    let second = json!({
        "id": format!("chatcmpl-pumice-{id}"),
        "object": "chat.completion.chunk",
        "created": now_secs,
        "model": model,
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": "stop",
        }],
    });
    format!("data: {first}\n\ndata: {second}\n\ndata: [DONE]\n\n")
}

/// A complete HTTP response ready to be written to a stream.
struct Response {
    status: u16,
    reason: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
}

impl Response {
    fn json(status: u16, body: String) -> Self {
        Response {
            status,
            reason: reason(status),
            content_type: "application/json",
            body: body.into_bytes(),
        }
    }

    fn sse(body: String) -> Self {
        Response {
            status: 200,
            reason: "OK",
            content_type: "text/event-stream",
            body: body.into_bytes(),
        }
    }

    fn error(status: u16, message: &str) -> Self {
        let body = json!({
            "error": {
                "message": message,
                "type": "invalid_request_error",
            },
        })
        .to_string();
        Response::json(status, body)
    }

    fn write_to(&self, stream: &mut TcpStream) -> io::Result<()> {
        let head = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.status,
            self.reason,
            self.content_type,
            self.body.len()
        );
        stream.write_all(head.as_bytes())?;
        stream.write_all(&self.body)?;
        stream.flush()
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        413 => "Content Too Large",
        _ => "Error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_last_user_message_with_string_content() {
        let body = br#"{
            "model": "x",
            "messages": [
                {"role": "system", "content": "s"},
                {"role": "user", "content": "first"},
                {"role": "assistant", "content": "a"},
                {"role": "user", "content": "last"}
            ]
        }"#;
        let extraction = extract_last_user_text(body).unwrap();
        assert_eq!(extraction.text, "last");
        assert_eq!(extraction.model.as_deref(), Some("x"));
        assert!(!extraction.stream);
    }

    #[test]
    fn extracts_last_user_message_with_array_content() {
        let body = br#"{
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "one "},
                    {"type": "image_url", "url": "ignored"},
                    {"type": "text", "text": "two"}
                ]}
            ]
        }"#;
        let extraction = extract_last_user_text(body).unwrap();
        assert_eq!(extraction.text, "one two");
        assert_eq!(extraction.model, None);
    }

    #[test]
    fn errors_on_invalid_json() {
        let err = extract_last_user_text(b"not json").unwrap_err();
        assert!(err.contains("valid JSON"));
    }

    #[test]
    fn errors_without_user_message() {
        let err = extract_last_user_text(br#"{"messages": [{"role": "system", "content": "s"}]}"#)
            .unwrap_err();
        assert!(err.contains("no user message"));
    }

    #[test]
    fn errors_when_content_is_not_text() {
        let err = extract_last_user_text(br#"{"messages": [{"role": "user", "content": 42}]}"#)
            .unwrap_err();
        assert!(err.contains("content"));
    }

    #[test]
    fn models_body_is_valid_openai_model_list() {
        let value: Value = serde_json::from_str(&models_body()).unwrap();
        assert_eq!(value["object"], "list");
        assert_eq!(value["data"][0]["id"], "pumice-echo");
        assert_eq!(value["data"][0]["object"], "model");
        assert_eq!(value["data"][0]["owned_by"], "pumice");
    }

    #[test]
    fn chat_body_is_valid_completion() {
        let body = chat_body("hello there", Some("my-model"), 7, 1_700_000_000);
        let value: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["id"], "chatcmpl-pumice-7");
        assert_eq!(value["object"], "chat.completion");
        assert_eq!(value["created"], 1_700_000_000);
        assert_eq!(value["model"], "my-model");
        assert_eq!(value["choices"][0]["message"]["role"], "assistant");
        assert_eq!(value["choices"][0]["message"]["content"], "hello there");
        assert_eq!(value["choices"][0]["finish_reason"], "stop");
        assert_eq!(value["usage"]["total_tokens"], 0);
    }

    #[test]
    fn chat_body_defaults_model_to_pumice_echo() {
        let value: Value = serde_json::from_str(&chat_body("hi", None, 0, 0)).unwrap();
        assert_eq!(value["model"], "pumice-echo");
    }

    #[test]
    fn stream_events_have_valid_sse_framing() {
        let events = stream_events("reply text", Some("m"), 3, 42);
        assert!(events.ends_with("data: [DONE]\n\n"));
        let data_lines: Vec<&str> = events
            .split("\n\n")
            .filter(|block| !block.is_empty())
            .collect();
        assert_eq!(data_lines.len(), 3);
        for (i, block) in data_lines.iter().enumerate() {
            assert!(block.starts_with("data: "), "block {i} missing data prefix");
            let payload = &block["data: ".len()..];
            if payload != "[DONE]" {
                let chunk: Value = serde_json::from_str(payload)
                    .unwrap_or_else(|e| panic!("block {i} is not valid JSON: {e}"));
                assert_eq!(chunk["object"], "chat.completion.chunk");
                assert_eq!(chunk["id"], "chatcmpl-pumice-3");
            }
        }
        let first: Value = serde_json::from_str(&data_lines[0]["data: ".len()..]).unwrap();
        assert_eq!(first["choices"][0]["delta"]["role"], "assistant");
        assert_eq!(first["choices"][0]["delta"]["content"], "reply text");
        assert_eq!(first["choices"][0]["finish_reason"], Value::Null);
        let second: Value = serde_json::from_str(&data_lines[1]["data: ".len()..]).unwrap();
        assert_eq!(second["choices"][0]["delta"], json!({}));
        assert_eq!(second["choices"][0]["finish_reason"], "stop");
    }
}
