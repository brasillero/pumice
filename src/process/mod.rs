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
pub use resolve::resolve_program;

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
#[derive(Debug, Default)]
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
        let root = tempfile::Builder::new()
            .prefix("pumice-")
            .tempdir()
            .map_err(io_error)?;
        let result = run_in(root.path(), &program, invocation, deadline).await;
        remove_root(root).await;
        result
    }
}

async fn run_in(
    root: &Path,
    program: &Path,
    invocation: CliInvocation,
    deadline: Instant,
) -> Result<ProcessOutput, ProviderError> {
    let workspace = root.join(WORKSPACE_DIR);
    let control = root.join(CONTROL_DIR);
    std::fs::create_dir(&workspace).map_err(io_error)?;
    std::fs::create_dir(&control).map_err(io_error)?;
    let control_paths = write_control_files(&control, &invocation.control_files)?;
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

    let mut command = Command::new(program);
    command
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
    let (stdout, stderr_tail) = captured?;
    let status = status.ok_or(ProviderError::other(ProviderErrorCode::Io))?;
    Ok(ProcessOutput {
        status,
        stdout,
        stderr_tail,
    })
}

/// Removes the temporary root, retrying briefly: on Windows a killed
/// descendant can hold the workspace for a moment after termination.
/// Failure must not hide the call's outcome, so it is ignored after the last
/// attempt (and `TempDir`'s drop tries once more).
async fn remove_root(root: tempfile::TempDir) {
    for _ in 0..REMOVE_ATTEMPTS {
        match std::fs::remove_dir_all(root.path()) {
            Ok(()) => return,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return,
            Err(_) => tokio::time::sleep(REMOVE_RETRY_DELAY).await,
        }
    }
}

/// Writes control files into `dir` and returns their absolute paths.
fn write_control_files(dir: &Path, files: &[ControlFile]) -> Result<Vec<PathBuf>, ProviderError> {
    files
        .iter()
        .map(|file| {
            let name = Path::new(file.name);
            let mut components = name.components();
            let plain = matches!(
                (components.next(), components.next()),
                (Some(std::path::Component::Normal(_)), None)
            );
            if !plain {
                return Err(ProviderError::other(
                    ProviderErrorCode::InvalidConfiguration,
                ));
            }
            let path = dir.join(name);
            std::fs::write(&path, &file.contents).map_err(io_error)?;
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
/// stdout and stderr tail; the caller kills the tree and reaps the child.
async fn drive(
    tree: &mut ProcessTree,
    stdin: Vec<u8>,
    deadline: Instant,
) -> Result<(Vec<u8>, Vec<u8>), ProviderError> {
    let stdin_pipe = tree.take_stdin();
    let stdout_pipe = tree.take_stdout();
    let stderr_pipe = tree.take_stderr();

    let mut stdout = Vec::new();
    let mut stderr_tail = Vec::new();
    let mut exited = false;
    let mut stdout_done = stdout_pipe.is_none();
    let mut stderr_done = stderr_pipe.is_none();
    let mut stdin_done = stdin_pipe.is_none();

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
                () = &mut stdin_fut, if !stdin_done => stdin_done = true,
            }
        }
    };

    match outcome {
        Ok(()) if Instant::now() >= deadline => Err(ProviderError::Timeout),
        Ok(()) => Ok((stdout, stderr_tail)),
        Err(Abort::Timeout) => Err(ProviderError::Timeout),
        Err(Abort::TooLarge) => Err(ProviderError::other(ProviderErrorCode::OutputTooLarge)),
        Err(Abort::Io) => Err(ProviderError::other(ProviderErrorCode::Io)),
    }
}

/// Writes `data` and closes stdin. Errors are ignored: a CLI may exit or
/// close stdin without reading it, and its exit status tells the rest.
async fn write_stdin(pipe: Option<ChildStdin>, data: Vec<u8>) {
    if let Some(mut pipe) = pipe
        && pipe.write_all(&data).await.is_ok()
    {
        let _ = pipe.shutdown().await;
    }
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
