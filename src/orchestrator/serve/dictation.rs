//! `POST /pumice/v1/dictations` (p13.1-flow-seam/D6): one audio upload and
//! one flow id in, the P12 run envelope out, through the shared flow
//! execution (`flow.rs`) and the warm transcription child (`warm.rs`).
//!
//! Admission comes before the body is read: a stopping service answers
//! 503, a busy one 429 with `Retry-After: 1` (one work permit, no queue),
//! an unready one 503. The permit is then held by the operation's own task
//! through every stage and its cleanup, so a client that goes away (the
//! connection's handler is dropped) cancels the operation without
//! releasing admission before its cleanup finished.
//!
//! Clocks start at the request's arrival: the upload must complete within
//! [`Dictations::upload_limit`], and the selected flow's total budget is
//! anchored to the arrival too (p12-pumice-run/D2), never reset.
//!
//! After every operation: the shared cleanup scope is drained (by
//! [`Operation::finish`]) and a transcription abandoned after it was sent
//! recycles its child generation (D5). If either cannot be confirmed,
//! dictations stop being admitted (status shows it) rather than start
//! another paid call over unconfirmed resources.
//!
//! Nothing here logs or reports content: events are metadata only, error
//! messages are fixed and never quote what the client sent.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::header::{self, HeaderMap, HeaderValue};
use hyper::{Request, Response, StatusCode};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};
use tokio::time::Instant;

use crate::orchestrator::cancel::{self, CancelGuard, CancelHandle, CancelReason};
use crate::orchestrator::config::{Flow, MAX_SERVED_FLOW_ID_BYTES, TEARDOWN_RESERVE};
use crate::orchestrator::error::StageError;
use crate::orchestrator::events::{EventSink, event, operation_id};
use crate::orchestrator::flow::{AudioSource, Deadlines, Operation, PreparedFlow, Runtime};
use crate::orchestrator::result::{
    HttpFlowError, RunEnvelope, RunOutput, StageStatus, http_error, stage,
};
use crate::orchestrator::serve::http::{Frontend, RoutePolicy, json_bytes_response};
use crate::orchestrator::serve::output::Reporter;
use crate::orchestrator::serve::warm::WarmTranscriber;

/// Upload limits, the transcription module's own (crates/transcription,
/// `MAX_TOTAL_BODY_BYTES`, `MAX_FILE_BYTES`, `MAX_TEXT_FIELD_BYTES`; kept
/// in sync by hand: the core does not depend on that crate).
pub const MAX_BODY_BYTES: u64 = 26_000_000;
pub const MAX_FILE_BYTES: u64 = 25_000_000;
pub const MAX_SCALAR_BYTES: u64 = 16 * 1024;
/// The `flow` field is an enabled flow id: tighter than any scalar.
pub const MAX_FLOW_BYTES: u64 = MAX_SERVED_FLOW_ID_BYTES as u64;
/// From arrival to the upload's end, at most (design §6).
pub const UPLOAD_LIMIT: Duration = Duration::from_secs(15);
/// The largest answer; a final text that does not fit fails the relay
/// stage (`relay_invalid_output`, cause `output_too_large`), never cut.
pub const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
/// A connection may outlive its longest flow by this much: the cleanup
/// after the flow's work deadline and the answer's write.
const CONNECTION_MARGIN: Duration = Duration::from_secs(5);
/// A recycled generation must be confirmed reaped within this.
const RECYCLE_LIMIT: Duration = Duration::from_secs(2);

/// What the route serves: the service's runtime (built once) and every
/// configured flow, so a disabled one is told apart from an unknown one.
pub struct Dictations {
    pub runtime: Arc<Runtime>,
    pub flows: Vec<Flow>,
}

impl Dictations {
    /// See [`upload_limit`].
    pub fn upload_limit(&self) -> Duration {
        upload_limit(&self.flows)
    }

    /// See [`policy`].
    pub fn policy(&self) -> RoutePolicy {
        policy(&self.flows)
    }
}

