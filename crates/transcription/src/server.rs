//! OpenAI-compatible HTTP server for `pumice-transcribe serve`.
//!
//! Serves on IPv4 loopback only, with one model loaded at startup and one
//! transcription admitted at a time. In managed mode (see [`crate::managed`])
//! it also serves `GET /ready` and `POST /pumice/v1/transcriptions`; the
//! OpenAI routes answer exactly as they do without it. Every error body and
//! log line is built from fixed text: nothing the client sent (model names,
//! field names, prompts, file names) and nothing the engine produced is ever
//! echoed or logged.

use crate::audio::MAX_FILE_BYTES;
use crate::engine::{CancelToken, Engine};
use crate::managed::{CAPABILITIES, PROTOCOL_VERSION};
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
    managed: Option<Arc<ManagedState>>,
}

struct ManagedState {
    instance_id: String,
    /// The only accepted `Host`: `127.0.0.1:<bound port>`.
    host: String,
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
            managed: None,
        }
    }

    fn managed(mut self, instance_id: String, port: u16) -> Self {
        self.managed = Some(Arc::new(ManagedState {
            instance_id,
            host: format!("{}:{port}", Ipv4Addr::LOCALHOST),
        }));
        self
    }
}

/// Identity of a managed instance (`serve --managed`).
pub struct Managed {
    /// Already validated by [`crate::managed::validate_instance_id`].
    pub instance_id: String,
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
    let listener = bind(port).await?;
    serve_on(
        listener,
        engine,
        served_model_id,
        default_language,
        None,
        shutdown_signal,
    )
}

/// Bind `127.0.0.1:port`; port 0 picks a free port.
pub async fn bind(port: u16) -> io::Result<TcpListener> {
    TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).await
}

/// [`serve`] on a listener from [`bind`], optionally in managed mode. Must be
/// called inside a Tokio runtime.
pub fn serve_on(
    listener: TcpListener,
    engine: Arc<dyn Engine>,
    served_model_id: String,
    default_language: Option<String>,
    managed: Option<Managed>,
    shutdown_signal: impl Future<Output = ()> + Send + 'static,
) -> io::Result<tokio::task::JoinHandle<()>> {
    let local_addr = listener.local_addr()?;
    if local_addr.ip() != Ipv4Addr::LOCALHOST {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the server only listens on 127.0.0.1",
        ));
    }
    eprintln!("pumice-transcribe listening on http://{local_addr}/v1 (model: {served_model_id})");

    let shutdown = CancellationToken::new();
    let mut state = ServerState::new(
        engine,
        &served_model_id,
        default_language,
        Limits::default(),
        shutdown.clone(),
    );
    if let Some(managed) = managed {
        state = state.managed(managed.instance_id, local_addr.port());
    }
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
    let mut router = Router::new()
        .route("/v1/audio/transcriptions", post(transcriptions))
        .route("/v1/models", get(list_models))
        .route("/v1/models/{model}", get(retrieve_model))
        .route("/health", get(health));
    if state.managed.is_some() {
        router = router
            .route("/ready", get(ready))
            .route("/pumice/v1/transcriptions", post(detailed_transcriptions));
    }
    router
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// An OpenAI error object. Every field is static text by construction.
///
/// `code` is what the OpenAI routes send (set only where OpenAI defines one);
/// managed routes always send `stable` as the code instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ApiError {
    status: StatusCode,
    kind: &'static str,
    code: Option<&'static str>,
    stable: &'static str,
    param: Option<&'static str>,
    message: &'static str,
}

const fn invalid(
    stable: &'static str,
    param: Option<&'static str>,
    message: &'static str,
) -> ApiError {
    ApiError {
        status: StatusCode::BAD_REQUEST,
        kind: "invalid_request_error",
        code: None,
        stable,
        param,
        message,
    }
}

const fn server_error(status: StatusCode, stable: &'static str, message: &'static str) -> ApiError {
    ApiError {
        status,
        kind: "server_error",
        code: None,
        stable,
        param: None,
        message,
    }
}

