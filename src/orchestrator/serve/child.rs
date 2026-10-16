//! One generation of the managed transcription child
//! (`pumice-transcribe serve --managed`, p13.1-flow-seam/D4): spawned
//! through the relay's [`ProcessTree`], with its piped stdin kept open as
//! the lifeline, its stdout read as bounded lifecycle JSONL and its stderr
//! drained and discarded.
//!
//! The protocol is the one `docs/transcription.md` ("Managed mode")
//! documents: `ready` (or `error`), then `stopping` and `stopped`, each one
//! JSON object per line under 4096 bytes. Anything else — an oversized or
//! malformed line, an unknown or duplicate record, another instance's id —
//! is a protocol error. Record contents are never logged; an `error`
//! record's message is not even kept.

use std::ffi::OsString;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Limited};
use serde::Deserialize;
use serde::de::IgnoredAny;
use tokio::io::AsyncReadExt;
use tokio::process::{ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::mpsc;
use tokio::time::Instant;

use pumice_relay::process::ProcessTree;

use crate::orchestrator::cancel::{self, CancelGuard};

/// Managed protocol version this core speaks.
pub const PROTOCOL_VERSION: u32 = 1;
/// Capabilities the core needs from its child (p13.1-flow-seam/D4).
pub const REQUIRED_CAPABILITIES: [&str; 3] = [
    "openai_transcriptions",
    "detailed_transcriptions",
    "managed_shutdown",
];
/// One lifecycle line, newline excluded, is shorter than this.
pub const MAX_RECORD_BYTES: usize = 4096;
/// A run writes at most `ready`/`error`, `stopping` and `stopped`; a few
/// more lines than that are already a protocol error.
const MAX_RECORDS: usize = 4;
/// After the child exits, its already written records are read this long
/// (at most [`MAX_RECORDS`] lines of [`MAX_RECORD_BYTES`] each).
const EXIT_GRACE: Duration = Duration::from_millis(250);
/// The child's `/ready` body is a small identity object.
const MAX_READY_BODY_BYTES: usize = 16 * 1024;

/// What the core asks a generation to serve.
#[derive(Clone, Debug)]
pub struct ChildSpec {
    /// The sibling `pumice-transcribe`, resolved like `pumice run` does.
    pub binary: PathBuf,
    /// The absolute transcription configuration.
    pub config: PathBuf,
    /// The served model id.
    pub model: String,
}

impl ChildSpec {
    fn args(&self, instance_id: &str) -> Vec<OsString> {
        vec![
            OsString::from("--config"),
            self.config.clone().into_os_string(),
            OsString::from("serve"),
            OsString::from("--model"),
            OsString::from(&self.model),
            OsString::from("--port"),
            OsString::from("0"),
            OsString::from("--managed"),
            OsString::from("--instance-id"),
            OsString::from(instance_id),
        ]
    }
}

/// A fresh instance id for `generation`: parent-generated launch metadata,
/// not a secret (design §4). Always within the child's id rules (1–64
/// characters from `A-Za-z0-9._-`).
pub fn instance_id(generation: u64) -> String {
    format!(
        "g{generation}-{}",
        crate::orchestrator::events::operation_id()
    )
}

/// What a ready generation announced, already checked against the spec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadyInfo {
    pub port: u16,
    pub model_load_ms: u64,
}

/// Why a generation is not (or no longer) usable. `code` is a fixed,
/// content-free identifier; `fatal` failures stop recovery at once
/// (p13.1-flow-seam/D5: protocol, version or configuration errors).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Failure {
    pub code: &'static str,
    pub fatal: bool,
    /// The child's own `error` record code, when it sent one (validated
    /// against the module's known codes).
    pub child_code: Option<&'static str>,
}

impl Failure {
    pub const fn fatal(code: &'static str) -> Failure {
        Failure {
            code,
            fatal: true,
            child_code: None,
        }
    }