/// `min(15 s, smallest enabled flow total − 2.5 s)`: a flow chosen by a
/// part that arrives last can never have spent more of its budget on the
/// upload than its own work deadline allows (design §6).
pub fn upload_limit(flows: &[Flow]) -> Duration {
    flows
        .iter()
        .filter(|flow| flow.enabled)
        .map(|flow| flow.budgets.total.saturating_sub(TEARDOWN_RESERVE))
        .fold(UPLOAD_LIMIT, Duration::min)
}

/// The route's connection bound: the longest enabled flow plus
/// [`CONNECTION_MARGIN`], and the 16 MiB answer.
pub fn policy(flows: &[Flow]) -> RoutePolicy {
    let longest = flows
        .iter()
        .filter(|flow| flow.enabled)
        .map(|flow| flow.budgets.total)
        .fold(upload_limit(flows), Duration::max);
    RoutePolicy {
        connection_limit: longest + CONNECTION_MARGIN,
        max_response_bytes: MAX_RESPONSE_BYTES,
    }
}

/// The single work permit and what status reports about it.
pub struct Admission {
    permit: Arc<Semaphore>,
    active: AtomicBool,
    stage: AtomicU8,
    /// A cleanup could not be confirmed: no further dictation is admitted.
    stopped: AtomicBool,
}

impl Default for Admission {
    fn default() -> Admission {
        Admission {
            permit: Arc::new(Semaphore::new(1)),
            active: AtomicBool::new(false),
            stage: AtomicU8::new(0),
            stopped: AtomicBool::new(false),
        }
    }
}

const STAGES: [&str; 5] = [
    stage::PREPARE,
    stage::TRANSCRIPTION,
    stage::FORMATTER,
    stage::RELAY,
    stage::CLEANUP,
];

impl Admission {
    pub fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }

    /// Stops admission for the rest of the service's life; true the first
    /// time.
    pub fn stop(&self) -> bool {
        !self.stopped.swap(true, Ordering::AcqRel)
    }

    pub fn active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// The running dictation's coarse stage.
    pub fn stage(&self) -> Option<&'static str> {
        match self.stage.load(Ordering::Acquire) {
            0 => None,
            n => STAGES.get(usize::from(n) - 1).copied(),
        }
    }

    fn set_stage(&self, name: &'static str) {
        if let Some(index) = STAGES.iter().position(|known| *known == name) {
            self.stage.store(index as u8 + 1, Ordering::Release);
        }
    }

    /// `open`, `busy` or `stopped`, for status.
    pub fn state(&self) -> &'static str {
        if self.stopped() {
            "stopped"
        } else if self.active() {
            "busy"
        } else {
            "open"
        }
    }

    /// Waits until no dictation holds the permit, or `deadline`.
    pub async fn idle(&self, deadline: Instant) -> bool {
        tokio::time::timeout_at(deadline, self.permit.acquire())
            .await
            .is_ok_and(|permit| permit.is_ok())
    }
}

/// Marks the permit's holder active, and inactive again when dropped.
struct Active<'a>(&'a Admission);

impl<'a> Active<'a> {
    fn new(admission: &'a Admission) -> Active<'a> {
        admission.stage.store(0, Ordering::Release);
        admission.active.store(true, Ordering::Release);
        Active(admission)
    }
}

impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.active.store(false, Ordering::Release);
        self.0.stage.store(0, Ordering::Release);
    }
}

/// A panic in the operation's task is a core failure (D9): when the task's
/// future is dropped while unwinding (tokio's behavior), admission stops
/// before the permit is released and the service is told to shut down. A
/// monitor of the task's handle reports it as well, in any case.
struct PanicGuard<'a>(&'a Frontend);

impl Drop for PanicGuard<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.admission().stop();
            self.0.core_failed();
        }
    }
}

