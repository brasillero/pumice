//! S2.7: the generic loopback adapter, tested against the fake HTTP server.
//!
//! The shared contract suite (`tests/support/adapter_contract.rs`) is shaped
//! around the fake CLI — its fixtures are literally `(stdout, exit_code)`
//! pairs — so the contract cases for this adapter live here instead, against
//! the fake HTTP server. Mapping:
//!
//! | Contract case            | Covered by                                             |
//! |--------------------------|--------------------------------------------------------|
//! | final_text_only          | `success_returns_content_unchanged`                    |
//! | restricted               | `request_is_exact` (no tools/auth; exact headers)      |
//! | empty_workspace          | n/a over HTTP; `request_is_exact` (stateless, fresh    |
//! |                          | socket per call) and `cancellation_drops_the_socket`   |
//! | transport                | `request_is_exact` (byte-exact body, Unicode, hostile) |
//! |                          | and `large_input_succeeds`                             |
//! | system_separation        | `request_is_exact` (separate system/user messages)     |
//! | not_installed            | `connection_refused_is_endpoint_unavailable`           |
//! | not_logged_in            | `status_errors_map_safely` (401/403 ->                 |
//! |                          | `AuthenticationRejected`; `NotLoggedIn` is CLI-only)   |
//! | timeout                  | `stalled_server_times_out`                             |
//! | invalid_output           | `malformed_responses_fail`                             |
//! | tool_activity            | `tool_calls_and_function_call_rejected`                |
//! | privacy                  | `errors_are_text_free`                                 |
//! | cleanup (sockets/tasks)  | `cancellation_drops_the_socket`                        |
//! | no fallback              | `failure_returns_raw_text_and_never_runs_another_`     |
//! |                          | `provider`                                             |
//! | disabled behavior        | `nothing_is_enabled_without_an_entry` (zero connections)|
//! | config protection        | `config_validation_*`, `recursion_*`                   |
//!
//! The HTTP-specific contract additions from the design (redirect with a
//! second listener receiving zero requests, address-spelling bypasses,
//! raw framing, overflow, compressed/SSE responses, recursion,
//! cancellation) are `redirect_is_not_followed`, `parse_endpoint_*`,
//! `raw_framing_*`, `oversized_*` and the tests above.

mod support;

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use pumice::config::{self, Config, DebugLogSettings, PromptSettings};
use pumice::pipeline::{OutcomeKind, Pipeline, RawReason};
use pumice::providers::generic::{
    DESCRIPTOR, EndpointError, GenericProvider, LoopbackEndpoint, parse_endpoint,
};
use pumice::providers::{
    FormatInput, Provider, ProviderError, ProviderErrorCode, ProviderLocations, UserPrompt,
};
use pumice::request::{ChatCompletionRequest, Content, ExtractedRequest, Message, extract_request};
use serde_json::{Value, json};
use support::adapter_contract::{AFTER, BEFORE, HOSTILE_TEXT, SYSTEM_PROMPT, UNICODE_TEXT};
use support::fake_http::{Behavior, FakeHttp, Reply};
use support::test_provider::{Step, TestProvider};
use tokio::time::Instant;

const MODEL: &str = "qwen2.5-7b";
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Builds the adapter pointed at `endpoint`.
fn build_provider(endpoint: &LoopbackEndpoint, timeout: Duration) -> Arc<dyn Provider> {
    Arc::new(GenericProvider::new(
        endpoint.clone(),
        MODEL.to_owned(),
        timeout,
    ))
}

async fn format_with(
    provider: &Arc<dyn Provider>,
    text: &str,
    deadline: Instant,
) -> Result<String, ProviderError> {
    provider.format(contract_input(text), deadline).await
}

fn contract_input(text: &str) -> FormatInput<'_> {
    FormatInput {
        system_prompt: SYSTEM_PROMPT,
        user_prompt: UserPrompt {
            before_text: BEFORE,
            after_text: AFTER,
        },
        text,
    }
}

/// The canonical success body for `text`, with protocol noise that must never
/// leak into the parsed result.
fn success_body(text: &str) -> Value {
    json!({
        "id": "chatcmpl-fake",
        "object": "chat.completion",
        "created": 1_700_000_000,
        "model": MODEL,
        "system_fingerprint": "fp-fake",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": text},
            "finish_reason": "stop",
            "logprobs": null
        }],
        "usage": {"prompt_tokens": 42, "completion_tokens": 7, "total_tokens": 49}
    })
}

