//! OpenAI-compatible HTTP server for `pumice-transcribe serve`.
//!
//! Serves on IPv4 loopback only, with one model loaded at startup and one
//! transcription admitted at a time. Dictated text never reaches ordinary logs;
//! only request metadata (id, sizes, timings, status, error code) is logged.

use crate::audio::MAX_FILE_BYTES;
use crate::config::Config;
use crate::engine::Engine;
use crate::service::transcribe_pcm_from_reader;
use crate::{Error, ErrorCode, TranscriptionOptions};
use axum::{
    Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use bytes::Bytes;
use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, oneshot};
use tokio::time;
use tokio_util::sync::CancellationToken;

const MAX_TOTAL_BODY_BYTES: usize = 26_000_000;
const MAX_MULTIPART_PARTS: usize = 32;
const MAX_TEXT_FIELD_BYTES: usize = 16 * 1024;
const UPLOAD_DEADLINE: Duration = Duration::from_secs(15);
const TOTAL_DEADLINE: Duration = Duration::from_secs(120);
const BUSY_RESPONSE_RETRY_AFTER: &str = "0";

/// Shared server state.
#[derive(Clone)]
pub struct ServerState {
    engine: Arc<dyn Engine>,
    served_model_id: String,
    config: Arc<Config>,
    admission: Arc<Semaphore>,
    shutdown: CancellationToken,
    request_ids: Arc<std::sync::atomic::AtomicU64>,
}

impl ServerState {
    fn next_id(&self) -> u64 {
        self.request_ids
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }
}

/// Start the HTTP server on `127.0.0.1:port` and serve until `shutdown_signal`
/// fires. Returns the local address actually bound (useful when port is 0).
pub async fn serve(
    port: u16,
    engine: Arc<dyn Engine>,
    served_model_id: String,
    config: Config,
    shutdown_signal: impl Future<Output = ()> + Send + 'static,
) -> io::Result<tokio::task::JoinHandle<()>> {
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).await?;
    let local_addr = listener.local_addr()?;
    eprintln!(
        "pumice-transcribe listening on http://{}/v1 (model: {})",
        local_addr, served_model_id
    );

    let shutdown = CancellationToken::new();
    let state = ServerState {
        engine,
        served_model_id,
        config: Arc::new(config),
        admission: Arc::new(Semaphore::new(1)),
        shutdown: shutdown.clone(),
        request_ids: Arc::new(std::sync::atomic::AtomicU64::new(1)),
    };

    let app = router(state);
    let shutdown_for_serve = shutdown.clone();
    let handle = tokio::spawn(async move {
        let serve = axum::serve(listener, app).with_graceful_shutdown(async move {
            shutdown_signal.await;
            shutdown_for_serve.cancel();
        });
        if let Err(e) = serve.await {
            eprintln!("server error: {e}");
        }
    });

    Ok(handle)
}

fn router(state: ServerState) -> Router {
    Router::new()
        .route("/v1/audio/transcriptions", post(transcriptions))
        .route("/v1/models", get(list_models))
        .route("/health", get(health))
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(MAX_TOTAL_BODY_BYTES))
        .with_state(state)
}

