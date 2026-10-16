//! Operation-wide cancellation: Ctrl-C on every platform, plus SIGTERM on
//! Unix, for `pumice run` (p12-pumice-run/D7 exit codes 130 and 143), and a
//! [`CancelHandle`] any other owner of an operation can fire with its own
//! reason (a service shutting down, a client that went away).
//!
//! Handlers are registered synchronously, before any preparation or child
//! spawn: an early signal can never hit a parent whose handlers are still
//! being installed. Every stage checks the signal before starting and while
//! waiting, and an explicit cancellation wins over any I/O failure it
//! induces.

use std::io;

use tokio::sync::watch;

/// What cancelled the operation. The two signals are `pumice run`'s; the
/// other reasons belong to an operation owned by a long-running service
/// (p13: a dropped client is not a synthetic Ctrl-C).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelReason {
    /// Ctrl-C (SIGINT on Unix, console Ctrl-C on Windows): exit 130.
    Interrupt,
    /// SIGTERM (Unix only): exit 143.
    #[cfg_attr(not(unix), allow(dead_code))]
    Terminate,
    /// The service owning the operation is shutting down.
    Shutdown,
    /// The client that asked for the operation went away.
    // Constructed by `pumice serve` (P13.4); only tests use it until then.
    ClientDisconnected,
}

impl CancelReason {
    pub fn exit_code(self) -> u8 {
        match self {
            CancelReason::Interrupt => 130,
            // 128 + SIGTERM(15), matching the shell convention.
            CancelReason::Terminate => 143,
            // Not a signal: a plain runtime failure for a process exit code.
            CancelReason::Shutdown | CancelReason::ClientDisconnected => 1,
        }
    }
}

/// Watches the cancellation signal. Cheap to clone; every stage holds one.
pub type CancelGuard = watch::Receiver<Option<CancelReason>>;

/// Fires an operation's cancellation. The first reason wins: a later
/// `cancel` never rewrites why the operation stopped.
pub struct CancelHandle {
    sender: watch::Sender<Option<CancelReason>>,
}

impl CancelHandle {
    pub fn cancel(&self, reason: CancelReason) {
        self.sender.send_if_modified(|current| {
            if current.is_some() {
                return false;
            }
            *current = Some(reason);
            true
        });
    }
}

/// A fresh, not yet cancelled context: the handle that fires it and the
/// guard the stages watch. The guard keeps working after the handle is
/// dropped (it simply never fires).
pub fn context() -> (CancelHandle, CancelGuard) {
    let (sender, receiver) = watch::channel(None);
    (CancelHandle { sender }, receiver)
}

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
    let (handle, receiver) = context();
    #[cfg(unix)]
    let listener = {
        use tokio::signal::unix::{SignalKind, signal};
        let mut interrupt = signal(SignalKind::interrupt())?;
        let mut terminate = signal(SignalKind::terminate())?;
        async move {
            tokio::select! {
                result = interrupt.recv() => {
                    if result.is_some() {
                        handle.cancel(CancelReason::Interrupt);
                    }
                }
                result = terminate.recv() => {
                    if result.is_some() {
                        handle.cancel(CancelReason::Terminate);
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
                handle.cancel(CancelReason::Interrupt);
            }
        }
    };
    #[cfg(not(any(unix, windows)))]
    let listener = async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            handle.cancel(CancelReason::Interrupt);
        }
    };
    Ok((receiver, tokio::spawn(listener)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_reason_wins() {
        let (handle, guard) = context();
        assert_eq!(current(&guard), None);
        handle.cancel(CancelReason::Shutdown);
        handle.cancel(CancelReason::ClientDisconnected);
        assert_eq!(current(&guard), Some(CancelReason::Shutdown));
    }

    #[tokio::test]
    async fn a_dropped_handle_never_fires() {
        let (handle, mut guard) = context();
        drop(handle);
        assert_eq!(current(&guard), None);
        let waited =
            tokio::time::timeout(std::time::Duration::from_millis(20), cancelled(&mut guard)).await;
        assert!(waited.is_err(), "no cancellation without a reason");
    }
}