/// Fires the operation's cancellation when the connection's handler is
/// dropped before it answered (the client went away, or the connection's
/// own bound ended it).
struct CancelOnDrop(Option<Arc<CancelHandle>>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.cancel(CancelReason::ClientDisconnected);
        }
    }
}

/// Metadata-only events of one dictation: to the service's event writer
/// under the dictation's operation id, and the coarse stage to status.
struct DictationEvents<'a> {
    reporter: &'a Reporter,
    operation_id: String,
    /// Set for the admitted operation, not for a rejection.
    admission: Option<&'a Admission>,
    frontend: &'a Frontend,
}

impl EventSink for DictationEvents<'_> {
    fn emit(
        &mut self,
        stage_name: Option<&'static str>,
        event_name: &'static str,
        elapsed_ms: Option<u64>,
        code: Option<&'static str>,
    ) {
        if event_name == event::STAGE_STARTED
            && let (Some(admission), Some(name)) = (self.admission, stage_name)
        {
            admission.set_stage(name);
            self.frontend.hook(name);
        }
        self.reporter
            .operation_event(&self.operation_id, stage_name, event_name, elapsed_ms, code);
    }
}

/// The route's handler; the guard and the route were checked already.
pub async fn handle(
    frontend: Arc<Frontend>,
    request: Request<Incoming>,
    arrival: Instant,
) -> Response<Full<Bytes>> {
    let operation_id = operation_id();
    let Some(dictations) = frontend.dictations() else {
        return reject(&frontend, operation_id, arrival, not_served()).await;
    };
    if frontend.stopping() {
        return reject(
            &frontend,
            operation_id,
            arrival,
            unavailable("shutting_down"),
        )
        .await;
    }
    let Ok(permit) = Arc::clone(&frontend.admission().permit).try_acquire_owned() else {
        return reject(&frontend, operation_id, arrival, busy()).await;
    };
    if let Some(code) = frontend.unavailable() {
        drop(permit);
        return reject(&frontend, operation_id, arrival, unavailable(code)).await;
    }

    // The operation runs on its own task, which owns the permit until its
    // cleanup is done; this handler only waits for the answer.
    let (handle, guard) = cancel::context();
    let handle = Arc::new(handle);
    let mut on_drop = CancelOnDrop(Some(Arc::clone(&handle)));
    let (answer, answered) = oneshot::channel();
    let task_frontend = Arc::clone(&frontend);
    let task = tokio::spawn(async move {
        let response = run(
            &task_frontend,
            &dictations,
            permit,
            Job {
                request,
                arrival,
                operation_id,
                handle,
                guard,
            },
        )
        .await;
        let _ = answer.send(response);
    });
    // Whatever happens to this handler, a panic of the operation's task is
    // reported (the [`PanicGuard`] already did, if the future was dropped
    // while unwinding).
    let monitor = Arc::clone(&frontend);
    tokio::spawn(async move {
        if task.await.is_err_and(|error| error.is_panic()) {
            monitor.admission().stop();
            monitor.core_failed();
        }
    });
    let response = answered.await;
    on_drop.0 = None;
    match response {
        Ok(response) => response,
        // The operation's task panicked: a core failure the service is
        // already handling.
        Err(_) => json_bytes_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            br#"{"error":{"message":"The request failed.","type":"server_error","param":null,"code":"internal"}}"#
                .to_vec(),
        ),
    }
}

/// One admitted request.
struct Job {
    request: Request<Incoming>,
    arrival: Instant,
    operation_id: String,
    handle: Arc<CancelHandle>,
    guard: CancelGuard,
}