    pub const fn transient(code: &'static str) -> Failure {
        Failure {
            code,
            fatal: false,
            child_code: None,
        }
    }
}

/// Failure codes.
pub mod code {
    pub const NOT_INSTALLED: &str = "transcription_not_installed";
    pub const SPAWN_FAILED: &str = "transcription_spawn_failed";
    pub const STARTUP_TIMEOUT: &str = "transcription_startup_timeout";
    pub const EXITED: &str = "transcription_exited";
    pub const MODULE_ERROR: &str = "transcription_module_error";
    pub const PROTOCOL: &str = "transcription_protocol_error";
    pub const VERSION_MISMATCH: &str = "transcription_version_mismatch";
    pub const MODEL_MISMATCH: &str = "transcription_model_mismatch";
    pub const CAPABILITIES_MISSING: &str = "transcription_capabilities_missing";
    pub const ADDRESS_INVALID: &str = "transcription_address_invalid";
    pub const READY_CHECK_FAILED: &str = "transcription_ready_check_failed";
    pub const PROBE_FAILED: &str = "transcription_probe_failed";
    pub const RECYCLED: &str = "transcription_recycled";
    pub const CLEANUP_FAILED: &str = "transcription_cleanup_failed";
    pub const STOPPED: &str = "transcription_stopped";
}

/// The transcription module's error codes a startup `error` record may
/// carry; the ones that are configuration mistakes are fatal.
fn known_child_code(code: &str) -> Option<(&'static str, bool)> {
    Some(match code {
        "invalid_arguments" => ("invalid_arguments", true),
        "invalid_config" => ("invalid_config", true),
        "model_not_found" => ("model_not_found", true),
        "engine_not_available" => ("engine_not_available", true),
        "model_load_failed" => ("model_load_failed", false),
        "engine_failed" => ("engine_failed", false),
        "internal" => ("internal", false),
        _ => return None,
    })
}

/// One lifecycle event from the stdout reader.
#[derive(Debug, PartialEq, Eq)]
enum Lifecycle {
    Ready(ReadyRecord),
    Error(Failure),
    Stopping,
    Stopped,
    /// The stream broke the protocol; nothing more is read.
    Protocol,
    /// stdout closed.
    Eof,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReadyRecord {
    protocol_version: u32,
    version: String,
    host: String,
    port: u16,
    model: String,
    capabilities: Vec<String>,
    model_load_ms: u64,
}

/// The records as the child writes them. Field sets are exact: both sides
/// come from the same release (the exact package version is checked).
#[derive(Deserialize)]
#[serde(tag = "event", deny_unknown_fields)]
enum RawRecord {
    #[serde(rename = "ready")]
    Ready {
        schema_version: u32,
        module: String,
        instance_id: String,
        protocol_version: u32,
        version: String,
        host: String,
        port: u16,
        model: String,
        capabilities: Vec<String>,
        model_load_ms: u64,
    },
    #[serde(rename = "error")]
    Error {
        schema_version: u32,
        module: String,
        #[serde(default)]
        instance_id: Option<String>,
        code: String,
        // Never kept: it may carry native or decoder text.
        #[allow(dead_code)]
        message: IgnoredAny,
    },
    #[serde(rename = "stopping")]
    Stopping {
        schema_version: u32,
        module: String,
        instance_id: String,
        reason: String,
    },
    #[serde(rename = "stopped")]
    Stopped {
        schema_version: u32,
        module: String,
        instance_id: String,
    },
}

/// The ordered protocol: `ready` or `error` first, then `stopping`, then
/// `stopped`; nothing twice and nothing out of order.
struct RecordParser {
    instance_id: String,
    seen_ready: bool,
    seen_error: bool,
    seen_stopping: bool,
    seen_stopped: bool,
    records: usize,
}

impl RecordParser {
    fn new(instance_id: &str) -> RecordParser {
        RecordParser {
            instance_id: instance_id.to_owned(),
            seen_ready: false,
            seen_error: false,
            seen_stopping: false,
            seen_stopped: false,
            records: 0,
        }
    }

