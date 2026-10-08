//! Shared runner for CLI calls.
//!
//! Every call gets a private temporary root, removed afterwards:
//!
//! ```text
//! <temporary root>/
//! ├── workspace/     # the CLI's working directory, empty when it starts
//! └── control/       # control files such as the system prompt
//! ```

mod invocation;
mod private_temp;
mod resolve;
mod tree;

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{ChildStdin, Command};
use tokio::time::Instant;

pub use invocation::{
    Argument, CliInvocation, ControlFile, OutputParser, ProcessOutput, ProgramSpec,
    toml_basic_string,
};
pub use resolve::{ResolvedProgram, resolve_program};

use crate::providers::{ProviderError, ProviderErrorCode};
use tree::ProcessTree;

/// Maximum stdout captured from a CLI; more is `OutputTooLarge`.
pub const MAX_STDOUT_BYTES: usize = 10 * 1024 * 1024;
/// Only this many trailing stderr bytes are kept.
pub const STDERR_TAIL_BYTES: usize = 64 * 1024;
/// How long to keep draining pipes after the direct child exits. A
/// descendant that inherited the pipes could otherwise hold them open
/// indefinitely.
const EXIT_GRACE: Duration = Duration::from_millis(250);
/// Upper bound on reaping the child after the tree has been killed.
const REAP_LIMIT: Duration = Duration::from_secs(2);

/// Attempts at removing the temporary root, and the pause between them.
const REMOVE_ATTEMPTS: u32 = 10;
const REMOVE_RETRY_DELAY: Duration = Duration::from_millis(50);

const WORKSPACE_DIR: &str = "workspace";
const CONTROL_DIR: &str = "control";

/// Runs CLI invocations. Stateless; share one instance.
#[derive(Debug, Default, Clone, Copy)]
pub struct ProcessRunner;

impl ProcessRunner {
    pub fn new() -> ProcessRunner {
        ProcessRunner
    }

    /// Runs `invocation` to completion before `deadline`.
    ///
    /// The deadline covers setup, stdin, execution and pipe draining. On
    /// timeout or oversized output the whole process tree is killed. The
    /// temporary root is always removed.
    pub async fn run(
        &self,
        invocation: CliInvocation,
        deadline: Instant,
    ) -> Result<ProcessOutput, ProviderError> {
        if Instant::now() >= deadline {
            return Err(ProviderError::Timeout);
        }
        let program = resolve_program(&invocation.program)?;
        // Dropping the guard removes the root, on success, failure and
        // cancellation alike.
        let root = RootGuard(Some(private_temp::tempdir().map_err(io_error)?));
        run_in(root.path(), &program, invocation, deadline).await
    }
}

async fn run_in(
    root: &Path,
    program: &ResolvedProgram,
    invocation: CliInvocation,
    deadline: Instant,
) -> Result<ProcessOutput, ProviderError> {
    let workspace = root.join(WORKSPACE_DIR);
    let control = root.join(CONTROL_DIR);
    private_temp::create_dir(&workspace).map_err(io_error)?;
    private_temp::create_dir(&control).map_err(io_error)?;
    write_files(&workspace, &invocation.workspace_files)?;
    let control_paths = write_files(&control, &invocation.control_files)?;
    let args = invocation
        .args
        .iter()
        .map(|arg| match arg {
            Argument::Literal(value) => Ok(value.clone()),
            Argument::ControlPath { file } => control_paths
                .get(*file)
                .map(|path| path.clone().into_os_string())
                .ok_or(ProviderError::other(
                    ProviderErrorCode::InvalidConfiguration,
                )),
            // A lossy conversion would name a different file, so a path that
            // is not valid Unicode is refused.
            Argument::ConfigControlPath { key, file } => control_paths
                .get(*file)
                .and_then(|path| path.to_str())
                .map(|path| OsString::from(format!("{key}={}", toml_basic_string(path))))
                .ok_or(ProviderError::other(
                    ProviderErrorCode::InvalidConfiguration,
                )),
        })
        .collect::<Result<Vec<OsString>, _>>()?;

    let mut command = Command::new(&program.path);
    command
        // Entrypoint of a translated Windows shim, before the adapter's args.
        .args(&program.prefix_args)
        .args(&args)
        .current_dir(&workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for name in &invocation.remove_env {
        command.env_remove(name);
    }
    command.envs(&invocation.env);

    // Setup above can take a while (temporary files, PATH lookup): never
    // start a CLI once the deadline has passed.
    if Instant::now() >= deadline {
        return Err(ProviderError::Timeout);
    }
    let mut tree = ProcessTree::spawn(command).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => ProviderError::NotInstalled,
        _ => ProviderError::other(ProviderErrorCode::Spawn),
    })?;
    let captured = drive(&mut tree, invocation.stdin, deadline).await;
    // Kill whatever is left (descendants included) on every path, then reap.
    // If this future is dropped before here, `ProcessTree`'s drop kills the
    // tree instead.
    let status = tree.finish(REAP_LIMIT).await;
    let (stdout, stderr_tail, stdin_complete) = captured?;
    let status = status.ok_or(ProviderError::other(ProviderErrorCode::Io))?;
    // A CLI that exits cleanly without taking all of its stdin answered a
    // truncated prompt. A failed exit keeps its own classification.
    if status.success() && !stdin_complete {
        return Err(ProviderError::other(ProviderErrorCode::Io));
    }
    Ok(ProcessOutput {
        status,
        stdout,
        stderr_tail,
    })
}