/// The admitted operation, start to cleanup, holding the permit.
async fn run(
    frontend: &Frontend,
    dictations: &Dictations,
    permit: OwnedSemaphorePermit,
    job: Job,
) -> Response<Full<Bytes>> {
    // Dropped in reverse order: the panic guard and the activity before
    // the permit.
    let _permit = permit;
    let admission = frontend.admission();
    let _active = Active::new(admission);
    let _panic = PanicGuard(frontend);

    let Job {
        request,
        arrival,
        operation_id,
        handle,
        guard,
    } = job;
    let reporter = frontend.reporter();
    let mut events = DictationEvents {
        reporter: &reporter,
        operation_id: operation_id.clone(),
        admission: Some(admission),
        frontend,
    };
    let transcriber = WarmTranscriber::new(frontend.supervisor().clone());
    let operation = async {
        let mut operation = Operation::start(
            operation_id,
            arrival,
            None,
            guard,
            Some(&mut events as &mut dyn EventSink),
        );
        let at = operation.stage_started(stage::PREPARE);
        let prepared = prepare(dictations, request, arrival, operation.cancel()).await;
        operation.stage_done(stage::PREPARE, at, prepared.as_ref().err());
        let outcome = match prepared {
            Ok((flow, audio)) => {
                operation.set_flow(flow.flow.id.clone());
                operation
                    .run_stages(&dictations.runtime, &flow, &transcriber, &audio)
                    .await
                // The audio is dropped here, before the cleanup.
            }
            Err(error) => Err(error),
        };
        admission.set_stage(stage::CLEANUP);
        operation
            .finish(outcome, Some(dictations.runtime.scope()))
            .await
    };
    let mut output = until_shutdown(operation, frontend.shutdown_guard(), &handle).await;

    // D5: an abandoned transcription recycles its generation, confirmed
    // reaped before the permit is released.
    let mut confirmed = dictations.runtime.scope().is_empty();
    if let Some(generation) = transcriber.abandoned() {
        let supervisor = frontend.supervisor();
        supervisor.recycle(generation);
        confirmed &= supervisor
            .wait_retired(generation, Instant::now() + RECYCLE_LIMIT)
            .await;
    }
    if frontend.cleanup_unconfirmed() {
        confirmed = false;
    }
    if !confirmed {
        if admission.stop() {
            reporter.admission_stopped("cleanup_failed");
        }
        // A result is never reported over unconfirmed cleanup.
        if output.error.is_none() {
            output.result = None;
            output.error = Some(StageError::new(
                stage::CLEANUP,
                "cleanup_failed",
                "cleanup of a child process did not finish in time",
                1,
            ));
        }
    }
    envelope_response(output)
}

/// Runs the operation, firing its cancellation with `Shutdown` when the
/// service stops; the operation still runs to its own end (its cleanup).
async fn until_shutdown<F: Future<Output = RunOutput>>(
    operation: F,
    mut shutdown: CancelGuard,
    handle: &CancelHandle,
) -> RunOutput {
    tokio::pin!(operation);
    let mut watching = true;
    loop {
        tokio::select! {
            biased;
            output = &mut operation => return output,
            _ = cancel::cancelled(&mut shutdown), if watching => {
                watching = false;
                handle.cancel(CancelReason::Shutdown);
            }
        }
    }
}

/// The `prepare` stage: the bounded upload, the form and the flow.
async fn prepare(
    dictations: &Dictations,
    request: Request<Incoming>,
    arrival: Instant,
    cancel: &CancelGuard,
) -> Result<(PreparedFlow, AudioSource), StageError> {
    let (parts, body) = request.into_parts();
    let boundary = boundary(&parts.headers)?;
    if declared_length(&parts.headers).is_some_and(|length| length > MAX_BODY_BYTES) {
        return Err(body_too_large());
    }
    let upload_deadline = arrival + dictations.upload_limit();
    let mut cancelled = cancel.clone();
    let body = tokio::select! {
        biased;
        reason = cancel::cancelled(&mut cancelled) => {
            return Err(StageError::cancelled(stage::PREPARE, reason));
        }
        read = tokio::time::timeout_at(upload_deadline, read_body(body)) => match read {
            Ok(body) => body?,
            Err(_) => return Err(request_error(
                "upload_timeout",
                "The request body was not received in time.",
                None,
            )),
        },
    };
    let form = parse_form(body, boundary).await?;

    let flow = match dictations.flows.iter().find(|flow| flow.id == form.flow) {
        None => {
            return Err(request_error(
                "flow_not_found",
                "No flow with this id is configured.",
                Some("flow"),
            ));
        }
        Some(flow) if !flow.enabled => {
            return Err(request_error(
                "flow_disabled",
                "This flow is disabled.",
                Some("flow"),
            ));
        }
        Some(flow) => flow.clone(),
    };
    let deadlines = Deadlines::anchored(arrival, flow.budgets.total)?;
    if Instant::now() >= deadlines.work() {
        return Err(StageError::new(
            stage::PREPARE,
            "flow_timeout",
            "the flow's total budget is exhausted",
            1,
        ));
    }
    Ok((
        PreparedFlow { flow, deadlines },
        AudioSource::Bytes(form.file),
    ))
}