const BUSY: ApiError = ApiError {
    status: StatusCode::TOO_MANY_REQUESTS,
    kind: "rate_limit_error",
    code: Some("rate_limit_exceeded"),
    stable: "rate_limit_exceeded",
    param: None,
    message: "The server is busy with another transcription. Retry after a short wait.",
};
const BODY_TOO_LARGE: ApiError = ApiError {
    status: StatusCode::PAYLOAD_TOO_LARGE,
    kind: "invalid_request_error",
    code: None,
    stable: "body_too_large",
    param: None,
    message: "The request body exceeds the maximum size of 26000000 bytes.",
};
const FILE_TOO_LARGE: ApiError = ApiError {
    status: StatusCode::PAYLOAD_TOO_LARGE,
    kind: "invalid_request_error",
    code: None,
    stable: "file_too_large",
    param: Some("file"),
    message: "The uploaded file exceeds the maximum size of 25000000 bytes.",
};
const FIELD_TOO_LARGE: ApiError = ApiError {
    status: StatusCode::PAYLOAD_TOO_LARGE,
    kind: "invalid_request_error",
    code: None,
    stable: "field_too_large",
    param: None,
    message: "A text field exceeds the maximum size of 16384 bytes.",
};
const UPLOAD_TIMEOUT: ApiError = ApiError {
    status: StatusCode::REQUEST_TIMEOUT,
    kind: "invalid_request_error",
    code: None,
    stable: "upload_timeout",
    param: None,
    message: "The request body was not received in time.",
};
const MODEL_NOT_FOUND: ApiError = ApiError {
    status: StatusCode::NOT_FOUND,
    kind: "invalid_request_error",
    code: Some("model_not_found"),
    stable: "model_not_found",
    param: Some("model"),
    message: "The requested model does not exist on this server.",
};
const NOT_FOUND: ApiError = ApiError {
    status: StatusCode::NOT_FOUND,
    kind: "invalid_request_error",
    code: None,
    stable: "not_found",
    param: None,
    message: "Invalid URL.",
};
const METHOD_NOT_ALLOWED: ApiError = ApiError {
    status: StatusCode::METHOD_NOT_ALLOWED,
    kind: "invalid_request_error",
    code: None,
    stable: "method_not_allowed",
    param: None,
    message: "Method not allowed for this URL.",
};
const NOT_MULTIPART: ApiError = invalid(
    "not_multipart",
    None,
    "The request must be multipart/form-data with a boundary.",
);
const MALFORMED_MULTIPART: ApiError = invalid(
    "malformed_multipart",
    None,
    "The multipart request body is malformed.",
);
const UNSUPPORTED_PARAMETER: ApiError = invalid(
    "unsupported_parameter",
    None,
    "Unrecognized or unsupported request parameter. Supported: file, model, language, prompt, response_format, temperature, stream.",
);
const DUPLICATE_PARAMETER: ApiError = invalid(
    "duplicate_parameter",
    None,
    "A request parameter was sent more than once.",
);
const MISSING_FILE: ApiError = invalid(
    "missing_file",
    Some("file"),
    "You must provide a file parameter.",
);
const EMPTY_FILE: ApiError = invalid("empty_file", Some("file"), "The uploaded file is empty.");
const FILE_NOT_UPLOAD: ApiError = invalid(
    "file_not_upload",
    Some("file"),
    "The file parameter must be sent as a file upload.",
);
const MISSING_MODEL: ApiError = invalid(
    "missing_model",
    Some("model"),
    "You must provide a model parameter.",
);
const INVALID_LANGUAGE: ApiError = invalid(
    "invalid_language",
    Some("language"),
    "language must be a supported ISO-639-1 code.",
);
const INVALID_RESPONSE_FORMAT: ApiError = invalid(
    "invalid_response_format",
    Some("response_format"),
    "Unsupported response_format. Supported: json, text.",
);
const INVALID_TEMPERATURE: ApiError = invalid(
    "invalid_temperature",
    Some("temperature"),
    "temperature must be a number between 0 and 1.",
);
const STREAM_UNSUPPORTED: ApiError = invalid(
    "stream_unsupported",
    Some("stream"),
    "Streaming is not supported.",
);
const INVALID_STREAM: ApiError = invalid(
    "invalid_stream",
    Some("stream"),
    "stream must be true or false.",
);
const NOT_UTF8: ApiError = invalid("invalid_utf8", None, "A text parameter is not valid UTF-8.");
const NUL_IN_TEXT: ApiError = invalid(
    "nul_in_text",
    None,
    "A text parameter contains a NUL character.",
);
const UNDECODABLE_AUDIO: ApiError = invalid(
    "invalid_audio",
    Some("file"),
    "The audio file could not be decoded.",
);
#[cfg(feature = "opus")]
const UNSUPPORTED_FORMAT: ApiError = invalid(
    "unsupported_format",
    Some("file"),
    "Invalid file format. Supported formats: wav, mp3, flac, ogg (Vorbis), ogg (Opus), WebM (Opus).",
);
#[cfg(not(feature = "opus"))]
const UNSUPPORTED_FORMAT: ApiError = invalid(
    "unsupported_format",
    Some("file"),
    "Invalid file format. Supported formats: wav, mp3, flac, ogg (Vorbis).",
);
const AUDIO_TOO_LONG: ApiError = invalid(
    "audio_too_long",
    Some("file"),
    "Audio longer than 120 seconds is not supported.",
);
const SHUTTING_DOWN: ApiError = server_error(
    StatusCode::SERVICE_UNAVAILABLE,
    "shutting_down",
    "The server is shutting down.",
);
const DEADLINE_EXCEEDED: ApiError = server_error(
    StatusCode::GATEWAY_TIMEOUT,
    "deadline_exceeded",
    "The transcription did not finish in time.",
);
const TRANSCRIPTION_FAILED: ApiError = server_error(
    StatusCode::INTERNAL_SERVER_ERROR,
    "transcription_failed",
    "The transcription failed.",
);