fn endpoint_error(err: ProviderError) -> ProviderErrorCode {
    match err {
        ProviderError::Other { code } => code,
        other => panic!("expected ProviderError::Other, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Endpoint parsing
// ---------------------------------------------------------------------------

#[test]
fn parse_endpoint_accepts_exact_loopback_spellings() {
    for value in [
        "http://127.0.0.1:11434/v1",
        "http://127.0.0.1:11434/v1/",
        "http://127.0.0.1:1/v1",
        "http://127.0.0.1:65535/v1",
        "http://localhost:11434/v1",
        "http://localhost:11434/v1/",
        "http://[::1]:11434/v1",
        "http://[::1]:11434/v1/",
    ] {
        assert!(parse_endpoint(value).is_ok(), "value: {value}");
    }

    let endpoint = parse_endpoint("http://127.0.0.1:11434/v1").expect("parses");
    assert_eq!(endpoint.addr.to_string(), "127.0.0.1:11434");
    assert_eq!(endpoint.authority, "127.0.0.1:11434");
    assert_eq!(endpoint.chat_path, "/v1/chat/completions");

    // Literal `localhost` maps to IPv4 loopback without DNS.
    let mapped = parse_endpoint("http://localhost:11434/v1").expect("parses");
    assert_eq!(mapped.addr.to_string(), "127.0.0.1:11434");
    assert_eq!(mapped.authority, "127.0.0.1:11434");

    // IPv6 stays IPv6, bracketed in the authority.
    let v6 = parse_endpoint("http://[::1]:11434/v1").expect("parses");
    assert_eq!(v6.addr.to_string(), "[::1]:11434");
    assert_eq!(v6.authority, "[::1]:11434");
}

#[test]
fn parse_endpoint_rejects_everything_else() {
    let scheme = EndpointError::Scheme;
    let forbidden = EndpointError::ForbiddenCharacters;
    let host = EndpointError::Host;
    let missing = EndpointError::PortMissing;
    let invalid = EndpointError::PortInvalid;
    let path = EndpointError::Path;
    for (value, expected) in [
        // Schemes and non-URL input.
        ("https://127.0.0.1:11434/v1", scheme),
        ("http+unix://127.0.0.1:11434/v1", scheme),
        ("ftp://127.0.0.1:11434/v1", scheme),
        ("HTTP://127.0.0.1:11434/v1", scheme),
        ("", scheme),
        ("127.0.0.1:11434/v1", scheme),
        // Any other host, including alternate spellings of loopback.
        ("http://192.168.0.10:11434/v1", host),
        ("http://10.0.0.1:11434/v1", host),
        ("http://example.com:11434/v1", host),
        ("http://localhost.localdomain:11434/v1", host),
        ("http://LOCALHOST:11434/v1", host),
        ("http://127.1:11434/v1", host),
        ("http://127.0.0.01:11434/v1", host),
        ("http://0x7f.0.0.1:11434/v1", host),
        ("http://2130706433:11434/v1", host),
        ("http://0177.0.0.1:11434/v1", host),
        ("http://[0::1]:11434/v1", host),
        ("http://[::1%lo]:11434/v1", forbidden),
        ("http://::1:11434/v1", host),
        // Userinfo, percent-encoding, zone ids.
        ("http://user@127.0.0.1:11434/v1", forbidden),
        ("http://user:pass@localhost:11434/v1", forbidden),
        ("http://%31%32%37.0.0.1:11434/v1", forbidden),
        ("http://127.0.0.1%zone:11434/v1", forbidden),
        // Query, fragment, backslashes. (A backslash inside the scheme
        // separator fails the scheme check first.)
        ("http://127.0.0.1:11434/v1?api-version=1", forbidden),
        ("http://127.0.0.1:11434/v1#frag", forbidden),
        ("http:\\/\\/127.0.0.1:11434/v1", scheme),
        ("http://127.0.0.1:11434\\v1", forbidden),
        // Whitespace and control characters anywhere.
        (" http://127.0.0.1:11434/v1", forbidden),
        ("http://127.0.0.1:11434/v1 ", forbidden),
        ("http://127.0.0.1:11434/v1\t", forbidden),
        ("http://127.0.0.1:11434/v1\n", forbidden),
        ("http://127.0.0.1:11 434/v1", forbidden),
        // Missing or malformed ports.
        ("http://127.0.0.1/v1", missing),
        ("http://localhost/v1", missing),
        ("http://[::1]/v1", missing),
        ("http://127.0.0.1:/v1", invalid),
        ("http://127.0.0.1:0/v1", invalid),
        ("http://127.0.0.1:65536/v1", invalid),
        ("http://127.0.0.1:999999999999/v1", invalid),
        ("http://127.0.0.1:12a/v1", invalid),
        ("http://127.0.0.1:-1/v1", invalid),
        ("http://127.0.0.1:+80/v1", invalid),
        ("http://127.0.0.1:80:90/v1", invalid),
        ("http://[::1]x11434/v1", missing),
        ("http://[::1]:11434x/v1", invalid),
        // Paths other than exactly /v1.
        ("http://127.0.0.1:11434", path),
        ("http://127.0.0.1:11434/", path),
        ("http://127.0.0.1:11434/v2", path),
        ("http://127.0.0.1:11434/api/v1", path),
        ("http://127.0.0.1:11434/v1/extra", path),
        ("http://127.0.0.1:11434//v1", path),
        ("http://127.0.0.1:11434/v1//", path),
    ] {
        assert_eq!(
            parse_endpoint(value).unwrap_err(),
            expected,
            "value: {value}"
        );
    }
}

// ---------------------------------------------------------------------------
// The exact request, and success parsing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn request_is_exact() {
    let server = FakeHttp::spawn(Behavior::Reply(Reply::json(
        200,
        success_body("Texto formatado."),
    )))
    .await;
    let provider = build_provider(&server.endpoint(), CALL_TIMEOUT);
    let text = format!("{HOSTILE_TEXT} — {UNICODE_TEXT}");

    let result = format_with(&provider, &text, Instant::now() + CALL_TIMEOUT)
        .await
        .expect("formats");

    // Success: only the choice content comes back.
    assert_eq!(result, "Texto formatado.");

    let request = server.last_request().await;
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/v1/chat/completions");
    assert_eq!(
        request.header("host"),
        Some(server.addr().to_string().as_str())
    );
    assert_eq!(request.header("content-type"), Some("application/json"));
    assert_eq!(request.header("accept"), Some("application/json"));
    assert_eq!(request.header("accept-encoding"), Some("identity"));
    assert_eq!(request.header("connection"), Some("close"));
    // No authentication, tools or proxy headers.
    assert!(request.header("authorization").is_none());
    assert!(request.header("proxy-authorization").is_none());
    assert!(request.header("x-api-key").is_none());

    // Content-Length matches the serialized body exactly.
    let content_length: usize = request
        .header("content-length")
        .expect("content-length header")
        .parse()
        .expect("numeric content-length");
    assert_eq!(content_length, request.body.len());

    // The body: exactly model/stream/messages, system and user separate, the
    // dictation byte-exact inside the user message.
    let body: Value =
        serde_json::from_slice(&request.body).expect("the request body is valid JSON");
    assert_eq!(
        body.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["messages", "model", "stream"]
    );
    assert_eq!(body["model"], json!(MODEL));
    assert_eq!(body["stream"], json!(false));
    assert_eq!(
        body["messages"],
        json!([
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": format!("{BEFORE}{text}{AFTER}")},
        ])
    );
}

#[tokio::test]
async fn localhost_authority_maps_to_ipv4() {
    // `localhost` is validated and mapped without DNS; the Host header carries
    // the mapped authority.
    let server = FakeHttp::spawn(Behavior::Reply(Reply::json(200, success_body("ok")))).await;
    let endpoint = parse_endpoint(&format!("http://localhost:{}/v1", server.addr().port()))
        .expect("localhost endpoint parses");
    let provider = build_provider(&endpoint, CALL_TIMEOUT);

    format_with(&provider, "ditado", Instant::now() + CALL_TIMEOUT)
        .await
        .expect("formats");

    let request = server.last_request().await;
    assert_eq!(
        request.header("host"),
        Some(server.addr().to_string().as_str())
    );
}

#[tokio::test]
async fn ipv6_loopback_works() {
    let server = FakeHttp::spawn_on(
        SocketAddr::new(std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST), 0),
        Behavior::Reply(Reply::json(200, success_body("ok v6"))),
    )
    .await;
    let endpoint = parse_endpoint(&format!("http://[::1]:{}/v1", server.addr().port()))
        .expect("IPv6 endpoint parses");
    let provider = build_provider(&endpoint, CALL_TIMEOUT);

    let result = format_with(&provider, "ditado", Instant::now() + CALL_TIMEOUT)
        .await
        .expect("formats over IPv6 loopback");
    assert_eq!(result, "ok v6");

    let request = server.last_request().await;
    assert_eq!(
        request.header("host"),
        Some(format!("[::1]:{}", server.addr().port()).as_str())
    );
}

#[tokio::test]
async fn success_returns_content_unchanged() {
    // Leading whitespace and a list must survive untouched for pipeline
    // cleanup; metadata around the choice never leaks in.
    let text = "  Line one.\n\n- item\n";
    let server = FakeHttp::spawn(Behavior::Reply(Reply::json(200, success_body(text)))).await;
    let provider = build_provider(&server.endpoint(), CALL_TIMEOUT);

    assert_eq!(
        format_with(&provider, "ditado", Instant::now() + CALL_TIMEOUT)
            .await
            .expect("formats"),
        text
    );
}

#[tokio::test]
async fn large_input_succeeds() {
    let long = "uma frase ditada comprida com conteúdo ".repeat(6000);
    assert!(long.len() >= 200 * 1024, "fixture is at least 200 KiB");
    let server = FakeHttp::spawn(Behavior::Reply(Reply::json(
        200,
        success_body("Texto formatado."),
    )))
    .await;
    let provider = build_provider(&server.endpoint(), CALL_TIMEOUT);

    format_with(&provider, &long, Instant::now() + CALL_TIMEOUT)
        .await
        .expect("formats");

    let request = server.last_request().await;
    let body: Value = serde_json::from_slice(&request.body).expect("valid JSON");
    assert_eq!(
        body["messages"][1]["content"],
        json!(format!("{BEFORE}{long}{AFTER}"))
    );
}

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

#[tokio::test]
async fn status_errors_map_safely() {
    for (status, expected) in [
        (408u16, ProviderError::Timeout),
        (504, ProviderError::Timeout),
        (
            401,
            ProviderError::other(ProviderErrorCode::AuthenticationRejected),
        ),
        (
            403,
            ProviderError::other(ProviderErrorCode::AuthenticationRejected),
        ),
        (429, ProviderError::RateLimited { retry_after: None }),
        (
            500,
            ProviderError::other(ProviderErrorCode::HttpStatus(500)),
        ),
        (
            404,
            ProviderError::other(ProviderErrorCode::HttpStatus(404)),
        ),
        (
            400,
            ProviderError::other(ProviderErrorCode::HttpStatus(400)),
        ),
        (
            418,
            ProviderError::other(ProviderErrorCode::HttpStatus(418)),
        ),
    ] {
        let server = FakeHttp::spawn(Behavior::Reply(Reply::json(
            status,
            json!({"error": "diagnostic text that must never leak"}),
        )))
        .await;
        let provider = build_provider(&server.endpoint(), CALL_TIMEOUT);

        let err = format_with(&provider, "ditado", Instant::now() + CALL_TIMEOUT)
            .await
            .unwrap_err();
        assert_eq!(err, expected, "status {status}");
    }
}

#[tokio::test]
async fn error_response_bodies_are_bounded_and_never_copied() {
    // A 1 MiB error body still classifies by status, and its text never
    // reaches the error.
    let server = FakeHttp::spawn(Behavior::Reply(Reply {
        status: 500,
        headers: vec![("content-type".to_owned(), "application/json".to_owned())],
        body: vec![b'x'; 1024 * 1024],
    }))
    .await;
    let provider = build_provider(&server.endpoint(), CALL_TIMEOUT);

    let err = format_with(&provider, "ditado", Instant::now() + CALL_TIMEOUT)
        .await
        .unwrap_err();
    assert_eq!(
        err,
        ProviderError::other(ProviderErrorCode::HttpStatus(500))
    );
    assert!(!format!("{err:?}").contains('x'));
}

#[tokio::test]
async fn redirect_is_not_followed() {
    let second = FakeHttp::spawn(Behavior::Reply(Reply::json(
        200,
        success_body("must not be reached"),
    )))
    .await;
    let first = FakeHttp::spawn(Behavior::Reply(Reply {
        status: 307,
        headers: vec![("location".to_owned(), second.base_url())],
        body: Vec::new(),
    }))
    .await;
    let provider = build_provider(&first.endpoint(), CALL_TIMEOUT);

    let err = format_with(&provider, "ditado", Instant::now() + CALL_TIMEOUT)
        .await
        .unwrap_err();
    assert_eq!(
        err,
        ProviderError::other(ProviderErrorCode::HttpStatus(307))
    );
    assert_eq!(
        second.hit_count(),
        0,
        "the redirect target must receive zero requests"
    );
}

#[tokio::test]
async fn connection_refused_is_endpoint_unavailable() {
    // Bind and immediately release a port so nothing listens there.
    let probe = FakeHttp::spawn(Behavior::Stall).await;
    let dead = probe.addr();
    drop(probe);
    let endpoint = parse_endpoint(&format!("http://{dead}/v1")).expect("parses");
    let provider = build_provider(&endpoint, CALL_TIMEOUT);

    let err = format_with(&provider, "ditado", Instant::now() + CALL_TIMEOUT)
        .await
        .unwrap_err();
    assert_eq!(
        err,
        ProviderError::other(ProviderErrorCode::EndpointUnavailable)
    );
}

#[tokio::test]
async fn stalled_server_times_out() {
    let server = FakeHttp::spawn(Behavior::Stall).await;
    let provider = build_provider(&server.endpoint(), Duration::from_secs(1));

    let start = Instant::now();
    let err = format_with(&provider, "ditado", Instant::now() + CALL_TIMEOUT)
        .await
        .unwrap_err();
    assert_eq!(err, ProviderError::Timeout);
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "timeout took {:?}",
        start.elapsed()
    );
}

