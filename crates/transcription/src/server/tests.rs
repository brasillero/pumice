//! Router-level tests: every request goes through `router(state)`, the same
//! stack `serve` runs, with fake engines instead of a model.

use super::*;
use crate::{EngineError, Transcript};
use std::io::Cursor;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use tokio::sync::{Notify, mpsc};
use tower::ServiceExt;

const MODEL: &str = "turbo";
const BOUNDARY: &str = "pumice-test-boundary";

/// A fake engine that records its options, can block until released (while
/// ignoring cancellation, like a slow native run), and can panic.
#[derive(Default)]
struct TestEngine {
    text: String,
    language: Option<String>,
    delay: Duration,
    gated: bool,
    panic: bool,
    started: Notify,
    release: AtomicBool,
    saw_cancel: AtomicBool,
    finished: AtomicBool,
    options: Mutex<Option<TranscriptionOptions>>,
}

impl TestEngine {
    fn returning(text: &str) -> Arc<Self> {
        Arc::new(Self {
            text: text.to_string(),
            ..Default::default()
        })
    }

    fn gated(text: &str) -> Arc<Self> {
        Arc::new(Self {
            text: text.to_string(),
            gated: true,
            ..Default::default()
        })
    }

    fn panicking() -> Arc<Self> {
        Arc::new(Self {
            panic: true,
            ..Default::default()
        })
    }

    fn release(&self) {
        self.release.store(true, Ordering::SeqCst);
    }

    fn recorded(&self) -> TranscriptionOptions {
        self.options
            .lock()
            .unwrap()
            .clone()
            .expect("engine was called")
    }
}

impl Engine for TestEngine {
    fn transcribe(
        &self,
        _pcm: &[f32],
        options: &TranscriptionOptions,
        cancel: &CancelToken,
    ) -> Result<Transcript, EngineError> {
        *self.options.lock().unwrap() = Some(options.clone());
        self.started.notify_one();
        if self.panic {
            panic!("test engine panic");
        }
        std::thread::sleep(self.delay);
        if self.gated {
            while !self.release.load(Ordering::SeqCst) {
                if cancel.is_cancelled() {
                    self.saw_cancel.store(true, Ordering::SeqCst);
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        self.finished.store(true, Ordering::SeqCst);
        if cancel.is_cancelled() {
            self.saw_cancel.store(true, Ordering::SeqCst);
            return Err(EngineError::Cancelled);
        }
        Ok(Transcript {
            text: self.text.clone(),
            language: self.language.clone(),
        })
    }
}

fn state_with(engine: Arc<TestEngine>, limits: Limits) -> ServerState {
    ServerState::new(engine, MODEL, None, limits, CancellationToken::new())
}

fn state(engine: Arc<TestEngine>) -> ServerState {
    state_with(engine, Limits::default())
}

fn wav(seconds: f64) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::new(&mut buf, spec).unwrap();
    for _ in 0..(seconds * 16_000.0) as usize {
        writer.write_sample(0i16).unwrap();
    }
    writer.finalize().unwrap();
    buf.into_inner()
}

enum Part<'a> {
    Text(&'a str, &'a str),
    File(&'a str, &'a [u8]),
}

fn multipart(parts: &[Part]) -> Vec<u8> {
    let mut body = Vec::new();
    for part in parts {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        match part {
            Part::Text(name, value) => {
                body.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}")
                        .as_bytes(),
                );
            }
            Part::File(name, bytes) => {
                body.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{name}\"; filename=\"clip.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
                    )
                    .as_bytes(),
                );
                body.extend_from_slice(bytes);
            }
        }
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    body
}

fn post(body: impl Into<Body>) -> Request {
    Request::builder()
        .method("POST")
        .uri("/v1/audio/transcriptions")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(body.into())
        .unwrap()
}

fn basic_body(audio: &[u8]) -> Vec<u8> {
    multipart(&[Part::File("file", audio), Part::Text("model", MODEL)])
}

async fn send(state: &ServerState, request: Request) -> Response {
    router(state.clone()).oneshot(request).await.unwrap()
}

async fn body_bytes(response: Response) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

async fn json(response: Response) -> serde_json::Value {
    serde_json::from_slice(&body_bytes(response).await).unwrap()
}

/// Assert an OpenAI error envelope and return the error object.
async fn error(response: Response, status: StatusCode) -> serde_json::Value {
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let value = json(response).await;
    let error = value["error"].clone();
    for key in ["message", "type", "param", "code"] {
        assert!(error.get(key).is_some(), "missing `{key}` in {error}");
    }
    error
}

async fn wait_until(mut condition: impl FnMut() -> bool) {
    for _ in 0..2_000 {
        if condition() {
            return;
        }
        time::sleep(Duration::from_millis(2)).await;
    }
    panic!("condition not reached");
}

/// A body that delivers its chunks and then stays open until dropped.
struct HangingBody(mpsc::UnboundedReceiver<Bytes>);

impl futures_core::Stream for HangingBody {
    type Item = Result<Bytes, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0.poll_recv(cx).map(|chunk| chunk.map(Ok))
    }
}

fn hanging_body(first: &[u8]) -> (Body, mpsc::UnboundedSender<Bytes>) {
    let (tx, rx) = mpsc::unbounded_channel();
    tx.send(Bytes::copy_from_slice(first)).unwrap();
    (Body::from_stream(HangingBody(rx)), tx)
}

// --- Success paths ---------------------------------------------------------