/// POST /v1/audio/transcriptions
async fn transcriptions(State(state): State<ServerState>, request: Request) -> Response {
    let id = state.next_id();
    let started = Instant::now();

    // Admission first: if we are busy, reject immediately without reading the body.
    let Ok(permit) = state.admission.clone().try_acquire_owned() else {
        log_request(id, started, StatusCode::TOO_MANY_REQUESTS, "busy", None);
        return too_many_requests();
    };

    // If shutdown is already in progress, release the permit and refuse new work.
    if state.shutdown.is_cancelled() {
        drop(permit);
        log_request(
            id,
            started,
            StatusCode::SERVICE_UNAVAILABLE,
            "shutting down",
            None,
        );
        return service_unavailable("the server is shutting down");
    }

    let request_token = state.shutdown.child_token();
    let engine_cancel = crate::CancelToken::new();

    // Bridge the request lifecycle token into the engine's CancelToken. This
    // covers shutdown, total deadline, and client disconnect (via the watcher
    // below).
    let bridge_token = request_token.clone();
    let bridge_cancel = engine_cancel.clone();
    let _bridge = tokio::spawn(async move {
        bridge_token.cancelled().await;
        bridge_cancel.cancel();
    });

    // Spawn a watcher so client disconnect cancels the request token.
    let (disconnect_tx, disconnect_rx) = oneshot::channel::<()>();
    let cancel_for_disconnect = request_token.clone();
    let disconnect_watcher = tokio::spawn(async move {
        let _ = disconnect_rx.await;
        cancel_for_disconnect.cancel();
    });

    // Extract boundary before consuming the body.
    let content_type = match request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    {
        Some(ct) => ct,
        None => {
            abort_watcher(disconnect_watcher);
            drop(permit);
            log_request(
                id,
                started,
                StatusCode::BAD_REQUEST,
                "missing content-type",
                None,
            );
            return bad_request("Content-Type must be multipart/form-data with a boundary");
        }
    };
    let boundary = match multer::parse_boundary(content_type) {
        Ok(boundary) => boundary,
        Err(_) => {
            abort_watcher(disconnect_watcher);
            drop(permit);
            log_request(
                id,
                started,
                StatusCode::BAD_REQUEST,
                "invalid content-type",
                None,
            );
            return bad_request("Content-Type must be multipart/form-data with a boundary");
        }
    };

    let body = request.into_body();

    // Read and validate the multipart body within the upload deadline.
    let parsed = match time::timeout(
        UPLOAD_DEADLINE,
        parse_multipart(body.into_data_stream(), boundary),
    )
    .await
    {
        Ok(Ok(parsed)) => parsed,
        Ok(Err(ParseError::TooLarge)) => {
            abort_watcher(disconnect_watcher);
            drop(permit);
            log_request(
                id,
                started,
                StatusCode::PAYLOAD_TOO_LARGE,
                "body too large",
                None,
            );
            return payload_too_large();
        }
        Ok(Err(ParseError::Malformed(message))) => {
            abort_watcher(disconnect_watcher);
            drop(permit);
            log_request(id, started, StatusCode::BAD_REQUEST, &message, None);
            return bad_request(&message);
        }
        Err(_) => {
            abort_watcher(disconnect_watcher);
            drop(permit);
            log_request(
                id,
                started,
                StatusCode::REQUEST_TIMEOUT,
                "upload deadline exceeded",
                None,
            );
            return request_timeout();
        }
    };

    // Validate request fields.
    if let Err(message) = validate_model(&state.served_model_id, &parsed) {
        abort_watcher(disconnect_watcher);
        drop(permit);
        log_request(id, started, StatusCode::BAD_REQUEST, &message, None);
        return bad_request(&message);
    }

    let language = match parsed.language {
        Some(lang) => match crate::config::validate_language(&lang) {
            Ok(normalized) => Some(normalized),
            Err(_) => {
                abort_watcher(disconnect_watcher);
                drop(permit);
                log_request(
                    id,
                    started,
                    StatusCode::BAD_REQUEST,
                    "invalid language",
                    None,
                );
                return bad_request("language must be a supported ISO-639-1 code");
            }
        },
        None => state.config.language.clone(),
    };
    let options = TranscriptionOptions {
        language,
        prompt: parsed.prompt,
        temperature: parsed.temperature,
    };

    // Run decode+inference on a blocking worker, enforcing the total deadline.
    let engine = Arc::clone(&state.engine);
    let mut blocking = tokio::task::spawn_blocking(move || {
        let reader = io::Cursor::new(parsed.file_bytes);
        let result = transcribe_pcm_from_reader(reader, engine.as_ref(), &options, &engine_cancel);
        (result, permit)
    });

    let response = tokio::select! {
        biased;
        () = request_token.cancelled() => {
            // Cancellation fired before the worker finished. Wait for it so the
            // permit is released and any native cleanup completes.
            let result = blocking.await;
            match result {
                Ok((result, _permit)) => match result {
                    Ok(result) => success_response(parsed.response_format, result.transcript.text),
                    Err(_) => service_unavailable("the request was cancelled"),
                },
                Err(_) => service_unavailable("the request was cancelled"),
            }
        }
        _ = time::sleep(TOTAL_DEADLINE) => {
            request_token.cancel();
            let _result = blocking.await;
            log_request(id, started, StatusCode::GATEWAY_TIMEOUT, "processing deadline exceeded", None);
            gateway_timeout()
        }
        worker_result = &mut blocking => {
            let result = match worker_result {
                Ok((result, _permit)) => result,
                Err(_) => Err(Error::internal("transcription worker panicked")),
            };
            match result {
                Ok(result) => {
                    log_request(id, started, StatusCode::OK, "ok", Some(result.audio_seconds));
                    success_response(parsed.response_format, result.transcript.text)
                }
                Err(error) => {
                    let (status, message) = map_error(&error);
                    log_request(id, started, status, &message, None);
                    openai_error(status, &message)
                }
            }
        }
    };

    abort_watcher(disconnect_watcher);
    // Keep disconnect_tx alive until after the response is built so a late
    // disconnect does not fire and cancel a subsequent request's token.
    let _ = disconnect_tx;
    response
}

