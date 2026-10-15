//! Operation-wide cancellation for `pumice run`: Ctrl-C on every platform,
//! plus SIGTERM on Unix (p12-pumice-run/D7 exit codes 130 and 143).
//!
//! Handlers are registered before any child is spawned; every stage checks
//! the signal before starting and while waiting, and an explicit
//! cancellation wins over any I/O failure it induces.

use tokio::sync::watch;

/// Which signal cancelled the run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelReason {
    /// Ctrl-C (SIGINT on Unix, console Ctrl-C on Windows): exit 130.
    Interrupt,
    /// SIGTERM (Unix only): exit 143.
    Terminate,
}

impl CancelReason {
    pub fn exit_code(self) -> u8 {
        match self {
            CancelReason::Interrupt => 130,
            // 128 + SIGTERM(15), matching the shell convention.
            CancelReason::Terminate => 143,
        }
    }
}

/// Watches the cancellation signal. Cheap to clone; every stage holds one.
pub type CancelGuard = watch::Receiver<Option<CancelReason>>;

/// The currently observed cancellation, if any.
pub fn current(guard: &CancelGuard) -> Option<CancelReason> {
    *guard.borrow()
}

/// Completes once a cancellation has been observed, now or in the future.
/// If the sender is gone without ever signalling, waits forever (the run is
/// shutting down anyway).
pub async fn cancelled(guard: &mut CancelGuard) -> CancelReason {
    loop {
        if let Some(reason) = current(guard) {
            return reason;
        }
        if guard.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// Registers the signal handlers and returns the guard stages watch. The
/// returned task ends when the runtime shuts down; abort it on the way out.
pub fn install() -> (CancelGuard, tokio::task::JoinHandle<()>) {
    let (sender, receiver) = watch::channel(None);
    let task = tokio::spawn(async move {
        #[cfg(unix)]
        {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut sigterm) => {
                    tokio::select! {
                        result = tokio::signal::ctrl_c() => {
                            if result.is_ok() {
                                let _ = sender.send(Some(CancelReason::Interrupt));
                            }
                        }
                        _ = sigterm.recv() => {
                            let _ = sender.send(Some(CancelReason::Terminate));
                        }
                    }
                }
                // SIGTERM registration failed: Ctrl-C alone still cancels.
                Err(_) => {
                    if tokio::signal::ctrl_c().await.is_ok() {
                        let _ = sender.send(Some(CancelReason::Interrupt));
                    }
                }
            }
        }
        #[cfg(not(unix))]
        {
            if tokio::signal::ctrl_c().await.is_ok() {
                let _ = sender.send(Some(CancelReason::Interrupt));
            }
        }
    });
    (receiver, task)
}
