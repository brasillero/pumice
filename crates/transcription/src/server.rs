//! OpenAI-compatible HTTP server for `pumice-transcribe serve`.
//!
//! Serves on IPv4 loopback only, with one model loaded at startup and one
//! transcription admitted at a time. Every error body and log line is built from
//! fixed text: nothing the client sent (model names, field names, prompts, file
//! names) and nothing the engine produced is ever echoed or logged.

use crate::audio::MAX_FILE_BYTES;
use crate::engine::{CancelToken, Engine};
use crate::service::transcribe_pcm_from_reader;
use crate::{ErrorCode, TranscriptionOptions};
use axum::{
    Router,
    body::Body,
    extract::{Path, Request, State, rejection::PathRejection},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use bytes::Bytes;
use http_body_util::BodyExt;
use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio::time;
use tokio_util::sync::CancellationToken;

/// Whole request body, including multipart framing and any epilogue.
const MAX_TOTAL_BODY_BYTES: u64 = 26_000_000;
const MAX_TEXT_FIELD_BYTES: u64 = 16 * 1024;
const BUSY_RETRY_AFTER_SECONDS: &str = "1";

/// Time budgets, both measured from the request's arrival.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// The whole body must be received within this time.
    pub upload_deadline: Duration,
    /// The response (success or error) is sent within this time.
    pub total_deadline: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            upload_deadline: Duration::from_secs(15),
            total_deadline: Duration::from_secs(120),
        }
    }
}

#[derive(Clone)]
struct ServerState {
    engine: Arc<dyn Engine>,
    served_model_id: Arc<str>,
    default_language: Option<String>,
    limits: Limits,
    admission: Arc<Semaphore>,
    shutdown: CancellationToken,
    request_ids: Arc<AtomicU64>,
}

impl ServerState {
    fn new(
        engine: Arc<dyn Engine>,
        served_model_id: &str,
        default_language: Option<String>,
        limits: Limits,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            engine,
            served_model_id: Arc::from(served_model_id),
            default_language,
            limits,
            admission: Arc::new(Semaphore::new(1)),
            shutdown,
            request_ids: Arc::new(AtomicU64::new(1)),
        }
    }
}

/// Start the HTTP server on `127.0.0.1:port` and serve until `shutdown_signal`
/// fires. The returned task ends after shutdown, once any transcription still
/// running on a worker thread has finished.
pub async fn serve(
    port: u16,
    engine: Arc<dyn Engine>,
    served_model_id: String,
    default_language: Option<String>,
    shutdown_signal: impl Future<Output = ()> + Send + 'static,
) -> io::Result<tokio::task::JoinHandle<()>> {
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).await?;
    let local_addr = listener.local_addr()?;
    eprintln!("pumice-transcribe listening on http://{local_addr}/v1 (model: {served_model_id})");

    let shutdown = CancellationToken::new();
    let state = ServerState::new(
        engine,
        &served_model_id,
        default_language,
        Limits::default(),
        shutdown.clone(),
    );
    let admission = Arc::clone(&state.admission);
    let app = router(state);

    let handle = tokio::spawn(async move {
        let serve = axum::serve(listener, app).with_graceful_shutdown(async move {
            shutdown_signal.await;
            shutdown.cancel();
        });
        if let Err(e) = serve.await {
            eprintln!("server error: {e}");
        }
        // Drain: a cancelled worker keeps the admission permit until the
        // engine actually returns.
        let _ = admission.acquire().await;
    });

    Ok(handle)
}

fn router(state: ServerState) -> Router {
    Router::new()
        .route("/v1/audio/transcriptions", post(transcriptions))
        .route("/v1/models", get(list_models))
        .route("/v1/models/{model}", get(retrieve_model))
        .route("/health", get(health))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// An OpenAI error object. Every field is static text by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ApiError {
    status: StatusCode,
    kind: &'static str,
    code: Option<&'static str>,
    param: Option<&'static str>,
    message: &'static str,
}

const fn invalid(param: Option<&'static str>, message: &'static str) -> ApiError {
    ApiError {
        status: StatusCode::BAD_REQUEST,
        kind: "invalid_request_error",
        code: None,
        param,
        message,
    }
}

const fn server_error(status: StatusCode, message: &'static str) -> ApiError {
    ApiError {
        status,
        kind: "server_error",
        code: None,
        param: None,
        message,
    }
}

