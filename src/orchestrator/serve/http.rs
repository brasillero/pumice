//! `pumice serve`'s loopback frontend (p13.1-flow-seam/D1, D2, D7, D8).
//!
//! Routes: `GET /health`, `GET /ready`, `GET /pumice/v1/status`,
//! `GET /pumice/v1/flows`, `POST /pumice/v1/dictations` (`dictation`) and
//! the OpenAI `GET /v1/models`, `GET /v1/models/{id}` and
//! `POST /v1/audio/transcriptions` (`transcriptions`,
//! p15-serve-transcriptions/D1, which narrows D2). There is no
//! `/v1/chat/completions` here (p15/D7).
//!
//! Before anything else, every request passes the origin guard (D8): the
//! `Host` must be the literal `127.0.0.1:<port>`, a present `Origin` must
//! equal `http://127.0.0.1:<port>` and a present `Sec-Fetch-Site` must be
//! `same-origin` or `none`, otherwise 403. No CORS headers are ever
//! sent. Every answer is JSON with `Cache-Control: no-store` (a
//! transcription asked for as `text` is plain text), errors use the
//! OpenAI error object (`message`, `type`, `param`, `code`), including 404
//! and 405 (with `Allow`). On the dictation route every answer, its guard
//! and method refusals included, is the versioned dictation envelope
//! (design §5), whose `error` is that same object with a few more fields.
//!
//! Bounds (design §7): at most [`MAX_CONNECTIONS`] connections (more are
//! closed at once), one request per connection and request headers within
//! [`HEADER_READ_TIMEOUT`] and [`MAX_HEADER_BYTES`]. Past the head, the
//! route's [`RoutePolicy`] bounds the rest of the connection: the metadata
//! routes (and every rejection) use [`METADATA`], a whole connection within
//! 10 s and answers of at most 128 KiB; the dictation route brings its own
//! policy ([`dictation::policy`]), which the transcription route shares.
//! Metadata handlers, the model routes included, only read in-memory
//! snapshots: they never wait for the child, the network, a CLI, a model or
//! the dictations' work permit.
//!
//! A client that closes its connection while a dictation runs ends the
//! connection (hyper notices the end of input while the answer is
//! pending), which drops the handler and cancels the operation
//! (`dictation`, cause `client_disconnected`). A write-half-close counts
//! the same (p13.4-dictations/D1): half-close support stays off, so a
//! client must keep its connection open, write side included, until the
//! answer arrives. When the service stops, running dictations are
//! cancelled and their connections get a bounded drain to send their
//! answers ([`DRAIN_LIMIT`]).

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::Full;
use hyper::header::{self, HeaderMap, HeaderValue};
use hyper::{Method, Request, Response, StatusCode, Uri};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinSet;
use tokio::time::Instant;

use crate::orchestrator::cancel::{self, CancelGuard, CancelHandle, CancelReason};
use crate::orchestrator::error::StageError;
use crate::orchestrator::result::{MODULE, SCHEMA_VERSION, stage};
use crate::orchestrator::serve::dictation::{self, Admission, Dictations};
use crate::orchestrator::serve::output::Reporter;
use crate::orchestrator::serve::supervisor::{ChildState, SupervisorHandle};
use crate::orchestrator::serve::transcriptions::{self, MODELS, TRANSCRIPTIONS};

pub const MAX_CONNECTIONS: usize = 64;
pub const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(5);
/// hyper's smallest read buffer, which also bounds the request head.
pub const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_HEADERS: usize = 64;

/// What bounds a routed request's connection once its head is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoutePolicy {
    /// From the connection's arrival to its end: the request, its handling
    /// and the answer.
    pub connection_limit: Duration,
    /// The largest answer body.
    pub max_response_bytes: usize,
}

/// The metadata routes' policy (design §7), also used for every answer
/// given before a route is chosen (guard rejections, 404, 405) and as the
/// bound while the head is still being read.
pub const METADATA: RoutePolicy = RoutePolicy {
    connection_limit: Duration::from_secs(10),
    max_response_bytes: 128 * 1024,
};
/// A failed `accept` (for example, out of file descriptors) is retried
/// after this pause instead of spinning.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(50);
/// After the stop, open connections and a running dictation's cleanup get
/// this long to finish (a cancelled dictation still answers 503 with its
/// transcript), then the rest is dropped.
pub const DRAIN_LIMIT: Duration = Duration::from_millis(2_500);
/// The dictation route.
pub const DICTATIONS: &str = "/pumice/v1/dictations";

/// The service's own state, as `/ready` and status report it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    /// Listening; the first transcription child is not ready yet.
    Starting,
    Running,
    Stopping,
}

/// One enabled flow as `/pumice/v1/flows` lists it.
#[derive(Clone, Debug, Serialize)]
pub struct FlowInfo {
    pub id: String,
    pub transcription_model: String,
    pub formatter: Option<String>,
    pub relay_route: String,
}

/// Test seam, never set by `pumice serve` itself: called with the path of
/// every request that passed the guard, before it is routed, and with
/// `dictation:<stage>` when a dictation starts a stage, so in-process tests
/// can inject a panic into a request handler or a dictation's task.
#[doc(hidden)]
pub type RequestHook = Arc<dyn Fn(&str) + Send + Sync>;

