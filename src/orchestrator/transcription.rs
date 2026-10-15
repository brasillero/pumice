//! Running the sibling `pumice-transcribe` program for `pumice run`
//! (p12-pumice-run/D1): spawn only, never exec — the parent stays alive to
//! enforce the deadline and to cancel the child.
//!
//! The sibling is the exact executable resolved next to the running `pumice`
//! by the dispatcher (never PATH). Its stdout carries one JSON envelope
//! (`schema_version: 1`), capped and validated here; its stderr is drained
//! and discarded, never logged. The child runs in its own process group /
//! Job Object (the relay's [`ProcessTree`]), so a deadline, a cancellation
//! or an oversized output kills the whole tree and reaps it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tokio::time::Instant;

use pumice_relay::process::ProcessTree;

use crate::orchestrator::cancel::{self, CancelGuard, CancelReason};
use crate::orchestrator::result::ErrorLocation;

/// Maximum stdout captured from the transcription child: 1 MiB.
const MAX_STDOUT_BYTES: usize = 1024 * 1024;
/// Grace after the direct child exits before its descendants' pipe output is
/// abandoned (mirrors the relay runner).
const EXIT_GRACE: Duration = Duration::from_millis(250);
/// Upper bound on reaping the child after the tree has been killed.
const REAP_LIMIT: Duration = Duration::from_secs(2);

/// What a successful `file --json` call returned.
pub struct TranscriptOutput {
    pub text: String,
    pub language: Option<String>,
    pub audio_seconds: f64,
    pub model_load_ms: Option<u64>,
    pub transcription_ms: Option<u64>,
}

/// One entry of `models list --json`.
pub struct ModelEntry {
    pub id: String,
    pub exists: bool,
}

/// How a call into the transcription sibling failed. Child error *messages*
/// are module-generated and safe to show; anything malformed collapses to a
/// fixed `Protocol` message instead.
#[derive(Debug)]
pub enum SttFailure {
    /// The sibling executable was not found or could not start.
    NotInstalled,
    /// The orchestrator's deadline fired; the tree was killed and reaped.
    Timeout,
    /// The run was cancelled; the tree was killed and reaped.
    Cancelled(CancelReason),
    /// The child spoke no valid envelope (bad JSON, wrong version, extra
    /// documents, oversized output, exit status disagreeing with `ok`, …).
    Protocol(&'static str),
    /// The child reported a typed error envelope (`ok: false`).
    Module {
        code: String,
        message: Option<String>,
        location: Option<ErrorLocation>,
    },
}

/// The sibling executable and the transcription config it is given.
pub struct Transcribe {
    binary: PathBuf,
    config: PathBuf,
}

impl Transcribe {
    pub fn new(binary: PathBuf, config: PathBuf) -> Transcribe {
        Transcribe { binary, config }
    }

    /// `pumice-transcribe --config <FILE> models list --json`.
    pub async fn models_list(
        &self,
        deadline: Instant,
        cancel: &CancelGuard,
    ) -> Result<Vec<ModelEntry>, SttFailure> {
        let (status, stdout) = self
            .run_child(
                &[
                    OsString::from("--config"),
                    self.config.clone().into_os_string(),
                    OsString::from("models"),
                    OsString::from("list"),
                    OsString::from("--json"),
                ],
                deadline,
                cancel,
            )
            .await?;
        let envelope = parse_envelope(&stdout)?;
        match envelope.ok {
            true => {
                if !status.success() {
                    return Err(SttFailure::Protocol(
                        "the transcription module reported success but exited nonzero",
                    ));
                }
                let entries: Vec<RawModelEntry> = parse_result(envelope.result)?;
                Ok(entries
                    .into_iter()
                    .map(|entry| ModelEntry {
                        id: entry.id,
                        exists: entry.exists,
                    })
                    .collect())
            }
            false => module_error(envelope, status),
        }
    }

    /// `pumice-transcribe --config <FILE> file <AUDIO> --model <ID> --json`.
    pub async fn transcribe_file(
        &self,
        audio: &Path,
        model: &str,
        deadline: Instant,
        cancel: &CancelGuard,
    ) -> Result<TranscriptOutput, SttFailure> {
        let (status, stdout) = self
            .run_child(
                &[
                    OsString::from("--config"),
                    self.config.clone().into_os_string(),
                    OsString::from("file"),
                    audio.as_os_str().to_owned(),
                    OsString::from("--model"),
                    OsString::from(model),
                    OsString::from("--json"),
                ],
                deadline,
                cancel,
            )
            .await?;
        let envelope = parse_envelope(&stdout)?;
        match envelope.ok {
            true => {
                if !status.success() {
                    return Err(SttFailure::Protocol(
                        "the transcription module reported success but exited nonzero",
                    ));
                }
                let result: RawFileResult = parse_result(envelope.result)?;
                if !result.audio_seconds.is_finite() || result.audio_seconds < 0.0 {
                    return Err(SttFailure::Protocol(
                        "the transcription module reported an invalid audio duration",
                    ));
                }
                Ok(TranscriptOutput {
                    text: result.text,
                    language: result.language,
                    audio_seconds: result.audio_seconds,
                    model_load_ms: result.model_load_ms,
                    transcription_ms: result.transcription_ms,
                })
            }
            false => module_error(envelope, status),
        }
    }

