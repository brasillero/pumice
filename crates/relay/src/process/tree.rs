//! Process-tree ownership: the CLI and everything it starts can be killed
//! together.
//!
//! Unix: the child leads a new process group, and the whole group is killed.
//! Windows: the child is created suspended, assigned to a Job Object, then
//! resumed, so no descendant can escape the job; the job kills its processes
//! when its last handle closes.
//!
//! A process group does not contain a Unix descendant that deliberately
//! leaves it. This is lifecycle management, not a security boundary.
//!
//! PGID reuse on Unix: the direct child leads the group, so the group ID
//! cannot be reused while that child exists, even as a zombie. Its exit is
//! therefore observed without reaping it (`waitid` with `WNOWAIT`), the group
//! is signalled while the zombie still pins the ID, and only then is the
//! child reaped. After reaping, the group is never signalled again.

use std::io;
use std::process::ExitStatus;
use std::sync::Mutex;
use std::time::Duration;

use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::time::Instant;

/// How often the direct child is polled for exit.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// A spawned CLI together with its process group or Job Object.
///
/// Dropping it before [`finish`](ProcessTree::finish) has reaped the child
/// (for example when the caller's future is cancelled) kills the whole tree.
pub struct ProcessTree {
    child: Box<dyn ChildWrapper>,
    #[cfg(unix)]
    pid: libc::id_t,
    #[cfg(windows)]
    status: Option<ExitStatus>,
    /// The tree was killed and the direct child reaped: never signal again.
    reaped: bool,
}

impl ProcessTree {
    /// Spawns `command` inside a new process group / Job Object.
    pub fn spawn(command: Command) -> io::Result<ProcessTree> {
        let mut wrap = CommandWrap::from(command);
        wrap.wrap(KillOnDrop);
        #[cfg(unix)]
        wrap.wrap(process_wrap::tokio::ProcessGroup::leader());
        #[cfg(windows)]
        wrap.wrap(process_wrap::tokio::JobObject);
        #[cfg_attr(windows, allow(unused_mut))]
        let mut child = wrap.spawn()?;
        #[cfg(unix)]
        let pid = match child.id() {
            Some(pid) => pid as libc::id_t,
            None => {
                let _ = child.start_kill();
                return Err(io::Error::other("spawned child has no PID"));
            }
        };
        Ok(ProcessTree {
            child,
            #[cfg(unix)]
            pid,
            #[cfg(windows)]
            status: None,
            reaped: false,
        })
    }

    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child.stdin().take()
    }

    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout().take()
    }

    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child.stderr().take()
    }

    /// Waits until the direct child exits, without reaping it on Unix.
    /// Descendants may still be running. Cancellation-safe.
    pub async fn exited(&mut self) -> io::Result<()> {
        while !self.poll_exited()? {
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        Ok(())
    }

    #[cfg(unix)]
    fn poll_exited(&mut self) -> io::Result<bool> {
        loop {
            // SAFETY: an all-zero `siginfo_t` is a valid value; `waitid` only
            // writes into it.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            // SAFETY: `info` is a valid, writable `siginfo_t`. `WNOWAIT`
            // leaves the child waitable, so Tokio still reaps it later.
            let rc = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.pid,
                    &mut info,
                    libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
                )
            };
            if rc == 0 {
                // With WNOHANG and no state change, the zeroed `info` stays
                // zero (Linux clears it; elsewhere it is left untouched).
                return Ok(info.si_signo == libc::SIGCHLD);
            }
            let err = io::Error::last_os_error();
            match err.raw_os_error() {
                Some(libc::EINTR) => continue,
                // Already reaped: it has certainly exited.
                Some(libc::ECHILD) => return Ok(true),
                _ => return Err(err),
            }
        }
    }

    #[cfg(windows)]
    fn poll_exited(&mut self) -> io::Result<bool> {
        // The Job Object wrapper's `wait` also waits for every process in the
        // job, which would block on a descendant holding the pipes; polling
        // the direct child observes it alone.
        if self.status.is_none() {
            self.status = self.child.try_wait()?;
        }
        Ok(self.status.is_some())
    }

    /// Kills every process in the tree without waiting. Safe to repeat: a
    /// reaped tree is never signalled again.
    pub(crate) fn kill_now(&mut self) {
        if !self.reaped {
            // Unix: the unreaped leader still pins the group ID here.
            let _ = self.child.start_kill();
        }
    }

    /// True once [`finish`](ProcessTree::finish) reaped the direct child.
    pub(crate) fn is_reaped(&self) -> bool {
        self.reaped
    }

    /// Kills every process in the tree, then reaps the direct child, waiting
    /// at most `limit`. Returns the direct child's exit status, or `None`
    /// when it could not be reaped in time (dropping the tree retries the
    /// kill).
    pub async fn finish(&mut self, limit: Duration) -> Option<ExitStatus> {
        self.kill_now();
        let status = tokio::time::timeout(limit, self.reap()).await.ok()?.ok()?;
        self.reaped = true;
        Some(status)
    }

    #[cfg(unix)]
    async fn reap(&mut self) -> io::Result<ExitStatus> {
        // Reap through the native Tokio child (the layer under the process
        // group wrapper) so Tokio records the exit and `kill_on_drop` never
        // signals a stale PID. Its `wait` is cancellation-safe.
        self.child.inner_mut().wait().await
    }

    #[cfg(windows)]
    async fn reap(&mut self) -> io::Result<ExitStatus> {
        self.exited().await?;
        Ok(self.status.expect("exited() sets the status"))
    }
}