/// Test seam, never set by `pumice serve` itself: when it returns true, a
/// dictation's cleanup counts as unconfirmed.
#[doc(hidden)]
pub type CleanupHook = Arc<dyn Fn() -> bool + Send + Sync>;

/// What the routes read.
pub struct Frontend {
    port: u16,
    state: AtomicU8,
    supervisor: SupervisorHandle,
    model: String,
    flows: Vec<FlowInfo>,
    dictations: Option<Arc<Dictations>>,
    admission: Admission,
    reporter: Reporter,
    /// Fired when the service stops: running dictations are cancelled.
    shutdown: (CancelHandle, CancelGuard),
    /// Set when a dictation's task panicked (a core failure).
    failed: watch::Sender<bool>,
    request_hook: Option<RequestHook>,
    cleanup_hook: Option<CleanupHook>,
}

impl Frontend {
    pub fn new(
        port: u16,
        supervisor: SupervisorHandle,
        model: String,
        flows: Vec<FlowInfo>,
    ) -> Frontend {
        Frontend {
            port,
            state: AtomicU8::new(ServiceState::Starting as u8),
            supervisor,
            model,
            flows,
            dictations: None,
            admission: Admission::default(),
            reporter: Reporter::silent(),
            shutdown: cancel::context(),
            failed: watch::channel(false).0,
            request_hook: None,
            cleanup_hook: None,
        }
    }

    /// Serves `POST /pumice/v1/dictations` (404 without it).
    pub fn set_dictations(&mut self, dictations: Dictations) {
        self.dictations = Some(Arc::new(dictations));
    }

    /// Where dictations report their metadata-only events.
    pub fn set_reporter(&mut self, reporter: Reporter) {
        self.reporter = reporter;
    }

    #[doc(hidden)]
    pub fn set_request_hook(&mut self, hook: RequestHook) {
        self.request_hook = Some(hook);
    }

    #[doc(hidden)]
    pub fn set_cleanup_hook(&mut self, hook: CleanupHook) {
        self.cleanup_hook = Some(hook);
    }

    pub(crate) fn dictations(&self) -> Option<Arc<Dictations>> {
        self.dictations.clone()
    }

    /// The enabled flows.
    pub(crate) fn flow_infos(&self) -> &[FlowInfo] {
        &self.flows
    }

    /// The flows published as transcription models; none without the
    /// dictation route's runtime.
    pub(crate) fn published(&self) -> &[String] {
        self.dictations
            .as_deref()
            .map_or(&[], |dictations| &dictations.transcription_flows)
    }

    pub(crate) fn admission(&self) -> &Admission {
        &self.admission
    }

    pub(crate) fn reporter(&self) -> Reporter {
        self.reporter.clone()
    }

    pub(crate) fn supervisor(&self) -> &SupervisorHandle {
        &self.supervisor
    }

    pub(crate) fn stopping(&self) -> bool {
        self.state() == ServiceState::Stopping
    }

    pub(crate) fn shutdown_guard(&self) -> CancelGuard {
        self.shutdown.1.clone()
    }

    /// Stops admission and cancels a running dictation: the service is
    /// stopping. Idempotent.
    pub fn begin_shutdown(&self) {
        self.set_state(ServiceState::Stopping);
        self.shutdown.0.cancel(CancelReason::Shutdown);
    }

    /// A dictation's task panicked: [`serve`] stops at once.
    pub(crate) fn core_failed(&self) {
        self.failed.send_replace(true);
    }

    pub(crate) fn hook(&self, stage: &str) {
        if let Some(hook) = &self.request_hook {
            hook(&format!("dictation:{stage}"));
        }
    }

    pub(crate) fn cleanup_unconfirmed(&self) -> bool {
        self.cleanup_hook.as_ref().is_some_and(|hook| hook())
    }

    pub fn set_state(&self, state: ServiceState) {
        self.state.store(state as u8, Ordering::Release);
    }

    pub fn state(&self) -> ServiceState {
        match self.state.load(Ordering::Acquire) {
            0 => ServiceState::Starting,
            1 => ServiceState::Running,
            _ => ServiceState::Stopping,
        }
    }

    /// Why the service is not ready, or `None` when it is.
    pub(crate) fn unavailable(&self) -> Option<&'static str> {
        match self.state() {
            ServiceState::Stopping => return Some("shutting_down"),
            ServiceState::Starting => return Some("service_starting"),
            ServiceState::Running => {}
        }
        if self.admission.stopped() {
            return Some("admission_stopped");
        }
        match self.supervisor.snapshot().state {
            ChildState::Ready => None,
            ChildState::Starting | ChildState::Restarting => Some("transcription_restarting"),
            ChildState::Failed => Some("transcription_failed"),
            ChildState::Stopping => Some("shutting_down"),
        }
    }
}

/// A request handler panicked: a core failure (p13.1-flow-seam/D9).
#[derive(Debug, PartialEq, Eq)]
pub struct HandlerPanicked;