#[tokio::test]
async fn json_response_returns_exact_text_and_forwards_options() {
    let engine = TestEngine::returning(" Olá, açúcar e pão.\n");
    let state = state(Arc::clone(&engine));
    let body = multipart(&[
        Part::Text("model", MODEL),
        Part::Text("language", "PT"),
        Part::Text("prompt", "Pumice, Handy"),
        Part::Text("temperature", "0.2"),
        Part::Text("stream", "false"),
        Part::File("file", &wav(1.0)),
    ]);
    let response = send(&state, post(body)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    assert_eq!(
        json(response).await,
        serde_json::json!({ "text": " Olá, açúcar e pão.\n" })
    );

    let options = engine.recorded();
    assert_eq!(options.language.as_deref(), Some("pt"));
    assert_eq!(options.prompt.as_deref(), Some("Pumice, Handy"));
    assert_eq!(options.temperature, Some(0.2));
}

#[tokio::test]
async fn text_response_is_the_exact_transcript() {
    let state = state(TestEngine::returning("linha um\nlinha dois"));
    let body = multipart(&[
        Part::File("file", &wav(1.0)),
        Part::Text("model", MODEL),
        Part::Text("response_format", "text"),
    ]);
    let response = send(&state, post(body)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/plain; charset=utf-8"
    );
    assert_eq!(body_bytes(response).await, b"linha um\nlinha dois");
}

#[tokio::test]
async fn silence_returns_empty_text() {
    let state = state(TestEngine::returning(""));
    let response = send(&state, post(basic_body(&wav(1.0)))).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json(response).await, serde_json::json!({ "text": "" }));
}

#[tokio::test]
async fn default_language_applies_when_the_request_has_none() {
    let engine = TestEngine::returning("x");
    let state = ServerState::new(
        engine.clone(),
        MODEL,
        Some("pt".into()),
        Limits::default(),
        CancellationToken::new(),
    );
    let response = send(&state, post(basic_body(&wav(1.0)))).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(engine.recorded().language.as_deref(), Some("pt"));
}

// --- Models, health and routing -------------------------------------------

#[tokio::test]
async fn models_list_and_retrieve_match_openai_shapes() {
    let state = state(TestEngine::returning("x"));
    let get = |uri: &str| Request::get(uri).body(Body::empty()).unwrap();

    let list = json(send(&state, get("/v1/models")).await).await;
    assert_eq!(list["object"], "list");
    assert_eq!(
        list["data"],
        serde_json::json!([{ "id": MODEL, "object": "model", "created": 0, "owned_by": "pumice" }])
    );

    let one = send(&state, get("/v1/models/turbo")).await;
    assert_eq!(one.status(), StatusCode::OK);
    assert_eq!(json(one).await, list["data"][0]);

    let missing = error(
        send(&state, get("/v1/models/other")).await,
        StatusCode::NOT_FOUND,
    )
    .await;
    assert_eq!(missing["code"], "model_not_found");

    // A path segment that is not UTF-8 still gets a JSON error.
    let invalid = error(
        send(&state, get("/v1/models/%FF")).await,
        StatusCode::NOT_FOUND,
    )
    .await;
    assert_eq!(invalid["code"], "model_not_found");
}

#[tokio::test]
async fn unknown_routes_and_methods_get_json_errors() {
    let state = state(TestEngine::returning("x"));
    let missing = send(
        &state,
        Request::get("/v1/nope").body(Body::empty()).unwrap(),
    )
    .await;
    let missing = error(missing, StatusCode::NOT_FOUND).await;
    assert_eq!(missing["type"], "invalid_request_error");
    assert_eq!(missing["code"], serde_json::Value::Null);

    for (method, uri) in [("GET", "/v1/audio/transcriptions"), ("POST", "/v1/models")] {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        error(send(&state, request).await, StatusCode::METHOD_NOT_ALLOWED).await;
    }
}

// --- Validation ------------------------------------------------------------

async fn expect_invalid(body: Vec<u8>, status: StatusCode, param: Option<&str>) {
    let engine = TestEngine::returning("x");
    let state = state(Arc::clone(&engine));
    let error = error(send(&state, post(body)).await, status).await;
    assert_eq!(error["param"], serde_json::json!(param));
    assert!(
        engine.options.lock().unwrap().is_none(),
        "engine must not run"
    );
}

#[tokio::test]
async fn missing_and_wrong_model_are_rejected() {
    expect_invalid(
        multipart(&[Part::File("file", &wav(1.0))]),
        StatusCode::BAD_REQUEST,
        Some("model"),
    )
    .await;
    expect_invalid(
        multipart(&[
            Part::File("file", &wav(1.0)),
            Part::Text("model", "whisper-1"),
        ]),
        StatusCode::NOT_FOUND,
        Some("model"),
    )
    .await;
}