    /// One line, newline already removed; `Lifecycle::Protocol` ends the
    /// stream.
    fn line(&mut self, line: &[u8]) -> Lifecycle {
        self.records += 1;
        if self.records > MAX_RECORDS || line.len() >= MAX_RECORD_BYTES {
            return Lifecycle::Protocol;
        }
        let Ok(record) = serde_json::from_slice::<RawRecord>(line) else {
            return Lifecycle::Protocol;
        };
        let header_ok =
            |schema_version: u32, module: &str| schema_version == 1 && module == "transcription";
        match record {
            RawRecord::Ready {
                schema_version,
                module,
                instance_id,
                protocol_version,
                version,
                host,
                port,
                model,
                capabilities,
                model_load_ms,
            } => {
                if !header_ok(schema_version, &module)
                    || instance_id != self.instance_id
                    || self.seen_ready
                    || self.seen_error
                    || self.seen_stopping
                    || self.seen_stopped
                {
                    return Lifecycle::Protocol;
                }
                self.seen_ready = true;
                Lifecycle::Ready(ReadyRecord {
                    protocol_version,
                    version,
                    host,
                    port,
                    model,
                    capabilities,
                    model_load_ms,
                })
            }
            RawRecord::Error {
                schema_version,
                module,
                instance_id,
                code,
                message: _,
            } => {
                let id_ok = instance_id
                    .as_deref()
                    .is_none_or(|id| id == self.instance_id);
                if !header_ok(schema_version, &module)
                    || !id_ok
                    || self.seen_ready
                    || self.seen_error
                {
                    return Lifecycle::Protocol;
                }
                self.seen_error = true;
                match known_child_code(&code) {
                    Some((child_code, fatal)) => Lifecycle::Error(Failure {
                        code: code::MODULE_ERROR,
                        fatal,
                        child_code: Some(child_code),
                    }),
                    None => Lifecycle::Protocol,
                }
            }
            RawRecord::Stopping {
                schema_version,
                module,
                instance_id,
                reason,
            } => {
                if !header_ok(schema_version, &module)
                    || instance_id != self.instance_id
                    || !matches!(reason.as_str(), "stdin_closed" | "signal")
                    || self.seen_stopping
                    || self.seen_stopped
                {
                    return Lifecycle::Protocol;
                }
                self.seen_stopping = true;
                Lifecycle::Stopping
            }
            RawRecord::Stopped {
                schema_version,
                module,
                instance_id,
            } => {
                if !header_ok(schema_version, &module)
                    || instance_id != self.instance_id
                    || self.seen_stopped
                {
                    return Lifecycle::Protocol;
                }
                self.seen_stopped = true;
                Lifecycle::Stopped
            }
        }
    }
}

/// Reads the child's stdout as lifecycle lines and forwards each event;
/// stops at EOF or the first protocol error (the child is then killed by
/// its owner, so an unread pipe cannot stall anything for long).
async fn read_lifecycle(
    mut stdout: ChildStdout,
    instance_id: String,
    events: mpsc::Sender<Lifecycle>,
) {
    let mut parser = RecordParser::new(&instance_id);
    let mut pending: Vec<u8> = Vec::with_capacity(MAX_RECORD_BYTES);
    let mut chunk = [0u8; 1024];
    loop {
        let n = match stdout.read(&mut chunk).await {
            Ok(0) => {
                // A partial last line is a broken record.
                let last = if pending.is_empty() {
                    Lifecycle::Eof
                } else {
                    Lifecycle::Protocol
                };
                let _ = events.send(last).await;
                return;
            }
            Ok(n) => n,
            Err(_) => {
                let _ = events.send(Lifecycle::Protocol).await;
                return;
            }
        };
        for &byte in &chunk[..n] {
            if byte == b'\n' {
                let event = parser.line(&pending);
                pending.clear();
                let stop = event == Lifecycle::Protocol;
                if events.send(event).await.is_err() || stop {
                    return;
                }
            } else {
                if pending.len() + 1 >= MAX_RECORD_BYTES {
                    let _ = events.send(Lifecycle::Protocol).await;
                    return;
                }
                pending.push(byte);
            }
        }
    }
}

/// Discards the child's stderr continuously, so a chatty child never
/// blocks on a full pipe. Never shown or logged.
async fn drain_stderr(mut stderr: ChildStderr) {
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        match stderr.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

/// Aborts a task when dropped.
struct TaskGuard(tokio::task::JoinHandle<()>);

impl Drop for TaskGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// A spawned generation, owned until reaped. Dropping it without
/// [`Generation::kill_and_reap`] or [`Generation::stop`] still kills the
/// whole tree (the [`ProcessTree`]'s kill-on-drop).
pub struct Generation {
    pub generation: u64,
    pub instance_id: String,
    tree: ProcessTree,
    stdin: Option<ChildStdin>,
    lifecycle: mpsc::Receiver<Lifecycle>,
    lifecycle_done: bool,
    /// When the direct child's exit was observed.
    exited_at: Option<Instant>,
    ready: Option<ReadyInfo>,
    _stdout_task: TaskGuard,
    _stderr_task: TaskGuard,
}

impl std::fmt::Debug for Generation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Generation")
            .field("generation", &self.generation)
            .field("ready", &self.ready)
            .finish_non_exhaustive()
    }
}