/// Serves `listener` until `stop` fires, then stops accepting, cancels
/// running dictations and gives open connections [`DRAIN_LIMIT`] to
/// answer before dropping them. Never panics on client behavior; when a
/// connection's or a dictation's task panics anyway, it drops every
/// connection at once and returns [`HandlerPanicked`] for the service to
/// shut down.
pub async fn serve(
    listener: TcpListener,
    frontend: Arc<Frontend>,
    mut stop: CancelGuard,
) -> Result<(), HandlerPanicked> {
    let permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let mut connections = JoinSet::new();
    let mut failed = frontend.failed.subscribe();
    let mut result = Ok(());
    loop {
        tokio::select! {
            biased;
            _ = cancel::cancelled(&mut stop) => break,
            () = core_failure(&mut failed) => {
                result = Err(HandlerPanicked);
                break;
            }
            Some(joined) = connections.join_next(), if !connections.is_empty() => {
                if joined.is_err_and(|error| error.is_panic()) {
                    result = Err(HandlerPanicked);
                    break;
                }
            }
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    // Over the cap the connection is closed at once: no
                    // queued work is ever created for it.
                    let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                        drop(stream);
                        continue;
                    };
                    let frontend = Arc::clone(&frontend);
                    connections.spawn(async move {
                        let _permit = permit;
                        serve_connection(stream, frontend).await;
                    });
                }
                Err(_) => tokio::time::sleep(ACCEPT_BACKOFF).await,
            },
        }
    }
    drop(listener);
    frontend.begin_shutdown();
    if result.is_ok() {
        // Cancelled dictations answer and clean up within the drain.
        let deadline = Instant::now() + DRAIN_LIMIT;
        loop {
            tokio::select! {
                biased;
                () = core_failure(&mut failed) => {
                    result = Err(HandlerPanicked);
                    break;
                }
                joined = connections.join_next() => match joined {
                    Some(Err(error)) if error.is_panic() => {
                        result = Err(HandlerPanicked);
                        break;
                    }
                    Some(_) => {}
                    None => break,
                },
                () = tokio::time::sleep_until(deadline) => break,
            }
        }
        if result.is_ok() {
            let _ = frontend.admission.idle(deadline).await;
        }
    }
    connections.abort_all();
    while connections.join_next().await.is_some() {}
    result
}

/// Resolves once a dictation's task reported a panic.
async fn core_failure(failed: &mut watch::Receiver<bool>) {
    if failed.wait_for(|failed| *failed).await.is_err() {
        std::future::pending::<()>().await;
    }
}

type Answer = Pin<Box<dyn Future<Output = Result<Response<Full<Bytes>>, Infallible>> + Send>>;

async fn serve_connection(stream: tokio::net::TcpStream, frontend: Arc<Frontend>) {
    let arrival = Instant::now();
    // The request's route publishes its policy as soon as it is chosen.
    let (routed, policy) = watch::channel(None);
    let service = hyper::service::service_fn(move |request: Request<hyper::body::Incoming>| {
        let answer: Answer = match route(
            &frontend,
            request.method(),
            request.uri(),
            request.headers(),
        ) {
            Routed::Answer(policy, response) => {
                routed.send_replace(Some(policy));
                Box::pin(async move { Ok(response) })
            }
            Routed::Dictation(policy) => {
                routed.send_replace(Some(policy));
                let frontend = Arc::clone(&frontend);
                Box::pin(async move { Ok(dictation::handle(frontend, request, arrival).await) })
            }
            Routed::Refused(error) => {
                routed.send_replace(Some(METADATA));
                let frontend = Arc::clone(&frontend);
                Box::pin(async move { Ok(dictation::refuse(&frontend, arrival, error).await) })
            }
            Routed::Transcription(policy) => {
                routed.send_replace(Some(policy));
                let frontend = Arc::clone(&frontend);
                Box::pin(
                    async move { Ok(transcriptions::handle(frontend, request, arrival).await) },
                )
            }
        };
        answer
    });
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT)
        .max_buf_size(MAX_HEADER_BYTES)
        .max_headers(MAX_HEADERS)
        .keep_alive(false)
        // p13.4-dictations/D1: the end of a client's input while its
        // answer is pending ends the connection and so cancels a running
        // dictation, also after a deliberate write-half-close.
        .half_close(false);
    let connection = builder.serve_connection(hyper_util::rt::TokioIo::new(stream), service);
    drive(connection, arrival, METADATA.connection_limit, policy).await;
}

/// Runs one connection until it ends or its deadline passes, both counted
/// from `arrival`: `unrouted` until its request is routed (hyper already
/// closes a connection whose head takes longer than the header-read
/// bound), then the route's own [`RoutePolicy::connection_limit`].
async fn drive(
    connection: impl std::future::Future,
    arrival: Instant,
    unrouted: Duration,
    mut routed: watch::Receiver<Option<RoutePolicy>>,
) {
    let mut connection = std::pin::pin!(connection);
    let mut deadline = arrival + unrouted;
    let mut routing = true;
    loop {
        tokio::select! {
            biased;
            _ = &mut connection => return,
            changed = routed.changed(), if routing => match changed {
                Ok(()) => {
                    if let Some(policy) = *routed.borrow_and_update() {
                        deadline = arrival + policy.connection_limit;
                    }
                }
                Err(_) => routing = false,
            },
            () = tokio::time::sleep_until(deadline) => return,
        }
    }
}