fn abort_watcher(watcher: tokio::task::JoinHandle<()>) {
    watcher.abort();
}

/// GET /v1/models: list only the served model id.
async fn list_models(State(state): State<ServerState>) -> Response {
    let body = serde_json::json!({
        "object": "list",
        "data": [
            {
                "id": state.served_model_id,
                "object": "model",
                "created": 0,
                "owned_by": "pumice",
            }
        ]
    });
    json_response(StatusCode::OK, body)
}

/// GET /health: lightweight liveness; stays responsive during transcription.
async fn health() -> Response {
    let body = serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    });
    json_response(StatusCode::OK, body)
}

async fn not_found() -> Response {
    openai_error(
        StatusCode::NOT_FOUND,
        "the requested resource was not found",
    )
}

fn success_response(format: ResponseFormat, text: String) -> Response {
    match format {
        ResponseFormat::Text => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            text,
        )
            .into_response(),
        ResponseFormat::Json => {
            let body = serde_json::json!({ "text": text });
            json_response(StatusCode::OK, body)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ResponseFormat {
    #[default]
    Json,
    Text,
}

struct ParsedRequest {
    file_bytes: Vec<u8>,
    model: Option<String>,
    language: Option<String>,
    response_format: ResponseFormat,
    prompt: Option<String>,
    temperature: Option<f32>,
}

#[derive(Debug)]
enum ParseError {
    TooLarge,
    Malformed(String),
}

async fn parse_multipart<S, E>(stream: S, boundary: String) -> Result<ParsedRequest, ParseError>
where
    S: futures_core::Stream<Item = Result<Bytes, E>> + Send + Unpin + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    let size_limit = multer::SizeLimit::new()
        .whole_stream(MAX_TOTAL_BODY_BYTES as u64)
        .per_field(MAX_TEXT_FIELD_BYTES as u64)
        .for_field("file", MAX_FILE_BYTES);
    let constraints = multer::Constraints::new()
        .allowed_fields(vec![
            "file".to_string(),
            "model".into(),
            "language".into(),
            "response_format".into(),
            "prompt".into(),
            "temperature".into(),
            "stream".into(),
        ])
        .size_limit(size_limit);
    let mut multipart = multer::Multipart::with_constraints(stream, boundary, constraints);

    let mut file_bytes: Option<Vec<u8>> = None;
    let mut language: Option<String> = None;
    let mut response_format = ResponseFormat::Json;
    let mut prompt: Option<String> = None;
    let mut temperature: Option<f32> = None;
    let mut model: Option<String> = None;
    let mut seen_fields: HashMap<String, usize> = HashMap::new();
    let mut part_count: usize = 0;

    while let Some(field) = multipart.next_field().await.map_err(map_multer_error)? {
        part_count += 1;
        if part_count > MAX_MULTIPART_PARTS {
            return Err(ParseError::Malformed(
                "too many multipart parts".to_string(),
            ));
        }
        let name = field.name().unwrap_or("").to_string();
        *seen_fields.entry(name.clone()).or_insert(0) += 1;

        match name.as_str() {
            "file" => {
                if file_bytes.is_some() {
                    return Err(ParseError::Malformed(
                        "only one file field is allowed".to_string(),
                    ));
                }
                let data = field.bytes().await.map_err(map_multer_error)?;
                if data.is_empty() {
                    return Err(ParseError::Malformed(
                        "the uploaded file must not be empty".to_string(),
                    ));
                }
                file_bytes = Some(data.to_vec());
            }
            "language" => {
                let value = read_text_field(field).await?;
                language = Some(value);
            }
            "model" => {
                model = Some(read_text_field(field).await?);
            }
            "response_format" => {
                let value = read_text_field(field).await?;
                response_format = parse_response_format(&value)?;
            }
            "prompt" => {
                prompt = Some(read_text_field(field).await?);
            }
            "temperature" => {
                let value = read_text_field(field).await?;
                temperature = Some(parse_temperature(&value)?);
            }
            "stream" => {
                let value = read_text_field(field).await?;
                if value != "false" {
                    return Err(ParseError::Malformed(
                        "streaming is not supported".to_string(),
                    ));
                }
            }
            _ => {
                return Err(ParseError::Malformed(format!(
                    "unknown or unsupported field `{name}`"
                )));
            }
        }
    }

    if seen_fields.values().any(|&c| c > 1) {
        return Err(ParseError::Malformed(
            "duplicate fields are not allowed".to_string(),
        ));
    }

    let file_bytes = file_bytes
        .ok_or_else(|| ParseError::Malformed("missing required field `file`".to_string()))?;

    Ok(ParsedRequest {
        file_bytes,
        model,
        language,
        response_format,
        prompt,
        temperature,
    })
}

fn map_multer_error(e: multer::Error) -> ParseError {
    match e {
        multer::Error::FieldSizeExceeded { .. } | multer::Error::StreamSizeExceeded { .. } => {
            ParseError::TooLarge
        }
        _ => ParseError::Malformed(format!("multipart error: {e}")),
    }
}

async fn read_text_field(field: multer::Field<'_>) -> Result<String, ParseError> {
    let data = field.bytes().await.map_err(map_multer_error)?;
    String::from_utf8(data.to_vec())
        .map_err(|_| ParseError::Malformed("text field is not valid UTF-8".to_string()))
}

fn parse_response_format(value: &str) -> Result<ResponseFormat, ParseError> {
    match value {
        "json" => Ok(ResponseFormat::Json),
        "text" => Ok(ResponseFormat::Text),
        _ => Err(ParseError::Malformed(
            "response_format must be `json` or `text`".to_string(),
        )),
    }
}

fn parse_temperature(value: &str) -> Result<f32, ParseError> {
    let value: f32 = value
        .parse()
        .map_err(|_| ParseError::Malformed("temperature must be a number".to_string()))?;
    if !value.is_finite() || value < 0.0 || value > 1.0 {
        return Err(ParseError::Malformed(
            "temperature must be between 0 and 1".to_string(),
        ));
    }
    Ok(value)
}

fn validate_model(served_model_id: &str, parsed: &ParsedRequest) -> Result<(), String> {
    let model = parsed
        .model
        .as_deref()
        .ok_or_else(|| "missing required field `model`".to_string())?;
    if model != served_model_id {
        return Err(format!(
            "model `{model}` is not served; the served model is `{served_model_id}`"
        ));
    }

    Ok(())
}

fn map_error(error: &Error) -> (StatusCode, String) {
    match error.code {
        ErrorCode::InvalidArguments => (StatusCode::BAD_REQUEST, "invalid request".to_string()),
        ErrorCode::InvalidInput => (StatusCode::BAD_REQUEST, "invalid audio input".to_string()),
        ErrorCode::InputTooLarge => (
            StatusCode::PAYLOAD_TOO_LARGE,
            "audio file too large".to_string(),
        ),
        ErrorCode::AudioTooLong => (
            StatusCode::BAD_REQUEST,
            "audio duration exceeds maximum".to_string(),
        ),
        ErrorCode::UnsupportedFormat => (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported audio format".to_string(),
        ),
        ErrorCode::ModelNotFound => (StatusCode::BAD_REQUEST, "model file not found".to_string()),
        ErrorCode::EngineFailed
        | ErrorCode::ModelLoadFailed
        | ErrorCode::EngineNotAvailable
        | ErrorCode::Internal => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "transcription failed".to_string(),
        ),
        ErrorCode::InvalidConfig => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "invalid server configuration".to_string(),
        ),
    }
}