/// A request error in `prepare`: a fixed message, the route's own
/// parameter name at most.
fn request_error(
    code: &'static str,
    message: &'static str,
    param: Option<&'static str>,
) -> StageError {
    let error = StageError::new(stage::PREPARE, code, message, 2);
    match param {
        Some(param) => error.with_param(param),
        None => error,
    }
}

fn body_too_large() -> StageError {
    request_error(
        "body_too_large",
        "The request body exceeds the maximum size of 26000000 bytes.",
        None,
    )
}

fn boundary(headers: &HeaderMap) -> Result<String, StageError> {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| multer::parse_boundary(value).ok())
        .ok_or_else(|| {
            request_error(
                "not_multipart",
                "The request must be multipart/form-data with a boundary.",
                None,
            )
        })
}

fn declared_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
}

/// The whole body, epilogue included, counting the bytes that arrive
/// whatever `Content-Length` or chunking says. A broken body means the
/// client went away.
async fn read_body(mut body: Incoming) -> Result<Bytes, StageError> {
    let mut buffer = Vec::new();
    while let Some(frame) = body.frame().await {
        let Ok(frame) = frame else {
            return Err(StageError::cancelled(
                stage::PREPARE,
                CancelReason::ClientDisconnected,
            ));
        };
        if let Ok(data) = frame.into_data() {
            if (buffer.len() + data.len()) as u64 > MAX_BODY_BYTES {
                return Err(body_too_large());
            }
            buffer.extend_from_slice(&data);
        }
    }
    Ok(Bytes::from(buffer))
}

/// A parsed upload. No `Debug`: `file` is the user's audio.
struct Form {
    file: Bytes,
    flow: String,
}

/// A single-chunk stream over the received body, for the parser.
struct Once(Option<Bytes>);

impl futures_core::Stream for Once {
    type Item = Result<Bytes, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.0.take().map(Ok))
    }
}

