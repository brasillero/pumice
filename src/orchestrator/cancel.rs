//! Operation-wide cancellation for `pumice run`: Ctrl-C on every platform,
//! plus SIGTERM on Unix (p12-pumice-run/D7 exit codes 130 and 143).
//!
//! Handlers are registered synchronously, before any preparation or child
//! spawn: an early signal can never hit a parent whose handlers are still
//! being installed. Every stage checks the signal before starting and while
//! waiting, and an explicit cancellation wins over any I/O failure it
//! induces.

use std::io;

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

/// Registers the signal handlers and returns the guard stages watch.
/// Registration is synchronous: when this returns `Ok`, an early signal is
/// already caught, before the caller has prepared or spawned anything. A
/// registration failure is handed back as an I/O error; the run must not
/// start without cancellation wired up.
///
/// The returned task ends with the first signal (or when the runtime shuts
/// down); abort it on the way out.
pub fn install() -> io::Result<(CancelGuard, tokio::task::JoinHandle<()>)> {
    let (sender, receiver) = watch::channel(None);
    #[cfg(unix)]
    let listener = {
        use tokio::signal::unix::{SignalKind, signal};
        let mut interrupt = signal(SignalKind::interrupt())?;
        let mut terminate = signal(SignalKind::terminate())?;
        async move {
            tokio::select! {
                result = interrupt.recv() => {
                    if result.is_some() {
                        let _ = sender.send(Some(CancelReason::Interrupt));
                    }
                }
                result = terminate.recv() => {
                    if result.is_some() {
                        let _ = sender.send(Some(CancelReason::Terminate));
                    }
                }
            }
        }
    };
    #[cfg(windows)]
    let listener = {
        let mut ctrl_c = tokio::signal::windows::ctrl_c()?;
        async move {
            if ctrl_c.recv().await.is_some() {
                let _ = sender.send(Some(CancelReason::Interrupt));
            }
        }
    };
    #[cfg(not(any(unix, windows)))]
    let listener = async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = sender.send(Some(CancelReason::Interrupt));
        }
    };
    Ok((receiver, tokio::spawn(listener)))
}