const BUSY: ApiError = ApiError {
    status: StatusCode::TOO_MANY_REQUESTS,
    kind: "rate_limit_error",
    code: Some("rate_limit_exceeded"),
    param: None,
    message: "The server is busy with another transcription. Retry after a short wait.",
};
const BODY_TOO_LARGE: ApiError = ApiError {
    status: StatusCode::PAYLOAD_TOO_LARGE,
    kind: "invalid_request_error",
    code: None,
    param: None,
    message: "The request body exceeds the maximum size of 26000000 bytes.",
};
const FILE_TOO_LARGE: ApiError = ApiError {
    status: StatusCode::PAYLOAD_TOO_LARGE,
    kind: "invalid_request_error",
    code: None,
    param: Some("file"),
    message: "The uploaded file exceeds the maximum size of 25000000 bytes.",
};
const FIELD_TOO_LARGE: ApiError = ApiError {
    status: StatusCode::PAYLOAD_TOO_LARGE,
    kind: "invalid_request_error",
    code: None,
    param: None,
    message: "A text field exceeds the maximum size of 16384 bytes.",
};
const UPLOAD_TIMEOUT: ApiError = ApiError {
    status: StatusCode::REQUEST_TIMEOUT,
    kind: "invalid_request_error",
    code: None,
    param: None,
    message: "The request body was not received in time.",
};
const MODEL_NOT_FOUND: ApiError = ApiError {
    status: StatusCode::NOT_FOUND,
    kind: "invalid_request_error",
    code: Some("model_not_found"),
    param: Some("model"),
    message: "The requested model does not exist on this server.",
};
const NOT_FOUND: ApiError = ApiError {
    status: StatusCode::NOT_FOUND,
    kind: "invalid_request_error",
    code: None,
    param: None,
    message: "Invalid URL.",
};
const METHOD_NOT_ALLOWED: ApiError = ApiError {
    status: StatusCode::METHOD_NOT_ALLOWED,
    kind: "invalid_request_error",
    code: None,
    param: None,
    message: "Method not allowed for this URL.",
};
const NOT_MULTIPART: ApiError = invalid(
    None,
    "The request must be multipart/form-data with a boundary.",
);
const MALFORMED_MULTIPART: ApiError = invalid(None, "The multipart request body is malformed.");
const UNSUPPORTED_PARAMETER: ApiError = invalid(
    None,
    "Unrecognized or unsupported request parameter. Supported: file, model, language, prompt, response_format, temperature, stream.",
);
const DUPLICATE_PARAMETER: ApiError = invalid(None, "A request parameter was sent more than once.");
const MISSING_FILE: ApiError = invalid(Some("file"), "You must provide a file parameter.");
const EMPTY_FILE: ApiError = invalid(Some("file"), "The uploaded file is empty.");
const FILE_NOT_UPLOAD: ApiError = invalid(
    Some("file"),
    "The file parameter must be sent as a file upload.",
);
const MISSING_MODEL: ApiError = invalid(Some("model"), "You must provide a model parameter.");
const INVALID_LANGUAGE: ApiError = invalid(
    Some("language"),
    "language must be a supported ISO-639-1 code.",
);
const INVALID_RESPONSE_FORMAT: ApiError = invalid(
    Some("response_format"),
    "Unsupported response_format. Supported: json, text.",
);
const INVALID_TEMPERATURE: ApiError = invalid(
    Some("temperature"),
    "temperature must be a number between 0 and 1.",
);
const STREAM_UNSUPPORTED: ApiError = invalid(Some("stream"), "Streaming is not supported.");
const INVALID_STREAM: ApiError = invalid(Some("stream"), "stream must be true or false.");
const NOT_UTF8: ApiError = invalid(None, "A text parameter is not valid UTF-8.");
const NUL_IN_TEXT: ApiError = invalid(None, "A text parameter contains a NUL character.");
const UNDECODABLE_AUDIO: ApiError = invalid(Some("file"), "The audio file could not be decoded.");
#[cfg(feature = "opus")]
const UNSUPPORTED_FORMAT: ApiError = invalid(
    Some("file"),
    "Invalid file format. Supported formats: wav, mp3, flac, ogg (Vorbis), ogg (Opus), WebM (Opus).",
);
#[cfg(not(feature = "opus"))]
const UNSUPPORTED_FORMAT: ApiError = invalid(
    Some("file"),
    "Invalid file format. Supported formats: wav, mp3, flac, ogg (Vorbis).",
);
const AUDIO_TOO_LONG: ApiError = invalid(
    Some("file"),
    "Audio longer than 120 seconds is not supported.",
);
const SHUTTING_DOWN: ApiError = server_error(
    StatusCode::SERVICE_UNAVAILABLE,
    "The server is shutting down.",
);
const DEADLINE_EXCEEDED: ApiError = server_error(
    StatusCode::GATEWAY_TIMEOUT,
    "The transcription did not finish in time.",
);
const TRANSCRIPTION_FAILED: ApiError = server_error(
    StatusCode::INTERNAL_SERVER_ERROR,
    "The transcription failed.",
);