// ---------------------------------------------------------------------------
// Bounds and framing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn oversized_response_is_output_too_large() {
    let server = FakeHttp::spawn(Behavior::Reply(Reply {
        status: 200,
        headers: vec![("content-type".to_owned(), "application/json".to_owned())],
        body: vec![b'a'; 10 * 1024 * 1024 + 1],
    }))
    .await;
    let provider = build_provider(&server.endpoint(), CALL_TIMEOUT);

    let err = format_with(&provider, "ditado", Instant::now() + CALL_TIMEOUT)
        .await
        .unwrap_err();
    assert_eq!(err, ProviderError::other(ProviderErrorCode::OutputTooLarge));
}

#[tokio::test]
async fn oversized_request_is_input_too_large() {
    let server =
        FakeHttp::spawn(Behavior::Reply(Reply::json(200, success_body("unreached")))).await;
    let provider = build_provider(&server.endpoint(), CALL_TIMEOUT);
    let huge = "a".repeat(10 * 1024 * 1024 + 1);

    let err = format_with(&provider, &huge, Instant::now() + CALL_TIMEOUT)
        .await
        .unwrap_err();
    assert_eq!(err, ProviderError::other(ProviderErrorCode::InputTooLarge));
    assert_eq!(
        server.hit_count(),
        0,
        "an oversized request must never reach the endpoint"
    );
}