/// The whole request handling of the snapshot routes: guard, route,
/// answer. Synchronous: it reads snapshots only. The dictation route needs
/// its connection ([`serve`]); here it answers 500.
pub fn respond(
    frontend: &Frontend,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
) -> Response<Full<Bytes>> {
    match route(frontend, method, uri, headers) {
        Routed::Answer(_, response) => response,
        Routed::Dictation(_) | Routed::Refused(_) | Routed::Transcription(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "server_error",
            "internal",
            "This route needs a connection.",
        ),
    }
}

/// Where a request goes, with the policy that bounds the rest of its
/// connection.
enum Routed {
    Answer(RoutePolicy, Response<Full<Bytes>>),
    Dictation(RoutePolicy),
    /// The dictation route's guard or method refusal: answered with the
    /// dictation envelope, under [`METADATA`].
    Refused(StageError),
    /// `POST /v1/audio/transcriptions`, under the dictation route's policy.
    Transcription(RoutePolicy),
}

fn route(frontend: &Frontend, method: &Method, uri: &Uri, headers: &HeaderMap) -> Routed {
    let dictations = match &frontend.dictations {
        Some(dictations) if uri.path() == DICTATIONS => dictations,
        Some(dictations) if uri.path() == TRANSCRIPTIONS => {
            return transcription_route(frontend, dictations, method, uri, headers);
        }
        // Everything else, and the guard's rejections, are snapshots.
        _ => {
            let (policy, response) = answer(frontend, method, uri, headers);
            return Routed::Answer(policy, response);
        }
    };
    // The guard first, then the method, as on every route.
    if let Err(rejection) = check_origin(frontend.port, uri, headers) {
        let (code, message) = rejection.describe();
        return Routed::Refused(StageError::new(stage::PREPARE, code, message, 2));
    }
    if let Some(hook) = &frontend.request_hook {
        hook(uri.path());
    }
    if method != Method::POST {
        return Routed::Refused(StageError::new(
            stage::PREPARE,
            "method_not_allowed",
            "This route only accepts POST.",
            2,
        ));
    }
    Routed::Dictation(dictations.policy())
}

/// The transcription route: the guard, then the method, refused with the
/// OpenAI error object like the snapshot routes.
fn transcription_route(
    frontend: &Frontend,
    dictations: &Dictations,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
) -> Routed {
    if let Err(rejection) = check_origin(frontend.port, uri, headers) {
        let (code, message) = rejection.describe();
        return Routed::Answer(
            METADATA,
            error(
                StatusCode::FORBIDDEN,
                "invalid_request_error",
                code,
                message,
            ),
        );
    }
    if let Some(hook) = &frontend.request_hook {
        hook(uri.path());
    }
    if method != Method::POST {
        let mut response = error(
            StatusCode::METHOD_NOT_ALLOWED,
            "invalid_request_error",
            "method_not_allowed",
            "This route only accepts POST.",
        );
        response
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("POST"));
        return Routed::Answer(METADATA, response);
    }
    Routed::Transcription(dictations.policy())
}

/// The snapshot routes and every rejection, with the policy that bounds the
/// rest of the connection.
fn answer(
    frontend: &Frontend,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
) -> (RoutePolicy, Response<Full<Bytes>>) {
    if let Err(rejection) = check_origin(frontend.port, uri, headers) {
        let (code, message) = rejection.describe();
        return (
            METADATA,
            error(
                StatusCode::FORBIDDEN,
                "invalid_request_error",
                code,
                message,
            ),
        );
    }
    if let Some(hook) = &frontend.request_hook {
        hook(uri.path());
    }
    let route = match uri.path() {
        "/health" => Route::Health,
        "/ready" => Route::Ready,
        "/pumice/v1/status" => Route::Status,
        "/pumice/v1/flows" => Route::Flows,
        MODELS => Route::Models,
        path if path.starts_with(MODELS) && path.as_bytes().get(MODELS.len()) == Some(&b'/') => {
            Route::Model(&path[MODELS.len() + 1..])
        }
        _ => {
            return (
                METADATA,
                error(
                    StatusCode::NOT_FOUND,
                    "invalid_request_error",
                    "not_found",
                    "Unknown route.",
                ),
            );
        }
    };
    if method != Method::GET {
        let mut response = error(
            StatusCode::METHOD_NOT_ALLOWED,
            "invalid_request_error",
            "method_not_allowed",
            "This route only accepts GET.",
        );
        response
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("GET"));
        return (METADATA, response);
    }
    let response = match route {
        Route::Health => json_response(
            StatusCode::OK,
            &json!({"status": "ok", "version": env!("CARGO_PKG_VERSION")}),
        ),
        Route::Ready => match frontend.unavailable() {
            None => json_response(
                StatusCode::OK,
                &json!({
                    "schema_version": SCHEMA_VERSION,
                    "module": MODULE,
                    "version": env!("CARGO_PKG_VERSION"),
                    "state": "ready",
                }),
            ),
            Some(code) => error(
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                code,
                not_ready_message(code),
            ),
        },
        Route::Status => json_response(StatusCode::OK, &status(frontend)),
        Route::Flows => json_response(StatusCode::OK, &flows(frontend)),
        Route::Models => json_response(StatusCode::OK, &transcriptions::models(frontend)),
        Route::Model(id) => match transcriptions::model(frontend, id) {
            Some(model) => json_response(StatusCode::OK, &model),
            None => json_response(
                StatusCode::NOT_FOUND,
                &json!({
                    "error": {
                        "message": "The requested model does not exist on this server.",
                        "type": "invalid_request_error",
                        "param": "model",
                        "code": "model_not_found",
                    }
                }),
            ),
        },
    };
    (route.policy(), response)
}