/// What [`Generation::watch`] observed.
#[derive(Debug, PartialEq, Eq)]
pub enum Watched {
    /// The direct child exited.
    Exited,
    /// stdout broke the protocol.
    Protocol,
}

impl Generation {
    /// Spawns `spec` for `generation` with a fresh instance id.
    pub fn spawn(spec: &ChildSpec, generation: u64) -> Result<Generation, Failure> {
        let instance_id = instance_id(generation);
        let mut command = tokio::process::Command::new(&spec.binary);
        command
            .args(spec.args(&instance_id))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut tree = ProcessTree::spawn(command).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => Failure::fatal(code::NOT_INSTALLED),
            _ => Failure::transient(code::SPAWN_FAILED),
        })?;
        let stdin = tree.take_stdin();
        let (Some(stdout), Some(stderr)) = (tree.take_stdout(), tree.take_stderr()) else {
            // Piped above; a missing pipe is a spawn failure. The tree is
            // killed on drop.
            return Err(Failure::transient(code::SPAWN_FAILED));
        };
        let (sender, lifecycle) = mpsc::channel(MAX_RECORDS + 2);
        let stdout_task = tokio::spawn(read_lifecycle(stdout, instance_id.clone(), sender));
        let stderr_task = tokio::spawn(drain_stderr(stderr));
        Ok(Generation {
            generation,
            instance_id,
            tree,
            stdin,
            lifecycle,
            lifecycle_done: false,
            exited_at: None,
            ready: None,
            _stdout_task: TaskGuard(stdout_task),
            _stderr_task: TaskGuard(stderr_task),
        })
    }

    pub fn ready(&self) -> Option<&ReadyInfo> {
        self.ready.as_ref()
    }

    /// Awaits the `ready` record until `deadline`, checks it against `spec`
    /// and then the child's own `GET /ready`. `Err(None)` means `stop`
    /// fired first. The generation stays owned either way: the caller
    /// stops or kills it.
    pub async fn handshake(
        &mut self,
        spec: &ChildSpec,
        deadline: Instant,
        stop: &CancelGuard,
    ) -> Result<ReadyInfo, Option<Failure>> {
        let mut stop = stop.clone();
        // After the child exits, what it already wrote is read for a short
        // grace (a descendant could hold stdout open), then it has failed.
        let mut limit = deadline;
        let record = loop {
            tokio::select! {
                biased;
                _ = cancel::cancelled(&mut stop) => return Err(None),
                () = tokio::time::sleep_until(limit) => {
                    let code = if self.exited_at.is_some() {
                        code::EXITED
                    } else {
                        code::STARTUP_TIMEOUT
                    };
                    return Err(Some(Failure::transient(code)));
                }
                event = self.lifecycle.recv(), if !self.lifecycle_done => match event {
                    Some(Lifecycle::Ready(record)) => break record,
                    Some(Lifecycle::Error(failure)) => return Err(Some(failure)),
                    Some(Lifecycle::Protocol) => {
                        self.lifecycle_done = true;
                        return Err(Some(Failure::fatal(code::PROTOCOL)));
                    }
                    // A shutdown before `ready` that the core did not ask
                    // for: the child is on its way out.
                    Some(Lifecycle::Stopping | Lifecycle::Stopped) => {
                        return Err(Some(Failure::transient(code::EXITED)));
                    }
                    Some(Lifecycle::Eof) | None => {
                        self.lifecycle_done = true;
                    }
                },
                result = self.tree.exited(), if self.exited_at.is_none() => {
                    self.exited_at = Some(Instant::now());
                    if result.is_err() || self.lifecycle_done {
                        return Err(Some(Failure::transient(code::EXITED)));
                    }
                    limit = deadline.min(Instant::now() + EXIT_GRACE);
                }
            }
            if self.exited_at.is_some() && self.lifecycle_done {
                return Err(Some(Failure::transient(code::EXITED)));
            }
        };
        let info = check_ready_record(&record, spec)?;
        let check = tokio::select! {
            biased;
            _ = cancel::cancelled(&mut stop) => return Err(None),
            check = probe_ready(info.port, &self.instance_id, &spec.model, deadline) => check,
        };
        match check {
            Ok(()) => {
                self.ready = Some(info.clone());
                Ok(info)
            }
            Err(ProbeError::Mismatch) => Err(Some(Failure::fatal(code::READY_CHECK_FAILED))),
            Err(ProbeError::Timeout) if Instant::now() >= deadline => {
                Err(Some(Failure::transient(code::STARTUP_TIMEOUT)))
            }
            Err(_) => Err(Some(Failure::transient(code::READY_CHECK_FAILED))),
        }
    }

    /// Waits until the ready child exits or breaks its lifecycle protocol.
    /// `stopping`/`stopped` records and stdout EOF are expected on the way
    /// out and only noted. After the exit, what the child already wrote is
    /// still read for [`EXIT_GRACE`], so a protocol error it wrote before
    /// exiting stays a protocol error (fatal, p13.1-flow-seam/D5) instead of
    /// a plain exit. Cancellation-safe: the grace is anchored to the exit.
    pub async fn watch(&mut self) -> Watched {
        loop {
            if let Some(exited_at) = self.exited_at {
                if self.lifecycle_done {
                    return Watched::Exited;
                }
                tokio::select! {
                    biased;
                    event = self.lifecycle.recv() => match event {
                        Some(Lifecycle::Protocol | Lifecycle::Ready(_) | Lifecycle::Error(_)) => {
                            self.lifecycle_done = true;
                            return Watched::Protocol;
                        }
                        Some(Lifecycle::Stopping | Lifecycle::Stopped) => {}
                        Some(Lifecycle::Eof) | None => self.lifecycle_done = true,
                    },
                    // A descendant may hold stdout open.
                    () = tokio::time::sleep_until(exited_at + EXIT_GRACE) => return Watched::Exited,
                }
                continue;
            }
            tokio::select! {
                result = self.tree.exited() => {
                    // An error polling the child is treated as its exit: the
                    // owner kills and reaps the tree next.
                    let _ = result;
                    self.exited_at = Some(Instant::now());
                }
                event = self.lifecycle.recv(), if !self.lifecycle_done => match event {
                    Some(Lifecycle::Protocol | Lifecycle::Ready(_) | Lifecycle::Error(_)) => {
                        self.lifecycle_done = true;
                        return Watched::Protocol;
                    }
                    Some(Lifecycle::Stopping | Lifecycle::Stopped) => {}
                    Some(Lifecycle::Eof) | None => self.lifecycle_done = true,
                },
            }
        }
    }

    /// Kills the whole tree now and reaps the direct child within `limit`.
    /// `Err(self)` when it could not be reaped: the tree stays owned (and
    /// killed) so the caller can retry instead of losing track of it.
    pub async fn kill_and_reap(mut self, limit: Duration) -> Result<(), Box<Generation>> {
        self.stdin = None;
        match self.tree.finish(limit).await {
            Some(_) => Ok(()),
            None => Err(Box::new(self)),
        }
    }

    /// The cooperative stop (p13.1-flow-seam/D9): close stdin, give the
    /// child `grace` to exit on its own, then kill and reap the tree within
    /// `reap_limit`.
    pub async fn stop(
        mut self,
        grace: Duration,
        reap_limit: Duration,
    ) -> Result<(), Box<Generation>> {
        self.stdin = None;
        if self.exited_at.is_none() {
            let _ = tokio::time::timeout(grace, self.tree.exited()).await;
        }
        self.kill_and_reap(reap_limit).await
    }
}