fn log_request(
    id: u64,
    started: Instant,
    status: StatusCode,
    detail: &str,
    audio_seconds: Option<f64>,
) {
    let elapsed_ms = started.elapsed().as_millis();
    match audio_seconds {
        Some(seconds) => {
            eprintln!("request {id}: {status} {detail} in {elapsed_ms} ms (audio {seconds:.2} s)")
        }
        None => eprintln!("request {id}: {status} {detail} in {elapsed_ms} ms"),
    }
}

fn openai_error(status: StatusCode, message: &str) -> Response {
    let body = serde_json::json!({
        "error": {
            "message": message,
            "type": error_type(status),
            "param": null,
            "code": error_code(status),
        }
    });
    json_response(status, body)
}

fn bad_request(message: &str) -> Response {
    openai_error(StatusCode::BAD_REQUEST, message)
}

fn payload_too_large() -> Response {
    openai_error(
        StatusCode::PAYLOAD_TOO_LARGE,
        "the request body is too large",
    )
}

fn request_timeout() -> Response {
    openai_error(
        StatusCode::REQUEST_TIMEOUT,
        "uploading the request body timed out",
    )
}

fn too_many_requests() -> Response {
    let body = serde_json::json!({
        "error": {
            "message": "the server is busy",
            "type": "rate_limit_error",
            "param": null,
            "code": "rate_limit",
        }
    });
    (
        StatusCode::TOO_MANY_REQUESTS,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::RETRY_AFTER, BUSY_RESPONSE_RETRY_AFTER),
        ],
        serde_json::to_string(&body).unwrap_or_default(),
    )
        .into_response()
}