enum Route<'a> {
    Health,
    Ready,
    Status,
    Flows,
    Models,
    /// One model; the id as it appears in the path (published ids need no
    /// escaping, so anything else is simply unknown).
    Model(&'a str),
}

impl Route<'_> {
    fn policy(&self) -> RoutePolicy {
        match self {
            Route::Health
            | Route::Ready
            | Route::Status
            | Route::Flows
            | Route::Models
            | Route::Model(_) => METADATA,
        }
    }
}

fn not_ready_message(code: &str) -> &'static str {
    match code {
        "service_starting" => "The service is starting; the transcription model is loading.",
        "transcription_restarting" => "The transcription module is restarting.",
        "transcription_failed" => {
            "The transcription module failed and is not restarted; restart pumice serve."
        }
        "admission_stopped" => {
            "A cleanup could not be confirmed; dictations are stopped until pumice serve is restarted."
        }
        _ => "The service is shutting down.",
    }
}

/// `GET /pumice/v1/status`, schema 1: no paths, no private child address,
/// no content.
fn status(frontend: &Frontend) -> Value {
    let child = frontend.supervisor.snapshot();
    json!({
        "schema_version": SCHEMA_VERSION,
        "module": MODULE,
        "version": env!("CARGO_PKG_VERSION"),
        "service": {
            "state": frontend.state(),
            "ready": frontend.unavailable().is_none(),
        },
        "transcription": {
            "state": child.state,
            "model": frontend.model,
            "generation": child.generation,
            "restart_attempts": child.restart_attempts,
            "next_retry_delay_ms": child.next_retry_delay_ms,
            "model_load_ms": child.model_load_ms,
            "compute": child.compute,
            "last_failure": child.last_failure,
        },
        "flows": {
            "enabled": frontend.flows.len(),
            "published": frontend.published().len(),
        },
        "dictations": {
            "served": frontend.dictations.is_some(),
            "admission": frontend.admission.state(),
            "active": frontend.admission.active(),
            "stage": frontend.admission.stage(),
        },
    })
}

/// `GET /pumice/v1/flows`, schema 1: the enabled flows and whether each can
/// run now, with a safe code when not. No formatter rules, no paths, no
/// disabled flows.
fn flows(frontend: &Frontend) -> Value {
    let unavailable = frontend.unavailable();
    let flows: Vec<Value> = frontend
        .flows
        .iter()
        .map(|flow| {
            let mut entry = json!({
                "id": flow.id,
                "transcription_model": flow.transcription_model,
                "formatter": flow.formatter,
                "relay_route": flow.relay_route,
                "ready": unavailable.is_none(),
            });
            if let Some(code) = unavailable {
                entry["unavailable"] = json!(code);
            }
            entry
        })
        .collect();
    json!({
        "schema_version": SCHEMA_VERSION,
        "module": MODULE,
        "flows": flows,
    })
}

/// Which part of the D8 guard refused a request.
enum Rejected {
    Host,
    Origin,
    FetchSite,
}

impl Rejected {
    /// The refusal's code and fixed message.
    fn describe(&self) -> (&'static str, &'static str) {
        match self {
            Rejected::Host => (
                "host_not_allowed",
                "This service only accepts the Host 127.0.0.1 with its own port.",
            ),
            Rejected::Origin => (
                "origin_not_allowed",
                "This service only accepts requests from its own origin.",
            ),
            Rejected::FetchSite => (
                "cross_site_not_allowed",
                "This service does not accept cross-site browser requests.",
            ),
        }
    }
}

