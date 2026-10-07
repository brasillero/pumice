//! The local OpenAI-compatible HTTP API (S1.1), served on IPv4 loopback only
//! (S5.3).
//!
//! [`bind`] listens on `127.0.0.1` and nothing else; [`serve`] runs the
//! router until Ctrl-C. Dictated text never reaches the ordinary log sink —
//! only one metadata entry per completion request does. When `debug_log` is
//! enabled (S1.4), the opt-in debug sink additionally records full request
//! and response payloads, including the dictation, for investigation.

mod handlers;
mod log_entry;
mod types;

use std::io::{self, IsTerminal};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::Router;
use axum::routing::{get, post};
use tokio::net::TcpListener;

use crate::logging::DebugLog;
use crate::pipeline::{FormatOutcome, Pipeline};

/// State shared by every handler.
#[derive(Clone)]
pub(crate) struct ApiState {
    pipeline: Arc<Pipeline>,
    counter: Arc<AtomicU64>,
    log: Arc<dyn RequestLog>,
    debug_log: Arc<DebugLog>,
}

impl ApiState {
    fn new(
        pipeline: Arc<Pipeline>,
        log: Arc<dyn RequestLog>,
        debug_log: Arc<DebugLog>,
    ) -> ApiState {
        ApiState {
            pipeline,
            counter: Arc::new(AtomicU64::new(1)),
            log,
            debug_log,
        }
    }

    fn next_id(&self) -> u64 {
        self.counter.fetch_add(1, Ordering::Relaxed)
    }

    /// Writes the readable log entry for one completion request.
    fn log_request(&self, number: u64, requested: Option<&str>, outcome: &FormatOutcome) {
        let entry = log_entry::Entry {
            number,
            requested,
            outcome,
        };
        self.log
            .write_line(&log_entry::render(&entry, self.log.color()));
    }

    /// Writes the log entry for a completion request that got no completion.
    fn log_unanswered(
        &self,
        number: u64,
        unanswered: log_entry::Unanswered<'_>,
        elapsed: std::time::Duration,
    ) {
        self.log.write_line(&log_entry::render_unanswered(
            number,
            unanswered,
            elapsed,
            self.log.color(),
        ));
    }
}

/// Where the one-per-request log entry goes. Production writes to stderr;
/// tests supply their own capturing implementation. An entry is one call to
/// `write_line` and may span two lines (the outcome, then the failed
/// provider attempt).
pub trait RequestLog: Send + Sync {
    fn write_line(&self, line: &str);

    /// Whether entries may carry ANSI colors.
    fn color(&self) -> bool {
        false
    }
}

/// The production [`RequestLog`]: one entry per completion request on
/// stderr, never dictated text.
pub struct StderrLog;

impl RequestLog for StderrLog {
    fn write_line(&self, line: &str) {
        eprintln!("{line}");
    }

    /// Colors only on a terminal, unless `NO_COLOR` is set. On Windows only
    /// inside Windows Terminal, since the legacy console prints the escape
    /// codes literally.
    fn color(&self) -> bool {
        std::io::stderr().is_terminal()
            && std::env::var_os("NO_COLOR").is_none()
            && (!cfg!(windows) || std::env::var_os("WT_SESSION").is_some())
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
    debug_log: Arc<DebugLog>,
) -> io::Result<()> {
    let state = ApiState::new(pipeline, log, debug_log);
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