    /// Spawns the sibling and captures its bounded stdout. On timeout and on
    /// cancellation the whole tree is killed and reaped before returning.
    async fn run_child(
        &self,
        args: &[OsString],
        deadline: Instant,
        cancel: &CancelGuard,
    ) -> Result<(std::process::ExitStatus, Vec<u8>), SttFailure> {
        if Instant::now() >= deadline {
            return Err(SttFailure::Timeout);
        }
        let mut command = tokio::process::Command::new(&self.binary);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut tree = ProcessTree::spawn(command).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => SttFailure::NotInstalled,
            _ => SttFailure::Protocol("the transcription module could not be started"),
        })?;

        let outcome = drive(&mut tree, deadline, cancel.clone()).await;
        // On every path: kill what is left (descendants included) and reap.
        let status = tree.finish(REAP_LIMIT).await;
        match outcome {
            DriveOutcome::Exited(stdout) => {
                let Some(status) = status else {
                    return Err(SttFailure::Protocol(
                        "the transcription module could not be reaped",
                    ));
                };
                Ok((status, stdout))
            }
            DriveOutcome::Timeout => Err(SttFailure::Timeout),
            DriveOutcome::Cancelled(reason) => Err(SttFailure::Cancelled(reason)),
            DriveOutcome::Oversize => Err(SttFailure::Protocol(
                "the transcription module returned too much output",
            )),
            DriveOutcome::Io => Err(SttFailure::Protocol(
                "I/O error while reading the transcription module",
            )),
        }
    }
}

enum DriveOutcome {
    Exited(Vec<u8>),
    Timeout,
    Cancelled(CancelReason),
    Oversize,
    Io,
}

/// Why the child driver stopped early.
enum Abort {
    Timeout,
    Cancelled(CancelReason),
    TooLarge,
    Io,
}

/// Drains stdout (capped) and stderr (discarded) while watching the child's
/// exit, the deadline and the cancellation signal, all concurrently.
async fn drive(tree: &mut ProcessTree, deadline: Instant, mut cancel: CancelGuard) -> DriveOutcome {
    let stdout_pipe = tree.take_stdout();
    let stderr_pipe = tree.take_stderr();

    let mut stdout: Vec<u8> = Vec::new();
    let mut stdout_done = stdout_pipe.is_none();
    let mut stderr_done = stderr_pipe.is_none();
    let mut exited = false;

    // The futures borrow the buffer, so whatever was read survives when they
    // are dropped early (at the end of this block).
    let outcome: Result<(), Abort> = {
        let stdout_fut = read_capped(stdout_pipe, &mut stdout);
        let stderr_fut = drain(stderr_pipe);
        tokio::pin!(stdout_fut, stderr_fut);

        let mut limit = deadline;
        loop {
            if exited && stdout_done && stderr_done {
                break Ok(());
            }
            tokio::select! {
                biased;
                reason = cancel::cancelled(&mut cancel) => {
                    break Err(Abort::Cancelled(reason));
                }
                () = tokio::time::sleep_until(limit) => {
                    // Past the grace period (and still before the deadline)
                    // the child has exited: keep what it wrote and let the
                    // caller kill the descendants still holding the pipes.
                    if exited && limit < deadline {
                        break Ok(());
                    }
                    break Err(Abort::Timeout);
                }
                result = tree.exited(), if !exited => match result {
                    Ok(()) => {
                        exited = true;
                        limit = deadline.min(Instant::now() + EXIT_GRACE);
                    }
                    Err(_) => break Err(Abort::Io),
                },
                outcome = &mut stdout_fut, if !stdout_done => match outcome {
                    Ok(()) => stdout_done = true,
                    Err(ReadError::TooLarge) => break Err(Abort::TooLarge),
                    Err(ReadError::Io) => break Err(Abort::Io),
                },
                () = &mut stderr_fut, if !stderr_done => stderr_done = true,
            }
        }
    };

    match outcome {
        Ok(()) => DriveOutcome::Exited(stdout),
        Err(Abort::Timeout) => DriveOutcome::Timeout,
        Err(Abort::Cancelled(reason)) => DriveOutcome::Cancelled(reason),
        Err(Abort::TooLarge) => DriveOutcome::Oversize,
        Err(Abort::Io) => DriveOutcome::Io,
    }
}