/// The D8 guard: exactly one `Host`, the literal `127.0.0.1:<port>` (an
/// absolute request target must name the same authority), no `Origin` but
/// this listener's own, and no cross-site fetch metadata (design §9): a
/// present `Sec-Fetch-Site` must be `same-origin` or `none` (typed by the
/// user). Native clients send neither header.
fn check_origin(port: u16, uri: &Uri, headers: &HeaderMap) -> Result<(), Rejected> {
    let authority = format!("127.0.0.1:{port}");
    let mut hosts = headers.get_all(header::HOST).iter();
    let host_ok = matches!(
        (hosts.next(), hosts.next()),
        (Some(host), None) if host.as_bytes() == authority.as_bytes()
    ) && uri
        .authority()
        .is_none_or(|uri_authority| uri_authority.as_str() == authority)
        && uri
            .scheme()
            .is_none_or(|scheme| scheme == &hyper::http::uri::Scheme::HTTP);
    if !host_ok {
        return Err(Rejected::Host);
    }
    let origin = format!("http://{authority}");
    let mut origins = headers.get_all(header::ORIGIN).iter();
    let origin_ok = match (origins.next(), origins.next()) {
        (None, _) => true,
        (Some(value), None) => value.as_bytes() == origin.as_bytes(),
        (Some(_), Some(_)) => false,
    };
    if !origin_ok {
        return Err(Rejected::Origin);
    }
    let mut sites = headers.get_all("sec-fetch-site").iter();
    let site_ok = match (sites.next(), sites.next()) {
        (None, _) => true,
        (Some(value), None) => matches!(value.as_bytes(), b"same-origin" | b"none"),
        (Some(_), Some(_)) => false,
    };
    if !site_ok {
        return Err(Rejected::FetchSite);
    }
    Ok(())
}

fn error(
    status: StatusCode,
    kind: &'static str,
    code: &'static str,
    message: &'static str,
) -> Response<Full<Bytes>> {
    json_response(
        status,
        &json!({
            "error": {
                "message": message,
                "type": kind,
                "param": null,
                "code": code,
            }
        }),
    )
}

fn json_response(status: StatusCode, body: &Value) -> Response<Full<Bytes>> {
    let (status, bytes) = match serde_json::to_vec(body) {
        Ok(bytes) if bytes.len() <= METADATA.max_response_bytes => (status, bytes),
        // Every answer is a bounded snapshot; this is a safeguard.
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            br#"{"error":{"message":"The response could not be produced.","type":"server_error","param":null,"code":"internal"}}"#
                .to_vec(),
        ),
    };
    json_bytes_response(status, bytes)
}