impl std::fmt::Debug for ProcessTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("ProcessTree");
        #[cfg(unix)]
        debug.field("pid", &self.pid);
        debug.field("reaped", &self.reaped).finish()
    }
}

impl Drop for ProcessTree {
    /// Cancellation safety: if the run was abandoned before `finish` reaped
    /// the child, kill the whole tree now. On Unix the unreaped leader still
    /// pins the group ID, so the signal cannot reach an unrelated group. On
    /// Windows closing the job handle afterwards also kills the job.
    fn drop(&mut self) {
        self.kill_now();
    }
}

/// Process trees killed but not yet reaped, collected so a caller can await
/// their teardown after a cancellation (p12-pumice-run: the orchestrator
/// drains the scope, bounded, before reporting its result).
///
/// Registration kills the tree synchronously — the same kill as
/// [`ProcessTree`]'s drop — and [`drain`](CleanupScope::drain) then reaps
/// each registered tree within one bounded budget. Trees still registered
/// when the scope is dropped are killed again by their own drop: kill-on-drop
/// stays the last defense.
#[derive(Debug, Default)]
pub struct CleanupScope {
    pending: Mutex<Vec<ProcessTree>>,
}

impl CleanupScope {
    pub fn new() -> CleanupScope {
        CleanupScope::default()
    }

    /// True when no killed tree is waiting to be reaped.
    pub fn is_empty(&self) -> bool {
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty()
    }

    /// Kills `tree` now and keeps it for a later [`drain`](Self::drain).
    pub(crate) fn kill_and_register(&self, mut tree: ProcessTree) {
        tree.kill_now();
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(tree);
    }

    /// Reaps every registered tree, spending at most `limit` overall, and
    /// returns how many trees could not be reaped in time. Unreaped trees
    /// stay registered — and killed — so a later drain can retry them.
    pub async fn drain(&self, limit: Duration) -> usize {
        let deadline = Instant::now() + limit;
        let mut unreaped = 0;
        loop {
            let next = self
                .pending
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .pop();
            let Some(mut tree) = next else { break };
            let remaining = deadline.saturating_duration_since(Instant::now());
            if tree.finish(remaining).await.is_none() {
                unreaped += 1;
            }
            if Instant::now() >= deadline {
                unreaped += self
                    .pending
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .len();
                break;
            }
        }
        unreaped
    }
}