/// Map a library error to a fixed HTTP error. The library error's own message
/// may contain decoder or native text and is never used.
fn map_error(code: ErrorCode) -> ApiError {
    match code {
        ErrorCode::InvalidInput => UNDECODABLE_AUDIO,
        ErrorCode::InputTooLarge => FILE_TOO_LARGE,
        ErrorCode::AudioTooLong => AUDIO_TOO_LONG,
        ErrorCode::UnsupportedFormat => UNSUPPORTED_FORMAT,
        ErrorCode::Cancelled => SHUTTING_DOWN,
        ErrorCode::InvalidArguments
        | ErrorCode::InvalidConfig
        | ErrorCode::ModelNotFound
        | ErrorCode::EngineNotAvailable
        | ErrorCode::EngineFailed
        | ErrorCode::ModelLoadFailed
        | ErrorCode::ChecksumMismatch
        | ErrorCode::DownloadFailed
        | ErrorCode::Internal => TRANSCRIPTION_FAILED,
    }
}

impl ApiError {
    /// The same error, naming one of the server's own (static) parameter names.
    const fn for_param(self, param: &'static str) -> Self {
        Self {
            param: Some(param),
            ..self
        }
    }
}

/// The accepted multipart parameter names. Unknown names are never echoed.
fn known_param(name: &str) -> Option<&'static str> {
    Some(match name {
        "file" => "file",
        "model" => "model",
        "language" => "language",
        "response_format" => "response_format",
        "prompt" => "prompt",
        "temperature" => "temperature",
        "stream" => "stream",
        _ => return None,
    })
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({
            "error": {
                "message": self.message,
                "type": self.kind,
                "param": self.param,
                "code": self.code,
            }
        });
        let mut response = json_response(self.status, &body);
        if self.status == StatusCode::TOO_MANY_REQUESTS {
            response.headers_mut().insert(
                header::RETRY_AFTER,
                header::HeaderValue::from_static(BUSY_RETRY_AFTER_SECONDS),
            );
        }
        response
    }
}

fn json_response(status: StatusCode, value: &serde_json::Value) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        value.to_string(),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Request log
// ---------------------------------------------------------------------------

/// One metadata line per request. Logs the disconnect if the handler is
/// dropped before it answers.
struct RequestLog {
    id: u64,
    started: Instant,
    answered: bool,
}

impl RequestLog {
    fn new(state: &ServerState) -> Self {
        Self {
            id: state.request_ids.fetch_add(1, Ordering::Relaxed),
            started: Instant::now(),
            answered: false,
        }
    }

    fn answered(mut self, status: StatusCode, audio_seconds: Option<f64>) {
        self.answered = true;
        let ms = self.started.elapsed().as_millis();
        match audio_seconds {
            Some(s) => eprintln!("request {}: {status} in {ms} ms (audio {s:.2} s)", self.id),
            None => eprintln!("request {}: {status} in {ms} ms", self.id),
        }
    }
}

impl Drop for RequestLog {
    fn drop(&mut self) {
        if !self.answered {
            let ms = self.started.elapsed().as_millis();
            eprintln!("request {}: client disconnected after {ms} ms", self.id);
        }
    }
}