#[tokio::test]
async fn compressed_response_is_rejected() {
    for encoding in ["gzip", "deflate", "br"] {
        let server = FakeHttp::spawn(Behavior::Reply(Reply {
            status: 200,
            headers: vec![
                ("content-type".to_owned(), "application/json".to_owned()),
                ("content-encoding".to_owned(), encoding.to_owned()),
            ],
            body: vec![0x1f, 0x8b, 1, 2, 3],
        }))
        .await;
        let provider = build_provider(&server.endpoint(), CALL_TIMEOUT);

        let err = format_with(&provider, "ditado", Instant::now() + CALL_TIMEOUT)
            .await
            .unwrap_err();
        assert_eq!(
            err,
            ProviderError::other(ProviderErrorCode::InvalidOutput),
            "content-encoding: {encoding}"
        );
    }
}

#[tokio::test]
async fn identity_content_encoding_is_accepted() {
    let server = FakeHttp::spawn(Behavior::Reply(Reply {
        status: 200,
        headers: vec![
            ("content-type".to_owned(), "application/json".to_owned()),
            ("content-encoding".to_owned(), "identity".to_owned()),
        ],
        body: success_body("ok").to_string().into_bytes(),
    }))
    .await;
    let provider = build_provider(&server.endpoint(), CALL_TIMEOUT);

    assert_eq!(
        format_with(&provider, "ditado", Instant::now() + CALL_TIMEOUT)
            .await
            .expect("formats"),
        "ok"
    );
}