/// Checks a `ready` record against what was asked for.
fn check_ready_record(
    record: &ReadyRecord,
    spec: &ChildSpec,
) -> Result<ReadyInfo, Option<Failure>> {
    if record.protocol_version != PROTOCOL_VERSION {
        return Err(Some(Failure::fatal(code::PROTOCOL)));
    }
    if record.version != env!("CARGO_PKG_VERSION") {
        return Err(Some(Failure::fatal(code::VERSION_MISMATCH)));
    }
    if record.model != spec.model {
        return Err(Some(Failure::fatal(code::MODEL_MISMATCH)));
    }
    if !REQUIRED_CAPABILITIES
        .iter()
        .all(|needed| record.capabilities.iter().any(|have| have == needed))
    {
        return Err(Some(Failure::fatal(code::CAPABILITIES_MISSING)));
    }
    if record.host != "127.0.0.1" || record.port == 0 {
        return Err(Some(Failure::fatal(code::ADDRESS_INVALID)));
    }
    Ok(ReadyInfo {
        port: record.port,
        model_load_ms: record.model_load_ms,
    })
}

/// Why a `GET /ready` on the child did not confirm it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeError {
    /// No answer before the deadline.
    Timeout,
    /// Connection refused or broken, or a non-200 status.
    Unavailable,
    /// A 200 answer with someone else's identity, or not the documented
    /// shape: a protocol error.
    Mismatch,
}