/// Cancels the engine when the handler returns or is dropped (client
/// disconnect). After a normal completion this is a harmless no-op.
struct CancelOnDrop(CancelToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /v1/audio/transcriptions
async fn transcriptions(State(state): State<ServerState>, request: Request) -> Response {
    let log = RequestLog::new(&state);
    match transcribe(&state, request).await {
        Ok((format, text, audio_seconds)) => {
            log.answered(StatusCode::OK, Some(audio_seconds));
            success_response(format, text)
        }
        Err(error) => {
            log.answered(error.status, None);
            error.into_response()
        }
    }
}

async fn transcribe(
    state: &ServerState,
    request: Request,
) -> Result<(ResponseFormat, String, f64), ApiError> {
    let arrived = time::Instant::now();
    let deadline = arrived + state.limits.total_deadline;
    let upload_deadline = (arrived + state.limits.upload_deadline).min(deadline);

    // Admission before reading anything: a busy server answers at once.
    let permit = state
        .admission
        .clone()
        .try_acquire_owned()
        .map_err(|_| BUSY)?;
    if state.shutdown.is_cancelled() {
        return Err(SHUTTING_DOWN);
    }

    let (parts, body) = request.into_parts();
    let boundary = multipart_boundary(&parts.headers)?;
    if declared_length(&parts.headers).is_some_and(|len| len > MAX_TOTAL_BODY_BYTES) {
        return Err(BODY_TOO_LARGE);
    }

    let body = tokio::select! {
        biased;
        () = state.shutdown.cancelled() => return Err(SHUTTING_DOWN),
        read = time::timeout_at(upload_deadline, read_body(body)) => {
            read.map_err(|_| UPLOAD_TIMEOUT)??
        }
    };
    let form = parse_form(body, boundary).await?;

    let model = form.model.as_deref().ok_or(MISSING_MODEL)?;
    if model != &*state.served_model_id {
        return Err(MODEL_NOT_FOUND);
    }
    let language = match form.language {
        Some(lang) => Some(crate::config::validate_language(&lang).map_err(|_| INVALID_LANGUAGE)?),
        None => state.default_language.clone(),
    };
    let options = TranscriptionOptions {
        language,
        prompt: form.prompt,
        temperature: form.temperature,
    };

    // The worker owns the permit: a timed-out or abandoned request keeps the
    // server busy until the engine really stops.
    let cancel = CancelToken::new();
    let _cancel_on_drop = CancelOnDrop(cancel.clone());
    let engine = Arc::clone(&state.engine);
    let worker_cancel = cancel.clone();
    let file = form.file;
    let worker = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        transcribe_pcm_from_reader(
            io::Cursor::new(file),
            engine.as_ref(),
            &options,
            &worker_cancel,
        )
    });

    tokio::select! {
        biased;
        () = state.shutdown.cancelled() => Err(SHUTTING_DOWN),
        () = time::sleep_until(deadline) => Err(DEADLINE_EXCEEDED),
        joined = worker => match joined {
            Ok(Ok(result)) => Ok((form.response_format, result.transcript.text, result.audio_seconds)),
            Ok(Err(error)) => Err(map_error(error.code)),
            // The worker panicked; its permit was released while unwinding.
            Err(_) => Err(TRANSCRIPTION_FAILED),
        },
    }
}

fn multipart_boundary(headers: &HeaderMap) -> Result<String, ApiError> {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .ok_or(NOT_MULTIPART)?;
    multer::parse_boundary(content_type).map_err(|_| NOT_MULTIPART)
}

fn declared_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
}

/// Read the complete body, epilogue included, failing as soon as it exceeds
/// the size limit.
async fn read_body(mut body: Body) -> Result<Bytes, ApiError> {
    let mut buf = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| MALFORMED_MULTIPART)?;
        if let Ok(data) = frame.into_data() {
            if (buf.len() + data.len()) as u64 > MAX_TOTAL_BODY_BYTES {
                return Err(BODY_TOO_LARGE);
            }
            buf.extend_from_slice(&data);
        }
    }
    Ok(Bytes::from(buf))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResponseFormat {
    Json,
    Text,
}

struct Form {
    file: Bytes,
    model: Option<String>,
    language: Option<String>,
    response_format: ResponseFormat,
    prompt: Option<String>,
    temperature: Option<f32>,
}

/// A single-chunk stream over the already-received body.
struct OnceBody(Option<Bytes>);

impl futures_core::Stream for OnceBody {
    type Item = Result<Bytes, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.0.take().map(Ok))
    }
}

#[derive(Default)]
struct Seen {
    file: bool,
    model: bool,
    language: bool,
    response_format: bool,
    prompt: bool,
    temperature: bool,
    stream: bool,
}