#[tokio::test]
async fn invalid_parameters_are_rejected_with_their_param() {
    let audio = wav(1.0);
    let with = |name, value| {
        multipart(&[
            Part::File("file", &audio),
            Part::Text("model", MODEL),
            Part::Text(name, value),
        ])
    };
    expect_invalid(
        with("response_format", "srt"),
        StatusCode::BAD_REQUEST,
        Some("response_format"),
    )
    .await;
    expect_invalid(
        with("temperature", "2"),
        StatusCode::BAD_REQUEST,
        Some("temperature"),
    )
    .await;
    expect_invalid(
        with("temperature", "NaN"),
        StatusCode::BAD_REQUEST,
        Some("temperature"),
    )
    .await;
    expect_invalid(
        with("language", "portuguese"),
        StatusCode::BAD_REQUEST,
        Some("language"),
    )
    .await;
    expect_invalid(
        with("stream", "true"),
        StatusCode::BAD_REQUEST,
        Some("stream"),
    )
    .await;
    expect_invalid(
        with("timestamp_granularities[]", "word"),
        StatusCode::BAD_REQUEST,
        None,
    )
    .await;
    expect_invalid(with("model", MODEL), StatusCode::BAD_REQUEST, Some("model")).await;
    expect_invalid(
        with("prompt", "before\0after"),
        StatusCode::BAD_REQUEST,
        Some("prompt"),
    )
    .await;
    let mut bad_utf8 = multipart(&[Part::File("file", &audio), Part::Text("model", MODEL)]);
    let tail = format!("--{BOUNDARY}--\r\n");
    bad_utf8.truncate(bad_utf8.len() - tail.len());
    bad_utf8.extend_from_slice(
        format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"prompt\"\r\n\r\n")
            .as_bytes(),
    );
    bad_utf8.extend_from_slice(&[0xff, 0xfe]);
    bad_utf8.extend_from_slice(format!("\r\n{tail}").as_bytes());
    expect_invalid(bad_utf8, StatusCode::BAD_REQUEST, Some("prompt")).await;
}

#[tokio::test]
async fn file_must_be_a_non_empty_upload_and_text_fields_must_not_be() {
    let audio = wav(1.0);
    expect_invalid(
        multipart(&[Part::Text("model", MODEL)]),
        StatusCode::BAD_REQUEST,
        Some("file"),
    )
    .await;
    expect_invalid(
        multipart(&[Part::File("file", b""), Part::Text("model", MODEL)]),
        StatusCode::BAD_REQUEST,
        Some("file"),
    )
    .await;
    // WAV bytes sent as a plain text field.
    let mut as_text = Vec::new();
    as_text.extend_from_slice(
        format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"\r\n\r\n").as_bytes(),
    );
    as_text.extend_from_slice(&audio);
    as_text.extend_from_slice(format!("\r\n--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n{MODEL}\r\n--{BOUNDARY}--\r\n").as_bytes());
    expect_invalid(as_text, StatusCode::BAD_REQUEST, Some("file")).await;
    expect_invalid(
        multipart(&[
            Part::File("file", &audio),
            Part::File("model", MODEL.as_bytes()),
        ]),
        StatusCode::BAD_REQUEST,
        Some("model"),
    )
    .await;
}

#[tokio::test]
async fn audio_errors_map_to_fixed_messages() {
    expect_invalid(
        basic_body(b"definitely not audio"),
        StatusCode::BAD_REQUEST,
        Some("file"),
    )
    .await;
    expect_invalid(
        basic_body(&wav(121.0)),
        StatusCode::BAD_REQUEST,
        Some("file"),
    )
    .await;
}

#[tokio::test]
async fn unsupported_format_message_reflects_enabled_features() {
    let engine = TestEngine::returning("x");
    let state = state(Arc::clone(&engine));
    let error = error(
        send(&state, post(basic_body(b"definitely not audio"))).await,
        StatusCode::BAD_REQUEST,
    )
    .await;
    let message = error["message"].as_str().unwrap();
    #[cfg(feature = "opus")]
    assert!(
        message.contains("ogg (Opus)") && message.contains("WebM (Opus)"),
        "message: {message}"
    );
    #[cfg(not(feature = "opus"))]
    assert!(!message.contains("Opus"), "message: {message}");
}