/// Owns a call's temporary root and removes it when dropped: after the
/// call, or when the call is cancelled (the request was dropped). One
/// attempt runs inline; when it fails (on Windows a killed descendant can
/// hold the workspace for a moment after termination) a plain thread keeps
/// retrying, so the retries depend neither on the request nor on the async
/// runtime still running.
struct RootGuard(Option<tempfile::TempDir>);

impl RootGuard {
    fn path(&self) -> &Path {
        self.0.as_ref().expect("root present until dropped").path()
    }
}

impl Drop for RootGuard {
    fn drop(&mut self) {
        let Some(root) = self.0.take() else { return };
        if try_remove(root.path()) {
            return;
        }
        let spawned = std::thread::Builder::new()
            .name("pumice-temp-cleanup".to_owned())
            .spawn(move || remove_root_with_retries(root));
        // Could not start a thread: `TempDir`'s drop (inside the failed
        // closure) tries once more.
        drop(spawned);
    }
}

/// One removal attempt; true when the root is gone.
fn try_remove(root: &Path) -> bool {
    match std::fs::remove_dir_all(root) {
        Ok(()) => true,
        Err(e) => e.kind() == io::ErrorKind::NotFound,
    }
}

/// Retries removal briefly. Failure must not hide the call's outcome: after
/// the last attempt it is reported on stderr (the path only, never dictated
/// text) and `TempDir`'s drop tries once more.
fn remove_root_with_retries(root: tempfile::TempDir) {
    for _ in 0..REMOVE_ATTEMPTS {
        std::thread::sleep(REMOVE_RETRY_DELAY);
        if try_remove(root.path()) {
            return;
        }
    }
    eprintln!(
        "warning: could not remove the temporary directory {}",
        root.path().display()
    );
}

/// Writes files into `dir` and returns their absolute paths.
///
/// Names may be plain file names or relative subpaths such as
/// `agents/pumice.json`; all components must be normal (`..` and absolute
/// paths are rejected) and intermediate directories are created as needed.
fn write_files(dir: &Path, files: &[ControlFile]) -> Result<Vec<PathBuf>, ProviderError> {
    files
        .iter()
        .map(|file| {
            let name = Path::new(&file.name);
            if name
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
                || name.as_os_str().is_empty()
            {
                return Err(ProviderError::other(
                    ProviderErrorCode::InvalidConfiguration,
                ));
            }
            let path = dir.join(name);
            if let Some(parent) = path.parent() {
                private_temp::create_dir_all(parent).map_err(io_error)?;
            }
            private_temp::write(&path, &file.contents).map_err(io_error)?;
            Ok(path)
        })
        .collect()
}

/// Why the output pump stopped early.
enum Abort {
    Timeout,
    TooLarge,
    Io,
}