async fn parse_form(body: Bytes, boundary: String) -> Result<Form, ApiError> {
    let limits = multer::SizeLimit::new()
        .whole_stream(MAX_TOTAL_BODY_BYTES)
        .per_field(MAX_TEXT_FIELD_BYTES)
        .for_field("file", MAX_FILE_BYTES);
    let constraints = multer::Constraints::new().size_limit(limits);
    let mut multipart =
        multer::Multipart::with_constraints(OnceBody(Some(body)), boundary, constraints);

    let mut seen = Seen::default();
    let mut file = None;
    let mut model = None;
    let mut language = None;
    let mut response_format = ResponseFormat::Json;
    let mut prompt = None;
    let mut temperature = None;

    while let Some(field) = multipart.next_field().await.map_err(map_multer_error)? {
        let is_upload = field.file_name().is_some();
        let param = field
            .name()
            .and_then(known_param)
            .ok_or(UNSUPPORTED_PARAMETER)?;
        let slot = match param {
            "file" => &mut seen.file,
            "model" => &mut seen.model,
            "language" => &mut seen.language,
            "response_format" => &mut seen.response_format,
            "prompt" => &mut seen.prompt,
            "temperature" => &mut seen.temperature,
            _ => &mut seen.stream,
        };
        if std::mem::replace(slot, true) {
            return Err(DUPLICATE_PARAMETER.for_param(param));
        }

        if param == "file" {
            if !is_upload {
                return Err(FILE_NOT_UPLOAD);
            }
            let data = field.bytes().await.map_err(map_multer_error)?;
            if data.is_empty() {
                return Err(EMPTY_FILE);
            }
            file = Some(data);
            continue;
        }

        if is_upload {
            return Err(text_param_error(param));
        }
        let data = field.bytes().await.map_err(map_multer_error)?;
        let value = String::from_utf8(data.to_vec()).map_err(|_| NOT_UTF8.for_param(param))?;
        // The engine passes text to C APIs that stop at NUL.
        if value.contains('\0') {
            return Err(NUL_IN_TEXT.for_param(param));
        }
        match param {
            "model" => model = Some(value),
            "language" => language = Some(value),
            "prompt" => prompt = Some(value),
            "response_format" => {
                response_format = match value.as_str() {
                    "json" => ResponseFormat::Json,
                    "text" => ResponseFormat::Text,
                    _ => return Err(INVALID_RESPONSE_FORMAT),
                }
            }
            "temperature" => {
                let t: f32 = value.trim().parse().map_err(|_| INVALID_TEMPERATURE)?;
                if !t.is_finite() || !(0.0..=1.0).contains(&t) {
                    return Err(INVALID_TEMPERATURE);
                }
                temperature = Some(t);
            }
            "stream" => match value.as_str() {
                "false" => {}
                "true" => return Err(STREAM_UNSUPPORTED),
                _ => return Err(INVALID_STREAM),
            },
            _ => unreachable!("every accepted name is matched above"),
        }
    }

    Ok(Form {
        file: file.ok_or(MISSING_FILE)?,
        model,
        language,
        response_format,
        prompt,
        temperature,
    })
}

/// A text parameter that arrived as a file upload.
fn text_param_error(param: &'static str) -> ApiError {
    invalid(
        Some(param),
        "This parameter must be sent as a text field, not a file.",
    )
}

fn map_multer_error(error: multer::Error) -> ApiError {
    match error {
        multer::Error::FieldSizeExceeded { field_name, .. } => {
            match field_name.as_deref().and_then(known_param) {
                Some("file") => FILE_TOO_LARGE,
                Some(param) => FIELD_TOO_LARGE.for_param(param),
                None => FIELD_TOO_LARGE,
            }
        }
        multer::Error::StreamSizeExceeded { .. } => BODY_TOO_LARGE,
        _ => MALFORMED_MULTIPART,
    }
}

fn success_response(format: ResponseFormat, text: String) -> Response {
    match format {
        ResponseFormat::Text => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            text,
        )
            .into_response(),
        ResponseFormat::Json => json_response(StatusCode::OK, &serde_json::json!({ "text": text })),
    }
}

fn model_object(id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "object": "model",
        "created": 0,
        "owned_by": "pumice",
    })
}

/// GET /v1/models: only the served model.
async fn list_models(State(state): State<ServerState>) -> Response {
    let body = serde_json::json!({
        "object": "list",
        "data": [model_object(&state.served_model_id)],
    });
    json_response(StatusCode::OK, &body)
}

/// GET /v1/models/{model}
async fn retrieve_model(
    State(state): State<ServerState>,
    model: Result<Path<String>, PathRejection>,
) -> Response {
    // A path that is not valid UTF-8 cannot name the served model.
    let Ok(Path(model)) = model else {
        return MODEL_NOT_FOUND.into_response();
    };
    if model == *state.served_model_id {
        json_response(StatusCode::OK, &model_object(&state.served_model_id))
    } else {
        MODEL_NOT_FOUND.into_response()
    }
}

/// GET /health: liveness; answers while a transcription runs.
async fn health() -> Response {
    json_response(
        StatusCode::OK,
        &serde_json::json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }),
    )
}

async fn not_found() -> Response {
    NOT_FOUND.into_response()
}

async fn method_not_allowed() -> Response {
    METHOD_NOT_ALLOWED.into_response()
}

/// Wait for shutdown via Ctrl-C or SIGTERM.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}

#[cfg(test)]
mod tests;