#[cfg(feature = "opus")]
#[tokio::test]
async fn webm_opus_upload_transcribes() {
    let state = state(TestEngine::returning("transcribed"));
    let webm = std::fs::read(format!(
        "{}/tests/fixtures/sample-pt.webm",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let response = send(&state, post(basic_body(&webm))).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json(response).await,
        serde_json::json!({ "text": "transcribed" })
    );
}

#[cfg(feature = "opus")]
#[tokio::test]
async fn ogg_opus_upload_transcribes() {
    let state = state(TestEngine::returning("transcribed"));
    let ogg = std::fs::read(format!(
        "{}/tests/fixtures/sample-pt-48k.opus",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let response = send(&state, post(basic_body(&ogg))).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json(response).await,
        serde_json::json!({ "text": "transcribed" })
    );
}

#[cfg(not(feature = "opus"))]
#[tokio::test]
async fn webm_upload_is_rejected_without_the_opus_feature() {
    let state = state(TestEngine::returning("transcribed"));
    let webm = std::fs::read(format!(
        "{}/tests/fixtures/sample-pt.webm",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let response = send(&state, post(basic_body(&webm))).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn non_multipart_requests_are_rejected() {
    let state = state(TestEngine::returning("x"));
    let request = Request::post("/v1/audio/transcriptions")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
        .unwrap();
    error(send(&state, request).await, StatusCode::BAD_REQUEST).await;
}

// --- Size limits -------------------------------------------------------------

#[tokio::test]
async fn declared_oversize_body_is_rejected_without_reading() {
    let state = state(TestEngine::returning("x"));
    let (body, _tx) = hanging_body(b"");
    let mut request = post(body);
    request
        .headers_mut()
        .insert(header::CONTENT_LENGTH, "26000001".parse().unwrap());
    error(send(&state, request).await, StatusCode::PAYLOAD_TOO_LARGE).await;
}

#[tokio::test]
async fn bytes_after_the_closing_boundary_count_toward_the_limit() {
    let mut body = basic_body(&wav(1.0));
    body.resize(MAX_TOTAL_BODY_BYTES as usize + 1, b'x');
    expect_invalid(body, StatusCode::PAYLOAD_TOO_LARGE, None).await;
}

#[tokio::test]
async fn oversize_file_and_text_fields_are_rejected() {
    let big = vec![0u8; MAX_FILE_BYTES as usize + 1];
    expect_invalid(
        basic_body(&big),
        StatusCode::PAYLOAD_TOO_LARGE,
        Some("file"),
    )
    .await;
    let long_prompt = "a".repeat(MAX_TEXT_FIELD_BYTES as usize + 1);
    expect_invalid(
        multipart(&[
            Part::File("file", &wav(1.0)),
            Part::Text("model", MODEL),
            Part::Text("prompt", &long_prompt),
        ]),
        StatusCode::PAYLOAD_TOO_LARGE,
        Some("prompt"),
    )
    .await;
}

// --- Privacy -----------------------------------------------------------------

#[tokio::test]
async fn client_input_is_never_echoed() {
    let sentinel = "SECRET-dictation/path\nlog-line";
    let audio = wav(1.0);
    let bodies = [
        multipart(&[Part::File("file", &audio), Part::Text("model", sentinel)]),
        multipart(&[
            Part::File("file", &audio),
            Part::Text("model", MODEL),
            Part::Text("SECRET-dictation", "x"),
        ]),
        multipart(&[
            Part::File("file", &audio),
            Part::Text("model", MODEL),
            Part::Text("language", sentinel),
        ]),
    ];
    for body in bodies {
        let state = state(TestEngine::returning("x"));
        let response = send(&state, post(body)).await;
        assert_ne!(response.status(), StatusCode::OK);
        let text = String::from_utf8(body_bytes(response).await).unwrap();
        assert!(!text.contains("SECRET"), "echoed client input: {text}");
    }
}

// --- Admission, cancellation, deadlines, shutdown ---------------------------

#[tokio::test]
async fn busy_server_answers_429_without_reading_and_stays_healthy() {
    let engine = TestEngine::gated("first");
    let state = state(Arc::clone(&engine));
    let first = tokio::spawn(send_owned(state.clone(), post(basic_body(&wav(1.0)))));
    engine.started.notified().await;

    // The second body never finishes: a 429 proves it was not read.
    let (body, _tx) = hanging_body(b"--");
    let busy = send(&state, post(body)).await;
    assert_eq!(busy.headers()[header::RETRY_AFTER], "1");
    let busy = error(busy, StatusCode::TOO_MANY_REQUESTS).await;
    assert_eq!(busy["type"], "rate_limit_error");
    assert_eq!(busy["code"], "rate_limit_exceeded");

    let health = send(&state, Request::get("/health").body(Body::empty()).unwrap()).await;
    assert_eq!(health.status(), StatusCode::OK);

    engine.release();
    let first = first.await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
}

async fn send_owned(state: ServerState, request: Request) -> Response {
    send(&state, request).await
}

#[tokio::test]
async fn disconnect_cancels_but_keeps_the_slot_until_the_engine_returns() {
    let engine = TestEngine::gated("late");
    let state = state(Arc::clone(&engine));
    let request = tokio::spawn(send_owned(state.clone(), post(basic_body(&wav(1.0)))));
    engine.started.notified().await;

    request.abort(); // the client went away
    wait_until(|| engine.saw_cancel.load(Ordering::SeqCst)).await;

    let still_busy = send(&state, post(basic_body(&wav(1.0)))).await;
    assert_eq!(still_busy.status(), StatusCode::TOO_MANY_REQUESTS);

    engine.release();
    wait_until(|| state.admission.available_permits() == 1).await;
}

#[tokio::test]
async fn total_deadline_answers_504_on_time_and_keeps_the_slot() {
    let engine = TestEngine::gated("late");
    let limits = Limits {
        upload_deadline: Duration::from_secs(5),
        total_deadline: Duration::from_millis(200),
    };
    let state = state_with(Arc::clone(&engine), limits);
    let started = Instant::now();
    let response = send(&state, post(basic_body(&wav(1.0)))).await;
    error(response, StatusCode::GATEWAY_TIMEOUT).await;
    assert!(started.elapsed() < Duration::from_secs(2));

    wait_until(|| engine.saw_cancel.load(Ordering::SeqCst)).await;
    assert_eq!(state.admission.available_permits(), 0);
    engine.release();
    wait_until(|| state.admission.available_permits() == 1).await;
}

#[tokio::test]
async fn slow_upload_answers_408() {
    let limits = Limits {
        upload_deadline: Duration::from_millis(100),
        total_deadline: Duration::from_secs(5),
    };
    let state = state_with(TestEngine::returning("x"), limits);
    let (body, _tx) = hanging_body(b"--");
    error(send(&state, post(body)).await, StatusCode::REQUEST_TIMEOUT).await;
    assert_eq!(state.admission.available_permits(), 1);
}

#[tokio::test]
async fn shutdown_during_upload_answers_503() {
    let state = state(TestEngine::returning("x"));
    let (body, _tx) = hanging_body(b"--");
    let request = tokio::spawn(send_owned(state.clone(), post(body)));
    time::sleep(Duration::from_millis(20)).await;
    state.shutdown.cancel();
    let response = time::timeout(Duration::from_secs(2), request)
        .await
        .expect("answered promptly")
        .unwrap();
    error(response, StatusCode::SERVICE_UNAVAILABLE).await;
}

#[tokio::test]
async fn shutdown_during_transcription_cancels_and_discards_the_result() {
    let engine = TestEngine::gated("must not be published");
    let state = state(Arc::clone(&engine));
    let request = tokio::spawn(send_owned(state.clone(), post(basic_body(&wav(1.0)))));
    engine.started.notified().await;

    state.shutdown.cancel();
    let response = request.await.unwrap();
    let text =
        String::from_utf8(body_bytes_status(response, StatusCode::SERVICE_UNAVAILABLE).await)
            .unwrap();
    assert!(!text.contains("published"));

    wait_until(|| engine.saw_cancel.load(Ordering::SeqCst)).await;
    engine.release();
    wait_until(|| state.admission.available_permits() == 1).await;
    assert!(engine.finished.load(Ordering::SeqCst));
}

async fn body_bytes_status(response: Response, status: StatusCode) -> Vec<u8> {
    assert_eq!(response.status(), status);
    body_bytes(response).await
}

#[tokio::test]
async fn requests_after_shutdown_get_503() {
    let state = state(TestEngine::returning("x"));
    state.shutdown.cancel();
    error(
        send(&state, post(basic_body(&wav(1.0)))).await,
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
}

#[tokio::test]
async fn engine_panic_answers_500_and_frees_the_slot() {
    let state = state(TestEngine::panicking());
    let response = send(&state, post(basic_body(&wav(1.0)))).await;
    let error = error(response, StatusCode::INTERNAL_SERVER_ERROR).await;
    assert_eq!(error["type"], "server_error");
    assert_eq!(state.admission.available_permits(), 1);
}

#[tokio::test]
async fn serve_binds_loopback_and_drains_on_shutdown() {
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let handle = serve(
        0,
        TestEngine::returning("x"),
        MODEL.to_string(),
        None,
        async {
            let _ = rx.await;
        },
    )
    .await
    .unwrap();
    tx.send(()).unwrap();
    time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("server stopped")
        .unwrap();
}

#[tokio::test]
async fn total_deadline_counts_from_arrival_including_the_upload() {
    // The upload stalls for half the total budget. Counted from the
    // arrival, the deadline comes about 1 s after the upload ends (less if
    // the stall overran); counted from the end of the upload, at least 2 s
    // after it. The 1.5 s bound between them leaves 0.5 s of slack on each
    // side, and the stall ends 1 s before the upload limit would apply.
    let engine = TestEngine::gated("late");
    let limits = Limits {
        upload_deadline: Duration::from_secs(5),
        total_deadline: Duration::from_millis(2_000),
    };
    let state = state_with(Arc::clone(&engine), limits);
    let body = basic_body(&wav(1.0));
    let (stream, tx) = hanging_body(&body[..10]);
    let request = tokio::spawn(send_owned(state.clone(), post(stream)));
    time::sleep(Duration::from_millis(1_000)).await;
    tx.send(Bytes::copy_from_slice(&body[10..])).unwrap();
    drop(tx);
    let uploaded = Instant::now();

    let response = request.await.unwrap();
    error(response, StatusCode::GATEWAY_TIMEOUT).await;
    assert!(
        uploaded.elapsed() < Duration::from_millis(1_500),
        "the deadline counted from the end of the upload: {:?}",
        uploaded.elapsed()
    );
    engine.release();
    wait_until(|| state.admission.available_permits() == 1).await;
}

#[test]
fn a_run_that_finishes_after_cancellation_is_discarded() {
    /// Cancellation arrives while the run is in progress and the run still
    /// returns text, as a native run past its last cancellation check would.
    struct CancelledMidRun;
    impl Engine for CancelledMidRun {
        fn transcribe(
            &self,
            _pcm: &[f32],
            _options: &TranscriptionOptions,
            cancel: &CancelToken,
        ) -> Result<Transcript, EngineError> {
            cancel.cancel();
            Ok(Transcript {
                text: "must not be published".into(),
                language: None,
            })
        }
    }

    let result = transcribe_pcm_from_reader(
        Cursor::new(wav(1.0)),
        &CancelledMidRun,
        &TranscriptionOptions::default(),
        &CancelToken::new(),
    );
    assert_eq!(result.err().map(|e| e.code), Some(ErrorCode::Cancelled));
}

// --- Managed mode -----------------------------------------------------------

const PORT: u16 = 40123;
const HOST: &str = "127.0.0.1:40123";

fn managed(state: ServerState) -> ServerState {
    state.managed("gen-1".to_string(), PORT)
}

fn with_host(mut request: Request) -> Request {
    request
        .headers_mut()
        .insert(header::HOST, header::HeaderValue::from_static(HOST));
    request
}

fn get(uri: &str) -> Request {
    Request::get(uri).body(Body::empty()).unwrap()
}

fn post_detailed(body: impl Into<Body>) -> Request {
    let mut request = post(body);
    *request.uri_mut() = "/pumice/v1/transcriptions".parse().unwrap();
    with_host(request)
}

/// Status, the headers a client acts on, and the exact body.
async fn snapshot(response: Response) -> (StatusCode, Vec<(String, String)>, Vec<u8>) {
    let status = response.status();
    let mut headers: Vec<_> = response
        .headers()
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_str().unwrap().to_string()))
        .collect();
    headers.sort();
    (status, headers, body_bytes(response).await)
}

/// A managed-route error: an OpenAI error object whose `code` is always set.
async fn managed_error(response: Response, status: StatusCode, code: &str) -> serde_json::Value {
    let error = error(response, status).await;
    assert_eq!(error["code"], code, "{error}");
    error
}

#[tokio::test]
async fn managed_routes_do_not_exist_without_managed_mode() {
    let state = state(TestEngine::returning("x"));
    let unknown = snapshot(send(&state, get("/no-such-route")).await).await;
    for request in [
        with_host(get("/ready")),
        post_detailed(basic_body(&wav(1.0))),
    ] {
        assert_eq!(snapshot(send(&state, request).await).await, unknown);
    }
}

#[tokio::test]
async fn public_routes_answer_byte_identically_in_managed_mode() {
    let requests: Vec<fn() -> Request> = vec![
        || post(basic_body(&wav(1.0))),
        || {
            post(multipart(&[
                Part::File("file", &wav(1.0)),
                Part::Text("model", MODEL),
                Part::Text("response_format", "text"),
            ]))
        },
        || post(multipart(&[Part::File("file", &wav(1.0))])),
        || {
            post(multipart(&[
                Part::File("file", b"not audio"),
                Part::Text("model", MODEL),
            ]))
        },
        || {
            let mut request = post(basic_body(&wav(1.0)));
            // Managed-route guards never apply to the OpenAI routes.
            request.headers_mut().insert(
                header::ORIGIN,
                header::HeaderValue::from_static("http://example.com"),
            );
            request
        },
        || get("/v1/models"),
        || get("/v1/models/turbo"),
        || get("/v1/models/other"),
        || get("/health"),
        || get("/unknown"),
        || Request::delete("/health").body(Body::empty()).unwrap(),
        || {
            Request::put("/v1/audio/transcriptions")
                .header(header::ORIGIN, "http://example.com")
                .body(Body::empty())
                .unwrap()
        },
    ];
    for make in requests {
        let plain = state(TestEngine::returning("same words"));
        let managed = managed(state(TestEngine::returning("same words")));
        assert_eq!(
            snapshot(send(&plain, make()).await).await,
            snapshot(send(&managed, make()).await).await
        );
    }

    // A busy answer too.
    let mut busy = Vec::new();
    for is_managed in [false, true] {
        let engine = TestEngine::gated("x");
        let mut state = state(Arc::clone(&engine));
        if is_managed {
            state = managed(state);
        }
        let first = tokio::spawn(send_owned(state.clone(), post(basic_body(&wav(1.0)))));
        engine.started.notified().await;
        busy.push(snapshot(send(&state, post(basic_body(&wav(1.0)))).await).await);
        engine.release();
        first.await.unwrap();
    }
    assert_eq!(busy[0], busy[1]);
}

#[tokio::test]
async fn ready_reports_the_instance_until_shutdown() {
    let engine = TestEngine::gated("x");
    let state = managed(state(Arc::clone(&engine)));
    let response = send(&state, with_host(get("/ready"))).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        String::from_utf8(body_bytes(response).await).unwrap(),
        format!(
            r#"{{"schema_version":1,"module":"transcription","instance_id":"gen-1","protocol_version":1,"version":"{}","model":"turbo","capabilities":["openai_transcriptions","detailed_transcriptions","managed_shutdown"],"state":"ready"}}"#,
            env!("CARGO_PKG_VERSION")
        )
    );

    // Busy is still ready.
    let first = tokio::spawn(send_owned(state.clone(), post(basic_body(&wav(1.0)))));
    engine.started.notified().await;
    let response = send(&state, with_host(get("/ready"))).await;
    assert_eq!(response.status(), StatusCode::OK);

    state.shutdown.cancel();
    let response = send(&state, with_host(get("/ready"))).await;
    let error = managed_error(response, StatusCode::SERVICE_UNAVAILABLE, "shutting_down").await;
    assert_eq!(error["type"], "server_error");
    engine.release();
    first.await.unwrap();
}

#[tokio::test]
async fn detailed_route_returns_the_versioned_result() {
    let engine = Arc::new(TestEngine {
        text: "olá mundo".to_string(),
        delay: Duration::from_millis(60),
        ..Default::default()
    });
    let state = managed(state(engine));
    let response = send(
        &state,
        post_detailed(multipart(&[
            Part::File("file", &wav(1.5)),
            Part::Text("model", MODEL),
            Part::Text("language", "pt"),
        ])),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let body = json(response).await;
    let keys: Vec<_> = body.as_object().unwrap().keys().cloned().collect();
    assert_eq!(
        keys,
        [
            "schema_version",
            "text",
            "language",
            "audio_seconds",
            "inference_ms"
        ]
    );
    assert_eq!(body["schema_version"], 1);
    assert_eq!(body["text"], "olá mundo");
    assert_eq!(body["language"], "pt");
    assert_eq!(body["audio_seconds"], 1.5);
    let inference_ms = body["inference_ms"].as_u64().unwrap();
    assert!((60..10_000).contains(&inference_ms), "{inference_ms}");
}

#[tokio::test]
async fn detailed_route_reports_detected_language_and_omits_unknown_values() {
    let detected = Arc::new(TestEngine {
        text: "hello".to_string(),
        language: Some("en".to_string()),
        ..Default::default()
    });
    let body = json(
        send(
            &managed(state(detected)),
            post_detailed(basic_body(&wav(1.0))),
        )
        .await,
    )
    .await;
    assert_eq!(body["language"], "en");

    // Silence: empty text is a success, and an unknown language is left out.
    let body = json(
        send(
            &managed(state(TestEngine::returning(""))),
            post_detailed(basic_body(&wav(1.0))),
        )
        .await,
    )
    .await;
    assert_eq!(body["text"], "");
    assert!(body.get("language").is_none(), "{body}");
}

#[tokio::test]
async fn detailed_route_errors_carry_stable_codes() {
    let state = managed(state(TestEngine::returning("x")));
    let cases: Vec<(Vec<u8>, StatusCode, &str)> = vec![
        (
            multipart(&[Part::File("file", &wav(1.0))]),
            StatusCode::BAD_REQUEST,
            "missing_model",
        ),
        (
            multipart(&[Part::File("file", &wav(1.0)), Part::Text("model", "other")]),
            StatusCode::NOT_FOUND,
            "model_not_found",
        ),
        (
            multipart(&[
                Part::File("file", &wav(1.0)),
                Part::Text("model", MODEL),
                Part::Text("response_format", "text"),
            ]),
            StatusCode::BAD_REQUEST,
            "invalid_response_format",
        ),
        (
            multipart(&[
                Part::File("file", &wav(1.0)),
                Part::Text("model", MODEL),
                Part::Text("flow", "clean"),
            ]),
            StatusCode::BAD_REQUEST,
            "unsupported_parameter",
        ),
        (
            multipart(&[Part::File("file", b"not audio"), Part::Text("model", MODEL)]),
            StatusCode::BAD_REQUEST,
            "unsupported_format",
        ),
        (
            multipart(&[Part::File("file", &wav(121.0)), Part::Text("model", MODEL)]),
            StatusCode::BAD_REQUEST,
            "audio_too_long",
        ),
        (
            multipart(&[
                Part::File("file", &wav(1.0)),
                Part::Text("model", MODEL),
                Part::Text("prompt", &"p".repeat(16 * 1024 + 1)),
            ]),
            StatusCode::PAYLOAD_TOO_LARGE,
            "field_too_large",
        ),
    ];
    for (body, status, code) in cases {
        managed_error(send(&state, post_detailed(body)).await, status, code).await;
    }

    let mut oversize = post_detailed(Body::empty());
    oversize.headers_mut().insert(
        header::CONTENT_LENGTH,
        header::HeaderValue::from(MAX_TOTAL_BODY_BYTES + 1),
    );
    managed_error(
        send(&state, oversize).await,
        StatusCode::PAYLOAD_TOO_LARGE,
        "body_too_large",
    )
    .await;
    assert_eq!(state.admission.available_permits(), 1);
}

#[tokio::test]
async fn detailed_and_openai_routes_share_one_admission() {
    let engine = TestEngine::gated("first");
    let state = managed(state(Arc::clone(&engine)));
    let first = tokio::spawn(send_owned(
        state.clone(),
        post_detailed(basic_body(&wav(1.0))),
    ));
    engine.started.notified().await;

    // Neither route reads the body while busy.
    let (body, _tx) = hanging_body(b"--");
    let busy = send(&state, post_detailed(body)).await;
    assert_eq!(busy.headers()[header::RETRY_AFTER], "1");
    let busy = managed_error(busy, StatusCode::TOO_MANY_REQUESTS, "rate_limit_exceeded").await;
    assert_eq!(busy["type"], "rate_limit_error");
    let (body, _tx) = hanging_body(b"--");
    error(
        send(&state, post(body)).await,
        StatusCode::TOO_MANY_REQUESTS,
    )
    .await;

    engine.release();
    assert_eq!(first.await.unwrap().status(), StatusCode::OK);

    // And the other way round.
    let engine = TestEngine::gated("first");
    let state = managed(state_with(Arc::clone(&engine), Limits::default()));
    let first = tokio::spawn(send_owned(state.clone(), post(basic_body(&wav(1.0)))));
    engine.started.notified().await;
    let (body, _tx) = hanging_body(b"--");
    managed_error(
        send(&state, post_detailed(body)).await,
        StatusCode::TOO_MANY_REQUESTS,
        "rate_limit_exceeded",
    )
    .await;
    engine.release();
    assert_eq!(first.await.unwrap().status(), StatusCode::OK);
}

#[tokio::test]
async fn detailed_route_cancels_on_shutdown_like_the_openai_route() {
    let engine = TestEngine::gated("must not be published");
    let state = managed(state(Arc::clone(&engine)));
    let request = tokio::spawn(send_owned(
        state.clone(),
        post_detailed(basic_body(&wav(1.0))),
    ));
    engine.started.notified().await;

    state.shutdown.cancel();
    let response = request.await.unwrap();
    managed_error(response, StatusCode::SERVICE_UNAVAILABLE, "shutting_down").await;
    wait_until(|| engine.saw_cancel.load(Ordering::SeqCst)).await;
    assert_eq!(state.admission.available_permits(), 0);
    engine.release();
    wait_until(|| state.admission.available_permits() == 1).await;
}

#[tokio::test]
async fn managed_routes_accept_only_the_literal_loopback_host_and_no_origin() {
    let engine = Arc::new(TestEngine {
        text: "x".to_string(),
        ..Default::default()
    });
    let state = managed(state(Arc::clone(&engine)));
    let bad_hosts = [
        None,
        Some("127.0.0.1"),
        Some("127.0.0.1:1"),
        Some("localhost:40123"),
        Some("attacker.example:40123"),
        Some("[::1]:40123"),
    ];
    for host in bad_hosts {
        for path in ["/ready", "/pumice/v1/transcriptions"] {
            let mut request = if path == "/ready" {
                get(path)
            } else {
                post_detailed(basic_body(&wav(1.0)))
            };
            request.headers_mut().remove(header::HOST);
            if let Some(host) = host {
                request
                    .headers_mut()
                    .insert(header::HOST, header::HeaderValue::from_static(host));
            }
            let error = managed_error(
                send(&state, request).await,
                StatusCode::FORBIDDEN,
                "host_not_allowed",
            )
            .await;
            assert_eq!(error["type"], "invalid_request_error");
        }
    }

    // An absolute URI must name the same authority.
    let mut request = with_host(get("/ready"));
    *request.uri_mut() = "http://attacker.example:40123/ready".parse().unwrap();
    managed_error(
        send(&state, request).await,
        StatusCode::FORBIDDEN,
        "host_not_allowed",
    )
    .await;

    // Any Origin is refused, even this server's own, before admission.
    for origin in ["http://127.0.0.1:40123", "null", "http://localhost:40123"] {
        let mut request = post_detailed(basic_body(&wav(1.0)));
        request
            .headers_mut()
            .insert(header::ORIGIN, header::HeaderValue::from_static(origin));
        managed_error(
            send(&state, request).await,
            StatusCode::FORBIDDEN,
            "origin_not_allowed",
        )
        .await;
        let mut request = with_host(get("/ready"));
        request
            .headers_mut()
            .insert(header::ORIGIN, header::HeaderValue::from_static(origin));
        managed_error(
            send(&state, request).await,
            StatusCode::FORBIDDEN,
            "origin_not_allowed",
        )
        .await;
    }
    assert!(
        engine.options.lock().unwrap().is_none(),
        "the engine never ran"
    );
    assert_eq!(state.admission.available_permits(), 1);
}

#[tokio::test]
async fn unsupported_methods_on_managed_routes_meet_the_guards_first() {
    let engine = Arc::new(TestEngine {
        text: "x".to_string(),
        ..Default::default()
    });
    let state = managed(state(Arc::clone(&engine)));
    let request = |method: &str, path: &str| {
        Request::builder()
            .method(method)
            .uri(path)
            .body(Body::from(basic_body(&wav(1.0))))
            .unwrap()
    };
    for (method, path, allow) in [
        ("PUT", "/pumice/v1/transcriptions", "POST"),
        ("GET", "/pumice/v1/transcriptions", "POST"),
        ("POST", "/ready", "GET,HEAD"),
        ("DELETE", "/ready", "GET,HEAD"),
    ] {
        let response = send(&state, with_host(request(method, path))).await;
        assert_eq!(response.headers()[header::ALLOW], allow, "{method} {path}");
        let error = managed_error(
            response,
            StatusCode::METHOD_NOT_ALLOWED,
            "method_not_allowed",
        )
        .await;
        assert_eq!(error["type"], "invalid_request_error");

        let mut hostile = request(method, path);
        hostile.headers_mut().insert(
            header::HOST,
            header::HeaderValue::from_static("attacker.example:40123"),
        );
        managed_error(
            send(&state, hostile).await,
            StatusCode::FORBIDDEN,
            "host_not_allowed",
        )
        .await;

        let mut cross_origin = with_host(request(method, path));
        cross_origin.headers_mut().insert(
            header::ORIGIN,
            header::HeaderValue::from_static("http://attacker.example"),
        );
        managed_error(
            send(&state, cross_origin).await,
            StatusCode::FORBIDDEN,
            "origin_not_allowed",
        )
        .await;
    }
    assert!(
        engine.options.lock().unwrap().is_none(),
        "the engine never ran"
    );
    assert_eq!(state.admission.available_permits(), 1);
}

/// Send a raw HTTP/1.1 request over a real socket without waiting for the
/// answer.
async fn open_request(addr: std::net::SocketAddr, body: &[u8]) -> tokio::net::TcpStream {
    use tokio::io::AsyncWriteExt;
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let head = format!(
        "POST /v1/audio/transcriptions HTTP/1.1\r\nHost: {addr}\r\nContent-Type: multipart/form-data; boundary={BOUNDARY}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.write_all(body).await.unwrap();
    stream
}

#[tokio::test]
async fn serve_waits_for_an_active_worker_before_returning() {
    let engine = TestEngine::gated("late");
    let listener = bind(0).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let handle = serve_on(
        listener,
        Arc::clone(&engine) as Arc<dyn Engine>,
        MODEL.to_string(),
        None,
        None,
        async {
            let _ = rx.await;
        },
    )
    .unwrap();

    let _client = open_request(addr, &basic_body(&wav(1.0))).await;
    time::timeout(Duration::from_secs(5), engine.started.notified())
        .await
        .expect("the engine started");
    tx.send(()).unwrap();
    wait_until(|| engine.saw_cancel.load(Ordering::SeqCst)).await;
    time::sleep(Duration::from_millis(100)).await;
    assert!(!handle.is_finished(), "returned while the engine still ran");

    engine.release();
    time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("server stopped after the engine returned")
        .unwrap();
    assert!(engine.finished.load(Ordering::SeqCst));
}

#[tokio::test]
async fn serve_on_refuses_a_non_loopback_listener() {
    let Ok(listener) = TcpListener::bind("0.0.0.0:0").await else {
        // Past libtest's output capture, so the skip shows in CI logs.
        use std::io::Write as _;
        let _ = writeln!(
            std::io::stderr(),
            "skipped serve_on_refuses_a_non_loopback_listener: \
             no IPv4 wildcard listener in this environment"
        );
        return;
    };
    let result = serve_on(
        listener,
        TestEngine::returning("x"),
        MODEL.to_string(),
        None,
        None,
        std::future::pending(),
    );
    assert_eq!(
        result.err().map(|e| e.kind()),
        Some(io::ErrorKind::InvalidInput)
    );
}