/// The child's `/ready` body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadyBody {
    schema_version: u32,
    module: String,
    instance_id: String,
    protocol_version: u32,
    version: String,
    model: String,
    capabilities: Vec<String>,
    state: String,
}

/// `GET /ready` on the child at `127.0.0.1:<port>` (the only address the
/// core ever builds: a validated numeric port, never request content),
/// with the literal loopback `Host` and no redirects, proxy or `Origin`.
pub async fn probe_ready(
    port: u16,
    instance_id: &str,
    model: &str,
    deadline: Instant,
) -> Result<(), ProbeError> {
    let (status, body) = tokio::time::timeout_at(deadline, get_ready(port))
        .await
        .map_err(|_| ProbeError::Timeout)??;
    if status != hyper::StatusCode::OK {
        return Err(ProbeError::Unavailable);
    }
    let Ok(body) = serde_json::from_slice::<ReadyBody>(&body) else {
        return Err(ProbeError::Mismatch);
    };
    let matches = body.schema_version == 1
        && body.module == "transcription"
        && body.instance_id == instance_id
        && body.protocol_version == PROTOCOL_VERSION
        && body.version == env!("CARGO_PKG_VERSION")
        && body.model == model
        && REQUIRED_CAPABILITIES
            .iter()
            .all(|needed| body.capabilities.iter().any(|have| have == needed))
        && body.state == "ready";
    if matches {
        Ok(())
    } else {
        Err(ProbeError::Mismatch)
    }
}

