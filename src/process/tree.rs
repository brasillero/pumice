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

use std::io;
use std::process::ExitStatus;
use std::time::Duration;

use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout, Command};

/// A spawned CLI together with its process group or Job Object.
///
/// Dropping it kills the direct child (`kill_on_drop`) and, on Windows, every
/// process in the job. Call [`kill`](ProcessTree::kill) for the whole tree
/// on Unix.
pub struct ProcessTree {
    child: Box<dyn ChildWrapper>,
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
        Ok(ProcessTree {
            child: wrap.spawn()?,
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

    /// Waits until the direct child exits. Descendants may still be running.
    ///
    /// Unix: the process-group wrapper's `wait` lets Tokio reap the child,
    /// then reaps any other children of ours in the group (there are none:
    /// descendants belong to the CLI), so a descendant holding the pipes does
    /// not block it. Its `try_wait` is avoided because it reaps behind Tokio's
    /// back, after which `kill_on_drop` would signal a stale PID.
    #[cfg(unix)]
    pub async fn wait_direct(&mut self) -> io::Result<ExitStatus> {
        self.child.wait().await
    }

    /// Waits until the direct child exits. Descendants may still be running.
    ///
    /// Windows: the Job Object wrapper's `wait` also waits for every process
    /// in the job, which would block on a descendant holding the pipes, so
    /// the direct child is polled with `try_wait` instead.
    #[cfg(windows)]
    pub async fn wait_direct(&mut self) -> io::Result<ExitStatus> {
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Kills every process in the tree without waiting. Best effort: the
    /// tree may already be gone.
    pub fn kill(&mut self) {
        let _ = self.child.start_kill();
    }

    /// Kills the tree and reaps the direct child, waiting at most `limit`.
    ///
    /// Uses [`wait_direct`](ProcessTree::wait_direct) rather than the
    /// Windows wrapper's `wait`, which waits for job notifications that the
    /// `try_wait` polling has already consumed and could block until `limit`
    /// on every call.
    pub async fn kill_and_reap(&mut self, limit: Duration) {
        self.kill();
        let _ = tokio::time::timeout(limit, self.wait_direct()).await;
    }
}
