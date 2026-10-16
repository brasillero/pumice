//! `pumice serve`'s loopback frontend (p13.1-flow-seam/D1, D2, D7, D8).
//!
//! Routes: `GET /health`, `GET /ready`, `GET /pumice/v1/status` and
//! `GET /pumice/v1/flows`; no OpenAI `/v1/*` routes (D2). The dictation
//! route arrives with P13.4.
//!
//! Before anything else, every request passes the origin guard (D8): the
//! `Host` must be the literal `127.0.0.1:<port>` and a present `Origin` must
//! equal `http://127.0.0.1:<port>`, otherwise 403. No CORS headers are ever
//! sent. Every answer is JSON with `Cache-Control: no-store`, errors use the
//! OpenAI error object (`message`, `type`, `param`, `code`), including 404
//! and 405 (with `Allow`).
//!
//! Bounds (design §7): at most [`MAX_CONNECTIONS`] connections (more are
//! closed at once), one request per connection, request headers within
//! [`HEADER_READ_TIMEOUT`] and [`MAX_HEADER_BYTES`], a whole connection
//! within [`CONNECTION_LIMIT`], answers of at most [`MAX_RESPONSE_BYTES`].
//! Handlers only read in-memory snapshots: they never wait for the child,
//! the network, a CLI or a model.

use std::convert::Infallible;
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
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::orchestrator::cancel::{self, CancelGuard};
use crate::orchestrator::result::{MODULE, SCHEMA_VERSION};
use crate::orchestrator::serve::supervisor::{ChildState, SupervisorHandle};

pub const MAX_CONNECTIONS: usize = 64;
pub const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(5);
/// hyper's smallest read buffer, which also bounds the request head.
pub const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_HEADERS: usize = 64;
pub const CONNECTION_LIMIT: Duration = Duration::from_secs(10);
pub const MAX_RESPONSE_BYTES: usize = 128 * 1024;
/// A failed `accept` (for example, out of file descriptors) is retried
/// after this pause instead of spinning.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(50);

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

/// What the routes read.
pub struct Frontend {
    port: u16,
    state: AtomicU8,
    supervisor: SupervisorHandle,
    model: String,
    flows: Vec<FlowInfo>,
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
        }
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
    fn unavailable(&self) -> Option<&'static str> {
        match self.state() {
            ServiceState::Stopping => return Some("shutting_down"),
            ServiceState::Starting => return Some("service_starting"),
            ServiceState::Running => {}
        }
        match self.supervisor.snapshot().state {
            ChildState::Ready => None,
            ChildState::Starting | ChildState::Restarting => Some("transcription_restarting"),
            ChildState::Failed => Some("transcription_failed"),
            ChildState::Stopping => Some("shutting_down"),
        }
    }
}

/// Serves `listener` until `stop` fires, then stops accepting and drops
/// every open connection. Never panics on client behavior.
pub async fn serve(listener: TcpListener, frontend: Arc<Frontend>, mut stop: CancelGuard) {
    let permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = cancel::cancelled(&mut stop) => break,
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
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
    connections.abort_all();
    while connections.join_next().await.is_some() {}
}

async fn serve_connection(stream: tokio::net::TcpStream, frontend: Arc<Frontend>) {
    let service = hyper::service::service_fn(move |request: Request<hyper::body::Incoming>| {
        let response = respond(
            &frontend,
            request.method(),
            request.uri(),
            request.headers(),
        );
        async move { Ok::<_, Infallible>(response) }
    });
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT)
        .max_buf_size(MAX_HEADER_BYTES)
        .max_headers(MAX_HEADERS)
        .keep_alive(false);
    let connection = builder.serve_connection(hyper_util::rt::TokioIo::new(stream), service);
    let _ = tokio::time::timeout(CONNECTION_LIMIT, connection).await;
}

/// The whole request handling: guard, route, answer. Synchronous: it reads
/// snapshots only.
pub fn respond(
    frontend: &Frontend,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
) -> Response<Full<Bytes>> {
    if let Err(rejection) = check_origin(frontend.port, uri, headers) {
        let (code, message) = match rejection {
            Rejected::Host => (
                "host_not_allowed",
                "This service only accepts the Host 127.0.0.1 with its own port.",
            ),
            Rejected::Origin => (
                "origin_not_allowed",
                "This service only accepts requests from its own origin.",
            ),
        };
        return error(
            StatusCode::FORBIDDEN,
            "invalid_request_error",
            code,
            message,
        );
    }
    let route = match uri.path() {
        "/health" => Route::Health,
        "/ready" => Route::Ready,
        "/pumice/v1/status" => Route::Status,
        "/pumice/v1/flows" => Route::Flows,
        _ => {
            return error(
                StatusCode::NOT_FOUND,
                "invalid_request_error",
                "not_found",
                "Unknown route.",
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
        return response;
    }
    match route {
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
    }
}

enum Route {
    Health,
    Ready,
    Status,
    Flows,
}

fn not_ready_message(code: &str) -> &'static str {
    match code {
        "service_starting" => "The service is starting; the transcription model is loading.",
        "transcription_restarting" => "The transcription module is restarting.",
        "transcription_failed" => {
            "The transcription module failed and is not restarted; restart pumice serve."
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
            "last_failure": child.last_failure,
        },
        "flows": {"enabled": frontend.flows.len()},
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
}

/// The D8 guard: exactly one `Host`, the literal `127.0.0.1:<port>` (an
/// absolute request target must name the same authority), and no `Origin`
/// but this listener's own.
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
        Ok(bytes) if bytes.len() <= MAX_RESPONSE_BYTES => (status, bytes),
        // Every answer is a bounded snapshot; this is a safeguard.
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            br#"{"error":{"message":"The response could not be produced.","type":"server_error","param":null,"code":"internal"}}"#
                .to_vec(),
        ),
    };
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
    fn unknown_routes_methods_and_openai_paths_are_json_errors() {
        let frontend = frontend(ChildState::Ready);
        for path in [
            "/",
            "/v1/models",
            "/v1/chat/completions",
            "/v1/audio/transcriptions",
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
                    "last_failure": null,
                },
                "flows": {"enabled": 1},
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
