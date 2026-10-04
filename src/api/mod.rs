//! The local OpenAI-compatible HTTP API (S1.1), served on IPv4 loopback only
//! (S5.3).
//!
//! [`bind`] listens on `127.0.0.1` and nothing else; [`serve`] runs the
//! router until Ctrl-C. Dictated text never reaches the log sink — only one
//! metadata line per completion request does (the S1.4 debug log plugs into
//! the handler at the marked spot).

mod handlers;
mod types;

use std::io;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::Router;
use axum::routing::{get, post};
use tokio::net::TcpListener;

use crate::pipeline::Pipeline;

/// State shared by every handler.
#[derive(Clone)]
pub(crate) struct ApiState {
    pipeline: Arc<Pipeline>,
    counter: Arc<AtomicU64>,
    log: Arc<dyn RequestLog>,
}

impl ApiState {
    fn new(pipeline: Arc<Pipeline>, log: Arc<dyn RequestLog>) -> ApiState {
        ApiState {
            pipeline,
            counter: Arc::new(AtomicU64::new(1)),
            log,
        }
    }

    fn next_id(&self) -> u64 {
        self.counter.fetch_add(1, Ordering::Relaxed)
    }
}

/// Where the one-per-request metadata line goes. Production writes to
/// stderr; tests supply their own capturing implementation.
pub trait RequestLog: Send + Sync {
    fn write_line(&self, line: &str);
}

/// The production [`RequestLog`]: one metadata line per completion request
/// on stderr, never dictated text.
pub struct StderrLog;

impl RequestLog for StderrLog {
    fn write_line(&self, line: &str) {
        eprintln!("{line}");
    }
}

/// Binds the IPv4 loopback address for `port` — never `0.0.0.0`, never `::`.
pub async fn bind(port: u16) -> io::Result<TcpListener> {
    TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).await
}

/// Serves the API on `listener` until Ctrl-C, then lets in-flight requests
/// finish and stops.
pub async fn serve(
    listener: TcpListener,
    pipeline: Arc<Pipeline>,
    log: Arc<dyn RequestLog>,
) -> io::Result<()> {
    let state = ApiState::new(pipeline, log);
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
}

fn router(state: ApiState) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(handlers::chat_completions))
        .route("/v1/models", get(handlers::list_models))
        .route("/health", get(handlers::health))
        .fallback(handlers::not_found)
        .with_state(state)
}

async fn shutdown_signal() {
    // One Ctrl-C starts the graceful drain; nothing more is special-cased.
    let _ = tokio::signal::ctrl_c().await;
}