/// Exactly one non-empty uploaded `file` and one text `flow`, in either
/// order, nothing else. No message quotes a field name, file name or
/// value the client sent.
async fn parse_form(body: Bytes, boundary: String) -> Result<Form, StageError> {
    let limits = multer::SizeLimit::new()
        .whole_stream(MAX_BODY_BYTES)
        .per_field(MAX_SCALAR_BYTES)
        .for_field("file", MAX_FILE_BYTES)
        .for_field("flow", MAX_FLOW_BYTES);
    let constraints = multer::Constraints::new().size_limit(limits);
    let mut multipart =
        multer::Multipart::with_constraints(Once(Some(body)), boundary, constraints);

    let mut file = None;
    let mut flow = None;
    while let Some(field) = multipart.next_field().await.map_err(multer_error)? {
        let upload = field.file_name().is_some();
        let param = match field.name() {
            Some("file") => "file",
            Some("flow") => "flow",
            _ => {
                return Err(request_error(
                    "unsupported_parameter",
                    "Unrecognized request parameter. Supported: file, flow.",
                    None,
                ));
            }
        };
        let seen = if param == "file" {
            file.is_some()
        } else {
            flow.is_some()
        };
        if seen {
            return Err(request_error(
                "duplicate_parameter",
                "A request parameter was sent more than once.",
                Some(param),
            ));
        }
        if param == "file" {
            if !upload {
                return Err(request_error(
                    "file_not_upload",
                    "The file parameter must be sent as a file upload.",
                    Some("file"),
                ));
            }
            let data = field.bytes().await.map_err(multer_error)?;
            if data.is_empty() {
                return Err(request_error(
                    "empty_file",
                    "The uploaded file is empty.",
                    Some("file"),
                ));
            }
            file = Some(data);
            continue;
        }
        if upload {
            return Err(request_error(
                "flow_not_text",
                "The flow parameter must be sent as a text field, not a file.",
                Some("flow"),
            ));
        }
        let data = field.bytes().await.map_err(multer_error)?;
        let Ok(value) = String::from_utf8(data.to_vec()) else {
            return Err(request_error(
                "invalid_utf8",
                "A text parameter is not valid UTF-8.",
                Some("flow"),
            ));
        };
        if value.contains('\0') {
            return Err(request_error(
                "nul_in_text",
                "A text parameter contains a NUL character.",
                Some("flow"),
            ));
        }
        flow = Some(value);
    }
    let Some(file) = file else {
        return Err(request_error(
            "missing_file",
            "You must provide a file parameter.",
            Some("file"),
        ));
    };
    let Some(flow) = flow else {
        return Err(request_error(
            "missing_flow",
            "You must provide a flow parameter.",
            Some("flow"),
        ));
    };
    Ok(Form { file, flow })
}

fn multer_error(error: multer::Error) -> StageError {
    match error {
        multer::Error::FieldSizeExceeded { field_name, .. } => match field_name.as_deref() {
            Some("file") => request_error(
                "file_too_large",
                "The uploaded file exceeds the maximum size of 25000000 bytes.",
                Some("file"),
            ),
            Some("flow") => request_error(
                "field_too_large",
                "The flow parameter exceeds the maximum size of 128 bytes.",
                Some("flow"),
            ),
            _ => request_error(
                "field_too_large",
                "A text field exceeds the maximum size of 16384 bytes.",
                None,
            ),
        },
        multer::Error::StreamSizeExceeded { .. } => body_too_large(),
        _ => request_error(
            "malformed_multipart",
            "The multipart request body is malformed.",
            None,
        ),
    }
}

fn not_served() -> StageError {
    StageError::new(stage::PREPARE, "not_found", "Unknown route.", 2)
}

fn busy() -> StageError {
    StageError::new(
        stage::PREPARE,
        "rate_limit_exceeded",
        "Another dictation is running. Retry after a short wait.",
        1,
    )
}

fn unavailable(code: &'static str) -> StageError {
    let message = match code {
        "service_starting" => "The service is starting; the transcription model is loading.",
        "transcription_restarting" => "The transcription module is restarting.",
        "transcription_failed" => {
            "The transcription module failed and is not restarted; restart pumice serve."
        }
        "admission_stopped" => {
            "A cleanup could not be confirmed; dictations are stopped until pumice serve is restarted."
        }
        _ => "The service is shutting down.",
    };
    let code = match code {
        "service_starting" => "service_starting",
        "transcription_restarting" => "transcription_restarting",
        "transcription_failed" => "transcription_failed",
        "admission_stopped" => "admission_stopped",
        _ => "shutting_down",
    };
    StageError::new(stage::PREPARE, code, message, 1)
}

/// A request refused before its body was read: the same envelope, with
/// every stage skipped and no flow.
async fn reject(
    frontend: &Frontend,
    operation_id: String,
    arrival: Instant,
    error: StageError,
) -> Response<Full<Bytes>> {
    let reporter = frontend.reporter();
    let mut events = DictationEvents {
        reporter: &reporter,
        operation_id: operation_id.clone(),
        admission: None,
        frontend,
    };
    let (_never, guard) = cancel::context();
    let operation = Operation::start(
        operation_id,
        arrival,
        None,
        guard,
        Some(&mut events as &mut dyn EventSink),
    );
    let output = operation.finish(Err(error), None).await;
    envelope_response(output)
}