async fn get_ready(port: u16) -> Result<(hyper::StatusCode, Bytes), ProbeError> {
    let stream = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        .await
        .map_err(|_| ProbeError::Unavailable)?;
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream))
            .await
            .map_err(|_| ProbeError::Unavailable)?;
    // The connection driver runs beside the request; the guard stops it
    // when this probe ends or is dropped.
    let _driver = TaskGuard(tokio::spawn(async move {
        let _ = connection.await;
    }));
    let request = hyper::Request::get("/ready")
        .header(hyper::header::HOST, format!("127.0.0.1:{port}"))
        .header(hyper::header::CONNECTION, "close")
        .body(Empty::<Bytes>::new())
        .map_err(|_| ProbeError::Unavailable)?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|_| ProbeError::Unavailable)?;
    let status = response.status();
    let body = Limited::new(response.into_body(), MAX_READY_BODY_BYTES)
        .collect()
        .await
        .map_err(|_| ProbeError::Unavailable)?
        .to_bytes();
    Ok((status, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready_line(id: &str) -> String {
        format!(
            r#"{{"schema_version":1,"event":"ready","module":"transcription","instance_id":"{id}","protocol_version":1,"version":"{}","host":"127.0.0.1","port":49152,"model":"turbo","capabilities":["openai_transcriptions","detailed_transcriptions","managed_shutdown"],"model_load_ms":362}}"#,
            env!("CARGO_PKG_VERSION")
        )
    }

    fn spec() -> ChildSpec {
        ChildSpec {
            binary: PathBuf::from("pumice-transcribe"),
            config: PathBuf::from("/cfg/transcribe.yml"),
            model: "turbo".to_owned(),
        }
    }

    #[test]
    fn instance_ids_follow_the_child_rules() {
        let id = instance_id(7);
        assert!(id.starts_with("g7-"), "{id}");
        assert!((1..=64).contains(&id.len()));
        assert!(
            id.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        );
        assert_ne!(instance_id(7), instance_id(7), "fresh per launch");
    }

    #[test]
    fn the_child_arguments_are_the_managed_serve_call() {
        let args = spec().args("g1-x");
        let args: Vec<_> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "--config",
                "/cfg/transcribe.yml",
                "serve",
                "--model",
                "turbo",
                "--port",
                "0",
                "--managed",
                "--instance-id",
                "g1-x"
            ]
        );
    }

    #[test]
    fn a_full_lifecycle_parses_in_order() {
        let mut parser = RecordParser::new("g1-a");
        let Lifecycle::Ready(record) = parser.line(ready_line("g1-a").as_bytes()) else {
            panic!("ready expected");
        };
        assert_eq!(record.port, 49152);
        assert_eq!(
            check_ready_record(&record, &spec()).unwrap().model_load_ms,
            362
        );
        assert_eq!(
            parser.line(br#"{"schema_version":1,"event":"stopping","module":"transcription","instance_id":"g1-a","reason":"stdin_closed"}"#),
            Lifecycle::Stopping
        );
        assert_eq!(
            parser.line(br#"{"schema_version":1,"event":"stopped","module":"transcription","instance_id":"g1-a"}"#),
            Lifecycle::Stopped
        );
    }

    #[test]
    fn malformed_duplicate_foreign_and_unknown_records_are_protocol_errors() {
        let ready = ready_line("g1-a");
        let error_after_ready = br#"{"schema_version":1,"event":"error","module":"transcription","code":"internal","message":"x"}"#;
        let cases: Vec<(Vec<Vec<u8>>, &str)> = vec![
            (vec![b"not json".to_vec()], "malformed"),
            (vec![br#"{"event":"hello"}"#.to_vec()], "unknown event"),
            (vec![ready.clone().into_bytes(), ready.clone().into_bytes()], "duplicate ready"),
            (vec![ready_line("g2-b").into_bytes()], "another instance"),
            (
                vec![ready.replace("\"port\":49152", "\"port\":49152,\"extra\":1").into_bytes()],
                "unknown field",
            ),
            (
                vec![ready.replace("\"module\":\"transcription\"", "\"module\":\"relay\"").into_bytes()],
                "wrong module",
            ),
            (
                vec![br#"{"schema_version":1,"event":"error","module":"transcription","code":"made_up","message":"x"}"#.to_vec()],
                "unknown error code",
            ),
            (
                vec![ready.clone().into_bytes(), error_after_ready.to_vec()],
                "error after ready",
            ),
        ];
        for (lines, what) in cases {
            let mut parser = RecordParser::new("g1-a");
            let last = lines.iter().map(|line| parser.line(line)).last().unwrap();
            assert_eq!(last, Lifecycle::Protocol, "{what}");
        }

        let mut parser = RecordParser::new("g1-a");
        assert_eq!(
            parser.line(&vec![b' '; MAX_RECORD_BYTES]),
            Lifecycle::Protocol,
            "oversized line"
        );
        let mut parser = RecordParser::new("g1-a");
        let stopped =
            br#"{"schema_version":1,"event":"stopped","module":"transcription","instance_id":"g1-a"}"#;
        for _ in 0..MAX_RECORDS {
            parser.seen_stopped = false;
            let _ = parser.line(stopped);
        }
        parser.seen_stopped = false;
        assert_eq!(
            parser.line(stopped),
            Lifecycle::Protocol,
            "record count cap"
        );
    }

    #[test]
    fn startup_error_records_keep_only_a_known_code() {
        let mut parser = RecordParser::new("g1-a");
        let event = parser.line(br#"{"schema_version":1,"event":"error","module":"transcription","instance_id":"g1-a","code":"model_not_found","message":"S3NT1NEL"}"#);
        assert_eq!(
            event,
            Lifecycle::Error(Failure {
                code: code::MODULE_ERROR,
                fatal: true,
                child_code: Some("model_not_found"),
            })
        );
        // Without an instance id (invalid arguments), still accepted.
        let mut parser = RecordParser::new("g1-a");
        let event = parser.line(br#"{"schema_version":1,"event":"error","module":"transcription","code":"model_load_failed","message":""}"#);
        assert!(matches!(
            event,
            Lifecycle::Error(Failure { fatal: false, .. })
        ));
    }

    #[test]
    fn ready_records_are_checked_against_the_spec() {
        let mut parser = RecordParser::new("g1-a");
        let Lifecycle::Ready(good) = parser.line(ready_line("g1-a").as_bytes()) else {
            panic!("ready expected");
        };
        let check = |edit: &dyn Fn(&mut ReadyRecord)| {
            let mut record = good.clone();
            edit(&mut record);
            check_ready_record(&record, &spec())
                .unwrap_err()
                .unwrap()
                .code
        };
        assert_eq!(check(&|r| r.protocol_version = 2), code::PROTOCOL);
        assert_eq!(
            check(&|r| r.version = "9.9.9".into()),
            code::VERSION_MISMATCH
        );
        assert_eq!(check(&|r| r.model = "small".into()), code::MODEL_MISMATCH);
        assert_eq!(
            check(&|r| r.capabilities.retain(|c| c != "managed_shutdown")),
            code::CAPABILITIES_MISSING
        );
        assert_eq!(check(&|r| r.host = "0.0.0.0".into()), code::ADDRESS_INVALID);
        assert_eq!(check(&|r| r.port = 0), code::ADDRESS_INVALID);
    }
}