enum ReadError {
    TooLarge,
    Io,
}

/// Reads `pipe` to EOF, bounded by [`MAX_STDOUT_BYTES`].
async fn read_capped(
    pipe: Option<tokio::process::ChildStdout>,
    buf: &mut Vec<u8>,
) -> Result<(), ReadError> {
    let Some(mut pipe) = pipe else { return Ok(()) };
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let n = pipe.read(&mut chunk).await.map_err(|_| ReadError::Io)?;
        if n == 0 {
            return Ok(());
        }
        if buf.len() + n > MAX_STDOUT_BYTES {
            return Err(ReadError::TooLarge);
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// Drains `pipe` to EOF, discarding everything: child stderr is never shown
/// or logged (it could contain dictated text from a misbehaving build).
async fn drain(pipe: Option<tokio::process::ChildStderr>) {
    let Some(mut pipe) = pipe else { return };
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

#[derive(Debug, Deserialize)]
struct Envelope {
    schema_version: i64,
    ok: bool,
    result: Option<serde_json::Value>,
    error: Option<EnvelopeError>,
}

#[derive(Debug, Deserialize)]
struct EnvelopeError {
    code: String,
    message: Option<String>,
    location: Option<EnvelopeLocation>,
}

#[derive(Debug, Deserialize)]
struct EnvelopeLocation {
    file: PathBuf,
    line: Option<u64>,
    column: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct RawModelEntry {
    id: String,
    exists: bool,
}

#[derive(Debug, Deserialize)]
struct RawFileResult {
    text: String,
    language: Option<String>,
    audio_seconds: f64,
    model_load_ms: Option<u64>,
    transcription_ms: Option<u64>,
}

/// Parses the child's stdout as exactly one JSON envelope. `serde_json`
/// rejects trailing non-whitespace, so a second document or garbage after
/// the envelope is a protocol error.
fn parse_envelope(stdout: &[u8]) -> Result<Envelope, SttFailure> {
    let text = std::str::from_utf8(stdout).map_err(|_| {
        SttFailure::Protocol("the transcription module's output was not valid UTF-8")
    })?;
    let envelope: Envelope = serde_json::from_str(text).map_err(|_| {
        SttFailure::Protocol("the transcription module's output was not a valid result envelope")
    })?;
    if envelope.schema_version != 1 {
        return Err(SttFailure::Protocol(
            "the transcription module answered with an unsupported envelope version",
        ));
    }
    Ok(envelope)
}

fn parse_result<T: for<'de> Deserialize<'de>>(
    result: Option<serde_json::Value>,
) -> Result<T, SttFailure> {
    let Some(result) = result else {
        return Err(SttFailure::Protocol(
            "the transcription module's envelope has no result",
        ));
    };
    serde_json::from_value(result).map_err(|_| {
        SttFailure::Protocol("the transcription module's result has an unexpected shape")
    })
}

/// A typed `ok: false` envelope. Unknown child codes are never inferred from
/// their text: they become a protocol error (design §4).
fn module_error<T>(envelope: Envelope, status: std::process::ExitStatus) -> Result<T, SttFailure> {
    if status.success() {
        return Err(SttFailure::Protocol(
            "the transcription module reported an error but exited successfully",
        ));
    }
    let Some(error) = envelope.error else {
        return Err(SttFailure::Protocol(
            "the transcription module's error envelope has no error",
        ));
    };
    let known = matches!(
        error.code.as_str(),
        "invalid_arguments"
            | "invalid_input"
            | "input_too_large"
            | "audio_too_long"
            | "unsupported_format"
            | "invalid_config"
            | "model_not_found"
            | "engine_not_available"
            | "engine_failed"
            | "model_load_failed"
            | "internal"
    );
    if !known {
        return Err(SttFailure::Protocol(
            "the transcription module returned an unknown error code",
        ));
    }
    // The module's own messages are designed to be safe; still bound them.
    let message = error.message.map(|message| {
        const MAX_MESSAGE: usize = 400;
        if message.len() > MAX_MESSAGE {
            let mut end = MAX_MESSAGE;
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}…", &message[..end])
        } else {
            message
        }
    });
    Err(SttFailure::Module {
        code: error.code,
        message,
        location: error.location.map(|location| ErrorLocation {
            file: location.file,
            line: location.line,
            column: location.column,
        }),
    })
}