/// The envelope as the answer: its status, `Retry-After: 1` for a local
/// busy refusal, and the 16 MiB bound (design §5).
fn envelope_response(output: RunOutput) -> Response<Full<Bytes>> {
    let busy = output
        .error
        .as_ref()
        .is_some_and(|error| error.code == "rate_limit_exceeded");
    let (status, envelope) = output.into_http_envelope();
    let (status, bytes) = match serde_json::to_vec(&envelope) {
        Ok(bytes) if bytes.len() <= MAX_RESPONSE_BYTES => (status, Some(bytes)),
        Ok(_) if envelope.result.is_some() => output_too_large(envelope),
        _ => (500, None),
    };
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let Some(bytes) = bytes else {
        return json_bytes_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            br#"{"error":{"message":"The response could not be produced.","type":"server_error","param":null,"code":"internal"}}"#
                .to_vec(),
        );
    };
    let mut response = json_bytes_response(status, bytes);
    if busy {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    }
    response
}

/// A final text too large to return fails the relay stage; the raw
/// transcript (at most the child's 1 MiB result) is kept.
fn output_too_large(mut envelope: RunEnvelope<HttpFlowError>) -> (u16, Option<Vec<u8>>) {
    let error = StageError::new(
        stage::RELAY,
        "relay_invalid_output",
        "the provider's reply is too large to return",
        1,
    )
    .with_cause("output_too_large");
    let (class, error) = http_error(&error);
    envelope.ok = false;
    envelope.result = None;
    envelope.stages.relay.status = StageStatus::Error;
    envelope.error = Some(error);
    match serde_json::to_vec(&envelope) {
        Ok(bytes) if bytes.len() <= MAX_RESPONSE_BYTES => (class.status, Some(bytes)),
        _ => (500, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::config::Budgets;

    fn flow(id: &str, enabled: bool, total_ms: u64) -> Flow {
        Flow {
            id: id.to_owned(),
            enabled,
            transcription_model: "turbo".to_owned(),
            formatter: None,
            relay_route: "passthrough".to_owned(),
            budgets: Budgets {
                total: Duration::from_millis(total_ms),
                ..Budgets::default()
            },
            references: Default::default(),
        }
    }

    fn form(parts: &[(&str, Option<&str>, &[u8])]) -> (Bytes, String) {
        let boundary = "XyZ".to_owned();
        let mut body = Vec::new();
        for (name, file_name, data) in parts {
            body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            match file_name {
                Some(file_name) => body.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{name}\"; filename=\"{file_name}\"\r\n\r\n"
                    )
                    .as_bytes(),
                ),
                None => body.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
                ),
            }
            body.extend_from_slice(data);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        (Bytes::from(body), boundary)
    }

    fn parse(parts: &[(&str, Option<&str>, &[u8])]) -> Result<Form, StageError> {
        let (body, boundary) = form(parts);
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(parse_form(body, boundary))
    }

    fn code(parts: &[(&str, Option<&str>, &[u8])]) -> (&'static str, Option<&'static str>) {
        match parse(parts) {
            Ok(_) => ("ok", None),
            Err(error) => (error.code, error.param),
        }
    }

    #[test]
    fn upload_limits_follow_the_shortest_enabled_flow() {
        assert_eq!(upload_limit(&[]), UPLOAD_LIMIT);
        let long = [flow("a", true, 180_000)];
        assert_eq!(upload_limit(&long), UPLOAD_LIMIT);
        assert_eq!(
            policy(&long).connection_limit,
            Duration::from_secs(180) + CONNECTION_MARGIN
        );
        assert_eq!(policy(&long).max_response_bytes, MAX_RESPONSE_BYTES);
        // A short enabled flow narrows the shared upload window; a short
        // disabled one does not.
        let short = [
            flow("a", true, 180_000),
            flow("b", true, 6_000),
            flow("c", false, 3_000),
        ];
        assert_eq!(upload_limit(&short), Duration::from_millis(3_500));
    }

    #[test]
    fn a_valid_form_in_either_order() {
        for parts in [
            vec![
                ("file", Some("x.wav"), &b"RIFF"[..]),
                ("flow", None, b"clean"),
            ],
            vec![("flow", None, &b"clean"[..]), ("file", Some(""), b"RIFF")],
        ] {
            let form = parse(&parts).unwrap_or_else(|_| panic!("valid"));
            assert_eq!(&form.file[..], b"RIFF");
            assert_eq!(form.flow, "clean");
        }
    }

    #[test]
    fn invalid_forms_get_fixed_codes() {
        let file = ("file", Some("x.wav"), &b"RIFF"[..]);
        let flow = ("flow", None, &b"clean"[..]);
        assert_eq!(code(&[flow]), ("missing_file", Some("file")));
        assert_eq!(code(&[file]), ("missing_flow", Some("flow")));
        assert_eq!(
            code(&[file, flow, file]),
            ("duplicate_parameter", Some("file"))
        );
        assert_eq!(
            code(&[file, flow, flow]),
            ("duplicate_parameter", Some("flow"))
        );
        assert_eq!(
            code(&[file, flow, ("model", None, b"turbo")]),
            ("unsupported_parameter", None)
        );
        assert_eq!(
            code(&[("file", Some("x.wav"), b""), flow]),
            ("empty_file", Some("file"))
        );
        assert_eq!(
            code(&[("file", None, b"RIFF"), flow]),
            ("file_not_upload", Some("file"))
        );
        assert_eq!(
            code(&[file, ("flow", Some("f.txt"), b"clean")]),
            ("flow_not_text", Some("flow"))
        );
        assert_eq!(
            code(&[file, ("flow", None, b"\xff\xfe")]),
            ("invalid_utf8", Some("flow"))
        );
        assert_eq!(
            code(&[file, ("flow", None, b"cl\0ean")]),
            ("nul_in_text", Some("flow"))
        );
        let long = vec![b'a'; 129];
        assert_eq!(
            code(&[file, ("flow", None, &long)]),
            ("field_too_large", Some("flow"))
        );
        let fits = vec![b'a'; 128];
        assert_eq!(code(&[file, ("flow", None, &fits)]), ("ok", None));
        // Not multipart at all.
        let result = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(parse_form(Bytes::from_static(b"garbage"), "XyZ".to_owned()));
        assert_eq!(
            result.err().map(|error| error.code),
            Some("malformed_multipart")
        );
    }

    #[test]
    fn messages_never_quote_the_request() {
        let sentinel = "S3NT1NEL";
        for parts in [
            vec![(sentinel, None, &b"x"[..])],
            vec![("file", Some(sentinel), &b""[..])],
            vec![
                ("file", Some("a"), &b"x"[..]),
                ("flow", Some(sentinel), b"x"),
            ],
        ] {
            let error = parse(&parts).err().expect("refused");
            assert!(!error.message.contains(sentinel), "{}", error.message);
        }
    }

    #[test]
    fn the_admission_reports_its_state_and_stage() {
        let admission = Admission::default();
        assert_eq!(admission.state(), "open");
        assert_eq!(admission.stage(), None);
        {
            let _active = Active::new(&admission);
            assert_eq!(admission.state(), "busy");
            admission.set_stage(stage::RELAY);
            assert_eq!(admission.stage(), Some("relay"));
        }
        assert_eq!(admission.state(), "open");
        assert_eq!(admission.stage(), None);
        assert!(admission.stop());
        assert!(!admission.stop(), "reported once");
        assert_eq!(admission.state(), "stopped");
    }
}