/// A JSON answer with the frontend's headers: `no-store`, `nosniff`, no
/// CORS.
pub(crate) fn json_bytes_response(status: StatusCode, bytes: Vec<u8>) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(bytes)));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::serve::supervisor::testing;
    use http_body_util::BodyExt;

    const PORT: u16 = 41234;

    fn frontend(state: ChildState) -> Frontend {
        let frontend = Frontend::new(
            PORT,
            testing::handle(state, 3),
            "turbo".to_owned(),
            vec![FlowInfo {
                id: "clean".to_owned(),
                transcription_model: "turbo".to_owned(),
                formatter: Some("builtin:clean".to_owned()),
                relay_route: "claude".to_owned(),
            }],
        );
        frontend.set_state(ServiceState::Running);
        frontend
    }

    fn request(
        frontend: &Frontend,
        method: Method,
        target: &str,
        headers: &[(&str, &str)],
    ) -> (StatusCode, HeaderMap, Value) {
        let uri: Uri = target.parse().unwrap();
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            map.append(
                header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        let response = respond(frontend, &method, &uri, &map);
        let status = response.status();
        let headers = response.headers().clone();
        let body = futures_body(response);
        (status, headers, serde_json::from_slice(&body).unwrap())
    }

    fn futures_body(response: Response<Full<Bytes>>) -> Bytes {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        runtime.block_on(async { response.into_body().collect().await.unwrap().to_bytes() })
    }

    const HOST: (&str, &str) = ("host", "127.0.0.1:41234");

    #[test]
    fn the_host_must_be_the_literal_loopback_authority() {
        let frontend = frontend(ChildState::Ready);
        for headers in [
            vec![],
            vec![("host", "localhost:41234")],
            vec![("host", "127.0.0.1")],
            vec![("host", "127.0.0.1:41235")],
            vec![("host", "evil.example:41234")],
            vec![("host", "127.0.0.1:41234"), ("host", "127.0.0.1:41234")],
        ] {
            let (status, response_headers, body) =
                request(&frontend, Method::GET, "/health", &headers);
            assert_eq!(status, StatusCode::FORBIDDEN, "{headers:?}");
            assert_eq!(body["error"]["code"], "host_not_allowed");
            assert!(
                response_headers
                    .get("access-control-allow-origin")
                    .is_none()
            );
        }
        // An absolute target must name the same authority.
        let (status, _, _) = request(
            &frontend,
            Method::GET,
            "http://evil.example:41234/health",
            &[HOST],
        );
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _, _) = request(
            &frontend,
            Method::GET,
            "http://127.0.0.1:41234/health",
            &[HOST],
        );
        assert_eq!(status, StatusCode::OK);
    }

    #[test]
    fn a_present_origin_must_be_this_listener() {
        let frontend = frontend(ChildState::Ready);
        for origin in [
            "null",
            "http://localhost:41234",
            "http://127.0.0.1:17569",
            "https://127.0.0.1:41234",
            "http://127.0.0.1:41234/",
            "http://evil.example",
        ] {
            let (status, headers, body) = request(
                &frontend,
                Method::GET,
                "/pumice/v1/status",
                &[HOST, ("origin", origin)],
            );
            assert_eq!(status, StatusCode::FORBIDDEN, "{origin}");
            assert_eq!(body["error"]["code"], "origin_not_allowed");
            assert!(headers.get("access-control-allow-origin").is_none());
        }
        // The guard runs before routing and method checks.
        let (status, _, body) = request(
            &frontend,
            Method::POST,
            "/nowhere",
            &[HOST, ("origin", "null")],
        );
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"]["code"], "origin_not_allowed");

        let (status, _, _) = request(
            &frontend,
            Method::GET,
            "/health",
            &[HOST, ("origin", "http://127.0.0.1:41234")],
        );
        assert_eq!(status, StatusCode::OK, "its own origin is accepted");
    }

    #[test]
    fn every_route_and_rejection_so_far_uses_the_metadata_policy() {
        assert_eq!(METADATA.connection_limit, Duration::from_secs(10));
        assert_eq!(METADATA.max_response_bytes, 128 * 1024);
        let frontend = frontend(ChildState::Ready);
        let host = HeaderMap::from_iter([(header::HOST, HeaderValue::from_static(HOST.1))]);
        for (method, path, headers) in [
            (Method::GET, "/health", host.clone()),
            (Method::GET, "/ready", host.clone()),
            (Method::GET, "/pumice/v1/status", host.clone()),
            (Method::GET, "/pumice/v1/flows", host.clone()),
            (Method::GET, "/v1/models", host.clone()),
            (Method::GET, "/v1/models/clean", host.clone()),
            (Method::POST, "/health", host.clone()),
            (Method::GET, "/nowhere", host.clone()),
            (Method::GET, "/health", HeaderMap::new()),
        ] {
            let (policy, _) = answer(&frontend, &method, &path.parse().unwrap(), &headers);
            assert_eq!(policy, METADATA, "{method} {path}");
        }
    }

    #[tokio::test]
    async fn a_connection_is_bounded_by_its_route_from_its_arrival() {
        let never = std::future::pending::<()>;
        let unrouted = Duration::from_millis(50);

        // Never routed: the unrouted bound.
        let (_routed, policy) = watch::channel(None);
        let arrival = Instant::now();
        drive(never(), arrival, unrouted, policy).await;
        let elapsed = arrival.elapsed();
        assert!(
            elapsed >= unrouted && elapsed < Duration::from_secs(2),
            "{elapsed:?}"
        );

        // Routed: the route's own limit replaces it, longer or shorter,
        // still counted from the arrival.
        for limit in [Duration::from_millis(300), Duration::from_millis(20)] {
            let (routed, policy) = watch::channel(None);
            let arrival = Instant::now();
            routed.send_replace(Some(RoutePolicy {
                connection_limit: limit,
                max_response_bytes: 1,
            }));
            drive(never(), arrival, unrouted, policy).await;
            let elapsed = arrival.elapsed();
            assert!(
                elapsed >= limit && elapsed < Duration::from_secs(2),
                "{elapsed:?}"
            );
            if limit > unrouted {
                assert!(elapsed >= unrouted, "outlived the unrouted bound");
            } else {
                assert!(elapsed < unrouted, "{elapsed:?}");
            }
        }

        // A connection that ends on its own ends the drive at once.
        let (_routed, policy) = watch::channel(None);
        let arrival = Instant::now();
        drive(async {}, arrival, Duration::from_secs(30), policy).await;
        assert!(arrival.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn cross_site_fetch_metadata_is_refused() {
        let frontend = frontend(ChildState::Ready);
        for site in ["cross-site", "same-site", "Same-Origin", "", "weird"] {
            let (status, headers, body) = request(
                &frontend,
                Method::GET,
                "/pumice/v1/status",
                &[HOST, ("sec-fetch-site", site)],
            );
            assert_eq!(status, StatusCode::FORBIDDEN, "{site:?}");
            assert_eq!(body["error"]["code"], "cross_site_not_allowed");
            assert!(headers.get("access-control-allow-origin").is_none());
        }
        // Before routing and method checks, and twice is refused.
        let (status, _, body) = request(
            &frontend,
            Method::POST,
            "/nowhere",
            &[HOST, ("sec-fetch-site", "cross-site")],
        );
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"]["code"], "cross_site_not_allowed");
        let (status, _, _) = request(
            &frontend,
            Method::GET,
            "/health",
            &[HOST, ("sec-fetch-site", "none"), ("sec-fetch-site", "none")],
        );
        assert_eq!(status, StatusCode::FORBIDDEN);
        for site in ["same-origin", "none"] {
            let (status, _, _) = request(
                &frontend,
                Method::GET,
                "/health",
                &[HOST, ("sec-fetch-site", site)],
            );
            assert_eq!(status, StatusCode::OK, "{site}");
        }
    }

    #[test]
    fn unknown_routes_methods_and_openai_paths_are_json_errors() {
        let frontend = frontend(ChildState::Ready);
        // No chat here (p15/D7); without the dictations' runtime there is
        // no transcription route either.
        for path in [
            "/",
            "/v1/chat/completions",
            "/v1/audio/transcriptions",
            "/v1/modelsx",
            "/health/",
        ] {
            let (status, headers, body) = request(&frontend, Method::GET, path, &[HOST]);
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
            assert_eq!(body["error"]["code"], "not_found");
            assert_eq!(body["error"]["type"], "invalid_request_error");
            assert!(body["error"]["param"].is_null());
            assert_eq!(headers["cache-control"], "no-store");
            assert_eq!(headers["content-type"], "application/json");
        }
        let (status, headers, body) = request(&frontend, Method::POST, "/health", &[HOST]);
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(headers["allow"], "GET");
        assert_eq!(body["error"]["code"], "method_not_allowed");
    }

    #[test]
    fn the_model_routes_list_only_published_flows() {
        // Nothing is published without the dictations' runtime: the
        // enabled flow `clean` is not a model.
        let frontend = frontend(ChildState::Ready);
        let (status, headers, body) = request(&frontend, Method::GET, "/v1/models", &[HOST]);
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers["cache-control"], "no-store");
        assert_eq!(body, json!({"object": "list", "data": []}));
        for path in ["/v1/models/clean", "/v1/models/", "/v1/models/a/b"] {
            let (status, _, body) = request(&frontend, Method::GET, path, &[HOST]);
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
            assert_eq!(
                body,
                json!({"error": {
                    "message": "The requested model does not exist on this server.",
                    "type": "invalid_request_error",
                    "param": "model",
                    "code": "model_not_found",
                }})
            );
        }
        // GET only, behind the guard.
        let (status, headers, _) = request(&frontend, Method::POST, "/v1/models", &[HOST]);
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(headers["allow"], "GET");
        let (status, _, body) = request(
            &frontend,
            Method::GET,
            "/v1/models",
            &[HOST, ("origin", "http://evil.example")],
        );
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"]["code"], "origin_not_allowed");
    }

    #[test]
    fn health_ready_status_and_flows_shapes() {
        let frontend = frontend(ChildState::Ready);
        let (status, headers, body) = request(&frontend, Method::GET, "/health", &[HOST]);
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers["cache-control"], "no-store");
        assert_eq!(
            body,
            json!({"status": "ok", "version": env!("CARGO_PKG_VERSION")})
        );

        let (status, _, body) = request(&frontend, Method::GET, "/ready", &[HOST]);
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["state"], "ready");

        let (status, _, body) = request(&frontend, Method::GET, "/pumice/v1/status", &[HOST]);
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            json!({
                "schema_version": 1,
                "module": "orchestrator",
                "version": env!("CARGO_PKG_VERSION"),
                "service": {"state": "running", "ready": true},
                "transcription": {
                    "state": "ready",
                    "model": "turbo",
                    "generation": 3,
                    "restart_attempts": 0,
                    "next_retry_delay_ms": null,
                    "model_load_ms": 12,
                    "compute": {
                        "requested": "auto",
                        "backend": "vulkan",
                        "device": {"name": "Vulkan0", "description": "Test GPU"},
                        "cpu_variant": "haswell",
                        "fallback_reason": null,
                    },
                    "last_failure": null,
                },
                "flows": {"enabled": 1, "published": 0},
                "dictations": {
                    "served": false,
                    "admission": "open",
                    "active": false,
                    "stage": null,
                },
            })
        );
        assert!(
            !body.to_string().contains("5555"),
            "no private child address"
        );

        let (status, _, body) = request(&frontend, Method::GET, "/pumice/v1/flows", &[HOST]);
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            json!({
                "schema_version": 1,
                "module": "orchestrator",
                "flows": [{
                    "id": "clean",
                    "transcription_model": "turbo",
                    "formatter": "builtin:clean",
                    "relay_route": "claude",
                    "ready": true,
                }],
            })
        );
    }

    #[test]
    fn readiness_follows_the_service_and_the_child() {
        for (service, child, code) in [
            (
                ServiceState::Starting,
                ChildState::Starting,
                "service_starting",
            ),
            (
                ServiceState::Running,
                ChildState::Restarting,
                "transcription_restarting",
            ),
            (
                ServiceState::Running,
                ChildState::Failed,
                "transcription_failed",
            ),
            (ServiceState::Stopping, ChildState::Ready, "shutting_down"),
        ] {
            let frontend = frontend(child);
            frontend.set_state(service);
            let (status, headers, body) = request(&frontend, Method::GET, "/ready", &[HOST]);
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{code}");
            assert_eq!(body["error"]["code"], code);
            assert_eq!(body["error"]["type"], "server_error");
            assert_eq!(headers["cache-control"], "no-store");
            // Health stays alive in every state.
            let (status, _, _) = request(&frontend, Method::GET, "/health", &[HOST]);
            assert_eq!(status, StatusCode::OK);
            let (_, _, flows) = request(&frontend, Method::GET, "/pumice/v1/flows", &[HOST]);
            assert_eq!(flows["flows"][0]["ready"], false);
            assert_eq!(flows["flows"][0]["unavailable"], code);
        }
    }
}