const DETAILED_RESPONSE_FORMAT: ApiError = invalid(
    "invalid_response_format",
    Some("response_format"),
    "Unsupported response_format. Supported: json.",
);
const HOST_NOT_ALLOWED: ApiError = ApiError {
    status: StatusCode::FORBIDDEN,
    kind: "invalid_request_error",
    code: None,
    stable: "host_not_allowed",
    param: None,
    message: "This route only accepts the Host 127.0.0.1 with the server's port.",
};
const ORIGIN_NOT_ALLOWED: ApiError = ApiError {
    status: StatusCode::FORBIDDEN,
    kind: "invalid_request_error",
    code: None,
    stable: "origin_not_allowed",
    param: None,
    message: "This route does not accept requests with an Origin header.",
};

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
        self.response(self.code)
    }
}

impl ApiError {
    /// The error as managed routes send it, with its stable `code`.
    fn managed_response(self) -> Response {
        self.response(Some(self.stable))
    }

    fn response(self, code: Option<&'static str>) -> Response {
        let body = serde_json::json!({
            "error": {
                "message": self.message,
                "type": self.kind,
                "param": self.param,
                "code": code,
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
    match transcribe(&state, request, Endpoint::OpenAi).await {
        Ok(done) => {
            log.answered(StatusCode::OK, Some(done.audio_seconds));
            success_response(done.format, done.text)
        }
        Err(error) => {
            log.answered(error.status, None);
            error.into_response()
        }
    }
}

/// POST /pumice/v1/transcriptions (managed mode only): the same request,
/// limits and admission as `/v1/audio/transcriptions`, answered with a
/// versioned detailed result instead of the OpenAI one.
async fn detailed_transcriptions(State(state): State<ServerState>, request: Request) -> Response {
    let log = RequestLog::new(&state);
    let result = match check_managed_request(&state, &request) {
        Ok(()) => transcribe(&state, request, Endpoint::Detailed).await,
        Err(error) => Err(error),
    };
    match result {
        Ok(done) => {
            log.answered(StatusCode::OK, Some(done.audio_seconds));
            let mut body = serde_json::json!({ "schema_version": 1, "text": done.text });
            if let Some(language) = done.language {
                body["language"] = language.into();
            }
            body["audio_seconds"] = done.audio_seconds.into();
            body["inference_ms"] = u64::try_from(done.inference.as_millis())
                .unwrap_or(u64::MAX)
                .into();
            json_response(StatusCode::OK, &body)
        }
        Err(error) => {
            log.answered(error.status, None);
            error.managed_response()
        }
    }
}

/// Managed routes answer only their owner: the literal `Host` `127.0.0.1`
/// with the bound port (no DNS name, against DNS rebinding) and no `Origin`
/// header (browsers send one on cross-origin requests). Checked before
/// admission and before reading the body.
fn check_managed_request(state: &ServerState, request: &Request) -> Result<(), ApiError> {
    let Some(managed) = state.managed.as_deref() else {
        return Err(NOT_FOUND);
    };
    if request.headers().contains_key(header::ORIGIN) {
        return Err(ORIGIN_NOT_ALLOWED);
    }
    let mut hosts = request.headers().get_all(header::HOST).iter();
    let host_ok = matches!(
        (hosts.next(), hosts.next()),
        (Some(host), None) if host.as_bytes() == managed.host.as_bytes()
    );
    let authority_ok = request
        .uri()
        .authority()
        .is_none_or(|authority| authority.as_str() == managed.host);
    if host_ok && authority_ok {
        Ok(())
    } else {
        Err(HOST_NOT_ALLOWED)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Endpoint {
    OpenAi,
    Detailed,
}

/// A finished transcription. No `Debug`: `text` is the transcript.
struct Transcribed {
    format: ResponseFormat,
    text: String,
    language: Option<String>,
    audio_seconds: f64,
    inference: Duration,
}

async fn transcribe(
    state: &ServerState,
    request: Request,
    endpoint: Endpoint,
) -> Result<Transcribed, ApiError> {
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
    if endpoint == Endpoint::Detailed && form.response_format != ResponseFormat::Json {
        return Err(DETAILED_RESPONSE_FORMAT);
    }

    let model = form.model.as_deref().ok_or(MISSING_MODEL)?;
    if model != &*state.served_model_id {
        return Err(MODEL_NOT_FOUND);
    }
    let language = match form.language {
        Some(lang) => Some(crate::config::validate_language(&lang).map_err(|_| INVALID_LANGUAGE)?),
        None => state.default_language.clone(),
    };
    let hint = language.clone();
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
            Ok(Ok(result)) => Ok(Transcribed {
                format: form.response_format,
                text: result.transcript.text,
                // The hint the engine was given wins over what it detected,
                // as in the `file` command.
                language: hint.or(result.transcript.language),
                audio_seconds: result.audio_seconds,
                inference: result.inference,
            }),
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
        "text_param_as_file",
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

/// GET /ready (managed mode only): the instance's identity while it accepts
/// work; 503 once shutting down. A running transcription does not make it
/// unready.
async fn ready(State(state): State<ServerState>, request: Request) -> Response {
    if let Err(error) = check_managed_request(&state, &request) {
        return error.managed_response();
    }
    let Some(managed) = state.managed.as_deref() else {
        return NOT_FOUND.into_response();
    };
    if state.shutdown.is_cancelled() {
        return SHUTTING_DOWN.managed_response();
    }
    json_response(
        StatusCode::OK,
        &serde_json::json!({
            "schema_version": 1,
            "module": "transcription",
            "instance_id": managed.instance_id,
            "protocol_version": PROTOCOL_VERSION,
            "version": env!("CARGO_PKG_VERSION"),
            "model": &*state.served_model_id,
            "capabilities": CAPABILITIES,
            "state": "ready",
        }),
    )
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

/// [`shutdown_signal`] with its handlers installed before it returns, so a
/// signal that arrives before the future is first polled is not missed. Must
/// be called inside a Tokio runtime.
pub fn registered_shutdown_signal() -> io::Result<impl Future<Output = ()> + Send + 'static> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut interrupt = signal(SignalKind::interrupt())?;
        let mut terminate = signal(SignalKind::terminate())?;
        Ok(async move {
            tokio::select! {
                _ = interrupt.recv() => {}
                _ = terminate.recv() => {}
            }
        })
    }

    #[cfg(windows)]
    {
        let mut ctrl_c = tokio::signal::windows::ctrl_c()?;
        Ok(async move {
            ctrl_c.recv().await;
        })
    }

    #[cfg(not(any(unix, windows)))]
    {
        Ok(shutdown_signal())
    }
}

#[cfg(test)]
mod tests;