fn gateway_timeout() -> Response {
    openai_error(
        StatusCode::GATEWAY_TIMEOUT,
        "processing the request timed out",
    )
}

fn service_unavailable(message: &str) -> Response {
    openai_error(StatusCode::SERVICE_UNAVAILABLE, message)
}

fn error_type(status: StatusCode) -> &'static str {
    if status == StatusCode::TOO_MANY_REQUESTS {
        "rate_limit_error"
    } else if status.is_server_error() {
        "server_error"
    } else {
        "invalid_request_error"
    }
}

fn error_code(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST => "invalid_request",
        StatusCode::PAYLOAD_TOO_LARGE => "payload_too_large",
        StatusCode::UNSUPPORTED_MEDIA_TYPE => "unsupported_media_type",
        StatusCode::REQUEST_TIMEOUT => "request_timeout",
        StatusCode::TOO_MANY_REQUESTS => "rate_limit",
        StatusCode::SERVICE_UNAVAILABLE => "service_unavailable",
        StatusCode::GATEWAY_TIMEOUT => "gateway_timeout",
        _ => "internal_error",
    }
}

fn json_response(status: StatusCode, value: serde_json::Value) -> Response {
    let body = serde_json::to_string(&value).unwrap_or_else(|_| {
        r#"{"error":{"message":"internal error","type":"server_error","param":null,"code":"internal_error"}}"#
            .to_string()
    });
    (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}

/// Wait for shutdown via Ctrl-C or SIGTERM.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        let mut sig = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler");
        sig.recv().await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FakeEngine;
    use axum::body::{Body, Bytes};
    use std::io::Cursor;

    fn make_state(engine: Arc<dyn Engine>, model_id: &str) -> ServerState {
        ServerState {
            engine,
            served_model_id: model_id.to_string(),
            config: Arc::new(Config::default()),
            admission: Arc::new(Semaphore::new(1)),
            shutdown: CancellationToken::new(),
            request_ids: Arc::new(std::sync::atomic::AtomicU64::new(1)),
        }
    }

    fn build_multipart(fields: &[(&str, &str, Option<&[u8]>)]) -> (Vec<u8>, String) {
        let boundary = "----pumice-test-boundary";
        let mut body = Vec::new();
        for (name, value, file) in fields {
            body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            if let Some(bytes) = file {
                body.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"; filename=\"audio.wav\"\r\n").as_bytes(),
                );
                body.extend_from_slice(b"Content-Type: audio/wav\r\n\r\n");
                body.extend_from_slice(bytes);
            } else {
                body.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}")
                        .as_bytes(),
                );
            }
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        (body, boundary.to_string())
    }

    fn silence_wav(seconds: f64) -> Vec<u8> {
        let samples = vec![0.0f32; (seconds * 16_000.0) as usize];
        let mut buf = Cursor::new(Vec::new());
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::new(&mut buf, spec).unwrap();
        for &s in &samples {
            writer.write_sample(s).unwrap();
        }
        writer.finalize().unwrap();
        buf.into_inner()
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let _state = make_state(Arc::new(FakeEngine::new("x", None::<String>)), "tiny");
        let response = health().await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn models_lists_only_served_id() {
        let state = make_state(Arc::new(FakeEngine::new("x", None::<String>)), "turbo");
        let response = list_models(State(state)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let ids: Vec<&str> = json["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["turbo"]);
    }

    #[tokio::test]
    async fn transcribe_json_returns_text() {
        let state = make_state(Arc::new(FakeEngine::new("Olá mundo", Some("pt"))), "tiny");
        let wav = silence_wav(1.0);
        let (body, boundary) = build_multipart(&[
            ("file", "", Some(&wav)),
            ("model", "tiny", None),
            ("language", "pt", None),
        ]);
        let request = Request::builder()
            .method("POST")
            .uri("/v1/audio/transcriptions")
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body))
            .unwrap();
        let response = transcriptions(State(state), request).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["text"], "Olá mundo");
    }

    #[tokio::test]
    async fn transcribe_text_returns_plain_text() {
        let state = make_state(Arc::new(FakeEngine::new("hello", None::<String>)), "tiny");
        let wav = silence_wav(1.0);
        let (body, boundary) = build_multipart(&[
            ("file", "", Some(&wav)),
            ("model", "tiny", None),
            ("response_format", "text", None),
        ]);
        let request = Request::builder()
            .method("POST")
            .uri("/v1/audio/transcriptions")
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body))
            .unwrap();
        let response = transcriptions(State(state), request).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(bytes, Bytes::from_static(b"hello"));
    }

    #[tokio::test]
    async fn missing_model_returns_400() {
        let state = make_state(Arc::new(FakeEngine::new("x", None::<String>)), "tiny");
        let wav = silence_wav(1.0);
        let (body, boundary) = build_multipart(&[("file", "", Some(&wav))]);
        let request = Request::builder()
            .method("POST")
            .uri("/v1/audio/transcriptions")
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body))
            .unwrap();
        let response = transcriptions(State(state), request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn wrong_model_returns_400() {
        let state = make_state(Arc::new(FakeEngine::new("x", None::<String>)), "tiny");
        let wav = silence_wav(1.0);
        let (body, boundary) =
            build_multipart(&[("file", "", Some(&wav)), ("model", "huge", None)]);
        let request = Request::builder()
            .method("POST")
            .uri("/v1/audio/transcriptions")
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body))
            .unwrap();
        let response = transcriptions(State(state), request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn busy_server_returns_429() {
        let state = make_state(Arc::new(FakeEngine::new("x", None::<String>)), "tiny");
        let _permit = state.admission.acquire().await.unwrap();
        let wav = silence_wav(1.0);
        let (body, boundary) =
            build_multipart(&[("file", "", Some(&wav)), ("model", "tiny", None)]);
        let request = Request::builder()
            .method("POST")
            .uri("/v1/audio/transcriptions")
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body))
            .unwrap();
        let response = transcriptions(State(state.clone()), request).await;
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn oversize_file_returns_413() {
        let state = make_state(Arc::new(FakeEngine::new("x", None::<String>)), "tiny");
        let big = vec![0u8; MAX_FILE_BYTES as usize + 1];
        let (body, boundary) =
            build_multipart(&[("file", "", Some(&big)), ("model", "tiny", None)]);
        let request = Request::builder()
            .method("POST")
            .uri("/v1/audio/transcriptions")
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body))
            .unwrap();
        let response = transcriptions(State(state), request).await;
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn unsupported_field_returns_400() {
        let state = make_state(Arc::new(FakeEngine::new("x", None::<String>)), "tiny");
        let wav = silence_wav(1.0);
        let (body, boundary) = build_multipart(&[
            ("file", "", Some(&wav)),
            ("model", "tiny", None),
            ("timestamp_granularities", "word", None),
        ]);
        let request = Request::builder()
            .method("POST")
            .uri("/v1/audio/transcriptions")
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body))
            .unwrap();
        let response = transcriptions(State(state), request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