#[tokio::test]
async fn sse_response_is_rejected() {
    let server = FakeHttp::spawn(Behavior::Reply(Reply {
        status: 200,
        headers: vec![("content-type".to_owned(), "text/event-stream".to_owned())],
        body: b"data: {\"choices\":[]}\n\n".to_vec(),
    }))
    .await;
    let provider = build_provider(&server.endpoint(), CALL_TIMEOUT);

    let err = format_with(&provider, "ditado", Instant::now() + CALL_TIMEOUT)
        .await
        .unwrap_err();
    assert_eq!(err, ProviderError::other(ProviderErrorCode::InvalidOutput));
}

#[tokio::test]
async fn raw_framing_truncated_body_fails() {
    // Content-Length promises more than the server sends, then the socket
    // closes: the adapter counts received bytes, not promised ones.
    let mut head = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 100\r\nconnection: close\r\n\r\n".to_owned();
    head.push_str(r#"{"choices":[{"message":{"content":"partial"#);
    let server = FakeHttp::spawn(Behavior::Raw(head.into_bytes())).await;
    let provider = build_provider(&server.endpoint(), CALL_TIMEOUT);

    let err = format_with(&provider, "ditado", Instant::now() + CALL_TIMEOUT)
        .await
        .unwrap_err();
    assert_eq!(err, ProviderError::other(ProviderErrorCode::InvalidOutput));
}

#[tokio::test]
async fn raw_framing_garbage_response_fails() {
    let server = FakeHttp::spawn(Behavior::Raw(b"this is not HTTP\r\n\r\n".to_vec())).await;
    let provider = build_provider(&server.endpoint(), CALL_TIMEOUT);

    assert!(
        format_with(&provider, "ditado", Instant::now() + CALL_TIMEOUT)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn raw_framing_chunked_body_is_supported() {
    let body = r#"{"choices":[{"message":{"content":"chunked ok"},"finish_reason":"stop"}]}"#;
    let chunked = format!("{:x}\r\n{body}\r\n0\r\n\r\n", body.len());
    let head = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n{chunked}"
    );
    let server = FakeHttp::spawn(Behavior::Raw(head.into_bytes())).await;
    let provider = build_provider(&server.endpoint(), CALL_TIMEOUT);

    assert_eq!(
        format_with(&provider, "ditado", Instant::now() + CALL_TIMEOUT)
            .await
            .expect("chunked framing parses"),
        "chunked ok"
    );
}

// ---------------------------------------------------------------------------
// Body validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn malformed_responses_fail() {
    for (name, body) in [
        ("not json", "not json at all"),
        ("empty", ""),
        ("array", "[]"),
        ("object without choices", "{}"),
        ("choices not an array", r#"{"choices":"nope"}"#),
        ("no choices", r#"{"choices":[]}"#),
        (
            "two choices",
            r#"{"choices":[
                {"message":{"content":"a"},"finish_reason":"stop"},
                {"message":{"content":"b"},"finish_reason":"stop"}
            ]}"#,
        ),
        ("no message", r#"{"choices":[{}]}"#),
        (
            "null content",
            r#"{"choices":[{"message":{"content":null}}]}"#,
        ),
        (
            "numeric content",
            r#"{"choices":[{"message":{"content":42}}]}"#,
        ),
        (
            "array content",
            r#"{"choices":[{"message":{"content":[]}}]}"#,
        ),
        (
            "no content field",
            r#"{"choices":[{"message":{"role":"assistant"}}]}"#,
        ),
        (
            "truncated finish",
            r#"{"choices":[{"message":{"content":"half"},"finish_reason":"length"}]}"#,
        ),
        (
            "content filtered",
            r#"{"choices":[{"message":{"content":""},"finish_reason":"content_filter"}]}"#,
        ),
    ] {
        let server = FakeHttp::spawn(Behavior::Reply(Reply {
            status: 200,
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: body.as_bytes().to_vec(),
        }))
        .await;
        let provider = build_provider(&server.endpoint(), CALL_TIMEOUT);

        let err = format_with(&provider, "ditado", Instant::now() + CALL_TIMEOUT)
            .await
            .unwrap_err();
        assert_eq!(
            endpoint_error(err),
            ProviderErrorCode::InvalidOutput,
            "case: {name}"
        );
    }
}

#[tokio::test]
async fn tool_calls_and_function_call_rejected() {
    for (name, message) in [
        (
            "tool_calls",
            json!({"role": "assistant", "content": "", "tool_calls": [{
                "id": "call_1",
                "type": "function",
                "function": {"name": "shell", "arguments": "{}"}
            }]}),
        ),
        (
            "function_call",
            json!({"role": "assistant", "content": "", "function_call": {"name": "shell", "arguments": "{}"}}),
        ),
        (
            "finish_reason tool_calls",
            json!({"role": "assistant", "content": ""}),
        ),
    ] {
        let mut choice = json!({"index": 0, "message": message});
        if name == "finish_reason tool_calls" {
            choice["finish_reason"] = json!("tool_calls");
        } else {
            choice["finish_reason"] = json!("stop");
        }
        let server = FakeHttp::spawn(Behavior::Reply(Reply::json(
            200,
            json!({"choices": [choice]}),
        )))
        .await;
        let provider = build_provider(&server.endpoint(), CALL_TIMEOUT);

        let err = format_with(&provider, "ditado", Instant::now() + CALL_TIMEOUT)
            .await
            .unwrap_err();
        let expected = if name == "finish_reason tool_calls" {
            ProviderErrorCode::InvalidOutput
        } else {
            ProviderErrorCode::UnexpectedToolActivity
        };
        assert_eq!(endpoint_error(err), expected, "case: {name}");
    }

    // An empty tool_calls array is tolerated (it carries no call).
    let server = FakeHttp::spawn(Behavior::Reply(Reply::json(
        200,
        json!({"choices": [{"message": {"role": "assistant", "content": "clean", "tool_calls": []}, "finish_reason": "stop"}]}),
    )))
    .await;
    let provider = build_provider(&server.endpoint(), CALL_TIMEOUT);
    assert_eq!(
        format_with(&provider, "ditado", Instant::now() + CALL_TIMEOUT)
            .await
            .expect("empty tool_calls is not activity"),
        "clean"
    );
}

// ---------------------------------------------------------------------------
// Privacy and cancellation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn errors_are_text_free() {
    let marker = "SECRET-GENERIC-MARKER";

    // A 500: the marker rides in the request body and the (unread) error
    // response body.
    let server = FakeHttp::spawn(Behavior::Reply(Reply {
        status: 500,
        headers: vec![],
        body: format!("error body {marker}").into_bytes(),
    }))
    .await;
    let provider = build_provider(&server.endpoint(), CALL_TIMEOUT);
    let err = format_with(&provider, marker, Instant::now() + CALL_TIMEOUT)
        .await
        .unwrap_err();
    assert_error_is_text_free(&err, marker);

    // A malformed 200 body carrying the marker.
    let server = FakeHttp::spawn(Behavior::Reply(Reply {
        status: 200,
        headers: vec![("content-type".to_owned(), "application/json".to_owned())],
        body: format!("not json {marker}").into_bytes(),
    }))
    .await;
    let provider = build_provider(&server.endpoint(), CALL_TIMEOUT);
    let err = format_with(&provider, marker, Instant::now() + CALL_TIMEOUT)
        .await
        .unwrap_err();
    assert_eq!(endpoint_error(err), ProviderErrorCode::InvalidOutput);
    assert_error_is_text_free(&err, marker);

    // Tool calls whose arguments carry the marker.
    let server = FakeHttp::spawn(Behavior::Reply(Reply::json(
        200,
        json!({"choices": [{"message": {"role": "assistant", "content": "", "tool_calls": [{"id": "call_1", "type": "function", "function": {"name": "f", "arguments": format!("{{\"x\":\"{marker}\"}}")}}]}, "finish_reason": "stop"}]}),
    )))
    .await;
    let provider = build_provider(&server.endpoint(), CALL_TIMEOUT);
    let err = format_with(&provider, marker, Instant::now() + CALL_TIMEOUT)
        .await
        .unwrap_err();
    assert_eq!(
        endpoint_error(err),
        ProviderErrorCode::UnexpectedToolActivity
    );
    assert_error_is_text_free(&err, marker);
}

fn assert_error_is_text_free(err: &ProviderError, marker: &str) {
    assert!(!err.to_string().contains(marker), "Display leaks: {err}");
    assert!(!format!("{err:?}").contains(marker), "Debug leaks: {err:?}");
}

#[tokio::test]
async fn cancellation_drops_the_socket() {
    // The server waits for the client to close the connection; aborting the
    // call must close the socket (no detached driver task keeps it alive).
    let server = FakeHttp::spawn(Behavior::HangUp).await;
    let provider = build_provider(&server.endpoint(), CALL_TIMEOUT);

    let call = tokio::spawn({
        let provider = Arc::clone(&provider);
        async move {
            let _ = format_with(&provider, "ditado", Instant::now() + CALL_TIMEOUT).await;
        }
    });
    wait_until(|| server.hit_count() >= 1).await;
    call.abort();
    wait_until(|| server.ended_count() >= 1).await;
}

async fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(
            std::time::Instant::now() < deadline,
            "condition did not hold in time"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// ---------------------------------------------------------------------------
// No fallback: a failure returns the original text, never another provider
// ---------------------------------------------------------------------------

#[tokio::test]
async fn failure_returns_raw_text_and_never_runs_another_provider() {
    // Selected generic fails (connection refused): raw text, the double
    // never runs.
    let dead = FakeHttp::spawn(Behavior::Stall).await;
    let dead_addr = dead.addr();
    drop(dead);
    let generic_endpoint = parse_endpoint(&format!("http://{dead_addr}/v1")).expect("parses");
    let backup = TestProvider::new("backup", vec![Step::Ready("Olá de novo.".to_owned())]);
    let config = chain_config(vec![
        ("generic", generic_settings(&generic_endpoint)),
        ("backup", generic_settings(&generic_endpoint)),
    ]);
    let providers: Vec<Arc<dyn Provider>> = vec![
        build_provider(&generic_endpoint, CALL_TIMEOUT),
        backup.clone(),
    ];
    let pipeline = Pipeline::new(&config, providers);

    let outcome = pipeline
        .format(&handy_request("generic", "ditado"), Instant::now())
        .await;
    assert_eq!(
        outcome.kind,
        OutcomeKind::Raw(RawReason::ProviderFailed(ProviderError::other(
            ProviderErrorCode::EndpointUnavailable
        )))
    );
    assert_eq!(outcome.attempts, 1);
    assert_eq!(outcome.text, "ditado");
    assert_eq!(backup.calls(), 0, "a failure never tries another provider");

    // Selected double fails: the generic adapter never runs.
    let server = FakeHttp::spawn(Behavior::Reply(Reply::json(
        200,
        success_body("Texto formatado."),
    )))
    .await;
    let alpha = TestProvider::new("alpha", vec![Step::Fail(ProviderError::NotLoggedIn)]);
    let config = chain_config(vec![
        ("alpha", generic_settings(&server.endpoint())),
        ("generic", generic_settings(&server.endpoint())),
    ]);
    let providers: Vec<Arc<dyn Provider>> = vec![
        alpha.clone(),
        build_provider(&server.endpoint(), CALL_TIMEOUT),
    ];
    let pipeline = Pipeline::new(&config, providers);

    let outcome = pipeline
        .format(&handy_request("alpha", "ditado"), Instant::now())
        .await;
    assert_eq!(
        outcome.kind,
        OutcomeKind::Raw(RawReason::ProviderFailed(ProviderError::NotLoggedIn))
    );
    assert_eq!(outcome.attempts, 1);
    assert_eq!(outcome.text, "ditado");
    assert_eq!(alpha.calls(), 1);
}

fn generic_settings(endpoint: &LoopbackEndpoint) -> pumice::providers::ProviderSettings {
    let mut settings = (DESCRIPTOR.defaults)();
    settings.enabled = true;
    settings.model = MODEL.to_owned();
    settings.options.insert(
        "base_url".to_owned(),
        format!("http://{}/v1", endpoint.addr),
    );
    settings
}

fn chain_config(entries: Vec<(&str, pumice::providers::ProviderSettings)>) -> Config {
    Config {
        port: 7567,
        total_timeout: Duration::from_secs(30),
        prompts: PromptSettings::default(),
        debug_log: DebugLogSettings {
            enabled: false,
            path: Path::new("pumice-debug.jsonl").to_path_buf(),
        },
        providers: entries
            .into_iter()
            .map(|(id, settings)| pumice::config::ProviderConfig {
                id: id.to_owned(),
                settings,
            })
            .collect(),
    }
}

fn handy_request(model: &str, text: &str) -> ExtractedRequest {
    extract_request(ChatCompletionRequest {
        model: Some(model.to_owned()),
        messages: vec![Message {
            role: "user".to_owned(),
            content: Content::Text(format!("<transcript>\n{text}\n</transcript>")),
        }],
        stream: None,
    })
    .expect("request extracts")
}

// ---------------------------------------------------------------------------
// Disabled behavior and configuration validation
// ---------------------------------------------------------------------------

#[test]
fn nothing_is_enabled_without_an_entry() {
    let loaded = config::load_with_env(None, |_| None).expect("defaults load");
    assert!(
        loaded.config.provider("generic").is_none(),
        "providers are only configured by list entries"
    );
    assert!(loaded.config.providers.is_empty());

    // Building the default configuration yields no provider at all, so a
    // default-config service makes zero connections.
    let built = pumice::providers::build_from_config(
        &loaded.config,
        Arc::new(pumice::process::ProcessRunner::new()),
    )
    .expect("defaults build");
    assert!(built.is_empty(), "without entries nothing is built or run");

    // The descriptor still carries the risk warning for when it is enabled.
    let descriptor = pumice::providers::descriptor("generic").expect("registered");
    assert_eq!(
        descriptor.risk_warning,
        Some(pumice::providers::generic::RISK_WARNING)
    );
}

const CONFIG_NAME: &str = "pumice.yaml";

fn load_text(text: &str) -> Result<Config, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let path = dir.path().join(CONFIG_NAME);
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    config::load_with_env(Some(&path), |_| None)
        .map(|loaded| loaded.config)
        .map_err(|error| error.to_string())
}

fn load_error(text: &str) -> String {
    match load_text(text) {
        Ok(_) => panic!("expected a configuration error for:\n{text}"),
        Err(error) => error,
    }
}

/// Asserts the rendered error ends with `":line:column: message"` and names
/// the configuration file.
fn assert_config_error(text: &str, line: u64, column: u64, message: &str) {
    let error = load_error(text);
    assert!(
        error.contains(&format!(":{line}:{column}: {message}")),
        "expected ':{line}:{column}: {message}' in:\n{error}"
    );
    assert!(
        error.contains(CONFIG_NAME),
        "error names the file:\n{error}"
    );
}

/// A minimal valid enabled block; tests mutate one piece and assert the error.
#[test]
fn config_validation_requires_model_at_enabled_line() {
    assert_config_error(
        "providers:\n  - id: generic\n    enabled: true\n    options:\n      base_url: \"http://127.0.0.1:11434/v1\"\n",
        3,
        14,
        "providers.generic.model is required when the provider is enabled",
    );
}

#[test]
fn config_validation_requires_base_url_at_enabled_line() {
    assert_config_error(
        "providers:\n  - id: generic\n    enabled: true\n    model: qwen2\n",
        3,
        14,
        "providers.generic.options.base_url is required when the provider is enabled",
    );
}

#[test]
fn config_validation_rejects_bad_base_url_at_the_value() {
    assert_config_error(
        "providers:\n  - id: generic\n    enabled: true\n    model: qwen2\n    options:\n      base_url: \"https://example.com/v1\"\n",
        6,
        17,
        "providers.generic.options.base_url must be an http:// URL (https is not supported)",
    );
}

#[test]
fn config_validation_rejects_unknown_options_at_the_key() {
    assert_config_error(
        "providers:\n  - id: generic\n    enabled: true\n    model: qwen2\n    options:\n      base_url: \"http://127.0.0.1:11434/v1\"\n      api_key: \"sk-nope\"\n",
        7,
        7,
        "providers.generic.options.api_key is not supported",
    );
}

#[test]
fn config_validation_rejects_binary_and_env() {
    assert_config_error(
        "providers:\n  - id: generic\n    enabled: true\n    model: qwen2\n    binary: /usr/bin/curl\n    options:\n      base_url: \"http://127.0.0.1:11434/v1\"\n",
        5,
        13,
        "providers.generic spawns no CLI; binary must not be set",
    );
    assert_config_error(
        "providers:\n  - id: generic\n    enabled: true\n    model: qwen2\n    env:\n      NOPE: \"1\"\n    options:\n      base_url: \"http://127.0.0.1:11434/v1\"\n",
        6,
        7,
        "providers entry \"generic\".env.NOPE is not an allowed environment variable (no environment overrides are allowed for this provider)",
    );
}

#[test]
fn config_validation_accepts_a_valid_enabled_block() {
    let config = load_text(
        "providers:\n  - id: generic\n    enabled: true\n    model: qwen2\n    options:\n      base_url: \"http://127.0.0.1:11434/v1\"\n",
    )
    .expect("valid block loads");
    let generic = config.provider("generic").expect("configured");
    assert!(generic.enabled);
    assert_eq!(generic.model, "qwen2");
    assert_eq!(
        generic.options.get("base_url").map(String::as_str),
        Some("http://127.0.0.1:11434/v1")
    );
}

#[test]
fn recursion_rejects_pumices_own_port() {
    // The IPv4 loopback spellings naming Pumice's port are refused at the
    // base_url value...
    assert_config_error(
        "port: 7567\nproviders:\n  - id: generic\n    enabled: true\n    model: qwen2\n    options:\n      base_url: \"http://127.0.0.1:7567/v1\"\n",
        7,
        17,
        "providers.generic.options.base_url points at Pumice's own port (7567), which would route requests back into this service",
    );
    assert_config_error(
        "port: 7567\nproviders:\n  - id: generic\n    enabled: true\n    model: qwen2\n    options:\n      base_url: \"http://localhost:7567/v1\"\n",
        7,
        17,
        "providers.generic.options.base_url points at Pumice's own port (7567), which would route requests back into this service",
    );
    // ...including when the port is the built-in default (no explicit line).
    assert_config_error(
        "providers:\n  - id: generic\n    enabled: true\n    model: qwen2\n    options:\n      base_url: \"http://127.0.0.1:7567/v1\"\n",
        6,
        17,
        "providers.generic.options.base_url points at Pumice's own port (7567), which would route requests back into this service",
    );
}

#[test]
fn recursion_allows_other_ports_and_ipv6_loopback() {
    // A different IPv4 port is fine.
    load_text(
        "providers:\n  - id: generic\n    enabled: true\n    model: qwen2\n    options:\n      base_url: \"http://127.0.0.1:7568/v1\"\n",
    )
    .expect("another port loads");
    // Pumice binds 127.0.0.1 only, so its own port on [::1] cannot recurse.
    load_text(
        "port: 7567\nproviders:\n  - id: generic\n    enabled: true\n    model: qwen2\n    options:\n      base_url: \"http://[::1]:7567/v1\"\n",
    )
    .expect("IPv6 loopback on Pumice's port loads");
}

#[test]
fn full_settings_validation_rejects_direct_construction() {
    // Defense in depth for library callers that bypass the loader: the
    // descriptor's validator sees binary and env, which the loader would
    // already have rejected.
    let mut settings = (DESCRIPTOR.defaults)();
    settings.enabled = true;
    settings.model = "qwen2".to_owned();
    settings.binary = Some(std::path::PathBuf::from("/bin/true"));
    settings.options.insert(
        "base_url".to_owned(),
        "http://127.0.0.1:11434/v1".to_owned(),
    );
    let locations = ProviderLocations::default();
    let error = (DESCRIPTOR.validate_settings)(&settings, &locations)
        .expect_err("a binary on an HTTP-only provider is refused")
        .to_string();
    assert!(
        error.contains("binary must not be set"),
        "unexpected error: {error}"
    );

    let mut settings = (DESCRIPTOR.defaults)();
    settings.enabled = true;
    settings.model = "qwen2".to_owned();
    settings.env.insert("NOPE".to_owned(), "1".to_owned());
    settings.options.insert(
        "base_url".to_owned(),
        "http://127.0.0.1:11434/v1".to_owned(),
    );
    let error = (DESCRIPTOR.validate_settings)(&settings, &locations)
        .expect_err("env overrides are refused")
        .to_string();
    assert!(
        error.contains("accepts no environment overrides"),
        "unexpected error: {error}"
    );
}