/// Feeds stdin, drains stdout/stderr and waits for the direct child to
/// exit, all concurrently and bounded by `deadline`. Returns the captured
/// stdout, the stderr tail and whether all of stdin was written; the caller
/// kills the tree and reaps the child.
async fn drive(
    tree: &mut ProcessTree,
    stdin: Vec<u8>,
    deadline: Instant,
) -> Result<(Vec<u8>, Vec<u8>, bool), ProviderError> {
    let stdin_pipe = tree.take_stdin();
    let stdout_pipe = tree.take_stdout();
    let stderr_pipe = tree.take_stderr();

    let mut stdout = Vec::new();
    let mut stderr_tail = Vec::new();
    let mut exited = false;
    let mut stdout_done = stdout_pipe.is_none();
    let mut stderr_done = stderr_pipe.is_none();
    let mut stdin_done = stdin_pipe.is_none();
    let mut stdin_complete = stdin_pipe.is_none() || stdin.is_empty();

    // The futures borrow the buffers, so whatever was read survives when they
    // are dropped early (at the end of this block).
    let outcome: Result<(), Abort> = {
        let stdin_fut = write_stdin(stdin_pipe, stdin);
        let stdout_fut = read_capped(stdout_pipe, &mut stdout, MAX_STDOUT_BYTES);
        let stderr_fut = read_tail(stderr_pipe, &mut stderr_tail, STDERR_TAIL_BYTES);
        tokio::pin!(stdin_fut, stdout_fut, stderr_fut);

        let mut limit = deadline;
        loop {
            if exited && stdout_done && stderr_done {
                break Ok(());
            }
            // Biased with the timer first: once the limit has passed, no
            // other ready branch can win.
            tokio::select! {
                biased;
                () = tokio::time::sleep_until(limit) => {
                    // Past the grace period (and still before the deadline) the
                    // parent has finished: keep what it wrote and let the caller
                    // kill the descendants still holding the pipes.
                    if exited && limit < deadline {
                        break Ok(());
                    }
                    break Err(Abort::Timeout);
                }
                r = tree.exited(), if !exited => match r {
                    Ok(()) => {
                        exited = true;
                        limit = deadline.min(Instant::now() + EXIT_GRACE);
                    }
                    Err(_) => break Err(Abort::Io),
                },
                r = &mut stdout_fut, if !stdout_done => match r {
                    Ok(()) => stdout_done = true,
                    Err(abort) => break Err(abort),
                },
                r = &mut stderr_fut, if !stderr_done => match r {
                    Ok(()) => stderr_done = true,
                    Err(abort) => break Err(abort),
                },
                written = &mut stdin_fut, if !stdin_done => {
                    stdin_done = true;
                    stdin_complete = written;
                }
            }
        }
    };

    match outcome {
        Ok(()) if Instant::now() >= deadline => Err(ProviderError::Timeout),
        Ok(()) => Ok((stdout, stderr_tail, stdin_complete)),
        Err(Abort::Timeout) => Err(ProviderError::Timeout),
        Err(Abort::TooLarge) => Err(ProviderError::other(ProviderErrorCode::OutputTooLarge)),
        Err(Abort::Io) => Err(ProviderError::other(ProviderErrorCode::Io)),
    }
}

/// Writes `data` and closes stdin; returns whether every byte was written.
/// A CLI may exit or close stdin without reading it: the caller decides
/// with the exit status. A failed close after a full write is ignored.
async fn write_stdin(pipe: Option<ChildStdin>, data: Vec<u8>) -> bool {
    let Some(mut pipe) = pipe else {
        return data.is_empty();
    };
    // On Windows `write_all` can return once the data is buffered: the
    // flush waits for the pipe to take it, so a CLI that never reads is
    // caught here too.
    if pipe.write_all(&data).await.is_err() || pipe.flush().await.is_err() {
        return false;
    }
    let _ = pipe.shutdown().await;
    true
}

async fn read_capped(
    pipe: Option<impl AsyncRead + Unpin>,
    buf: &mut Vec<u8>,
    cap: usize,
) -> Result<(), Abort> {
    let Some(mut pipe) = pipe else { return Ok(()) };
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let n = pipe.read(&mut chunk).await.map_err(|_| Abort::Io)?;
        if n == 0 {
            return Ok(());
        }
        if buf.len() + n > cap {
            return Err(Abort::TooLarge);
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

async fn read_tail(
    pipe: Option<impl AsyncRead + Unpin>,
    buf: &mut Vec<u8>,
    keep: usize,
) -> Result<(), Abort> {
    let Some(mut pipe) = pipe else { return Ok(()) };
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        let n = pipe.read(&mut chunk).await.map_err(|_| Abort::Io)?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > keep {
            buf.drain(..buf.len() - keep);
        }
    }
}

fn io_error(_: io::Error) -> ProviderError {
    ProviderError::other(ProviderErrorCode::Io)
}
