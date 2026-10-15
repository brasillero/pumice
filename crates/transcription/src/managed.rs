//! Managed mode for `pumice-transcribe serve --managed` (internal protocol v1),
//! used when `pumice serve` owns this process as a warm child.
//!
//! - stdout carries only the lifecycle records below, one JSON object per line
//!   and at most [`MAX_RECORD_BYTES`] each: `ready` (or `error` if startup
//!   fails), then `stopping` and `stopped`. Human logs stay on stderr.
//! - stdin is the owner's lifeline: end of file (or a read error) requests a
//!   graceful shutdown. Bytes sent on stdin are discarded.
//! - Once a shutdown is requested, a watchdog ends the process after
//!   [`SHUTDOWN_GRACE`] even if a native run ignores cancellation.

use crate::{Error, ErrorCode};
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tokio_util::sync::WaitForCancellationFuture;

pub const PROTOCOL_VERSION: u32 = 1;
pub const CAPABILITIES: [&str; 3] = [
    "openai_transcriptions",
    "detailed_transcriptions",
    "managed_shutdown",
];
pub const MAX_RECORD_BYTES: usize = 4096;
pub const MAX_INSTANCE_ID_BYTES: usize = 64;
/// Longest model id a managed instance serves. Model ids are plain ASCII (see
/// the config rules), so with this bound a `ready` record always fits in
/// [`MAX_RECORD_BYTES`] and carries the id unchanged.
pub const MAX_MODEL_ID_BYTES: usize = 512;
/// From a shutdown request to the forced exit.
pub const SHUTDOWN_GRACE: Duration = Duration::from_millis(2000);
/// Exit status when the grace ran out before the server drained.
pub const FORCED_EXIT_CODE: i32 = 3;
/// Error messages in an `error` record are cut to this many bytes.
const MAX_MESSAGE_BYTES: usize = 512;

pub const INVALID_INSTANCE_ID: &str =
    "--instance-id must be 1 to 64 characters from A-Z, a-z, 0-9, '.', '_' and '-'.";
pub const INVALID_ARGUMENTS: &str = "Invalid command-line arguments.";

/// Accept an instance id only if it is short and made of safe characters, so
/// that echoing it can never break a record or a log line.
pub fn validate_instance_id(id: &str) -> Result<&str, &'static str> {
    let valid = (1..=MAX_INSTANCE_ID_BYTES).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if valid {
        Ok(id)
    } else {
        Err(INVALID_INSTANCE_ID)
    }
}

/// Refuse a model id too long to announce. The owner checks the announced id
/// against the one it asked for, so it is never cut short instead.
pub fn validate_model_id(id: &str) -> Result<(), Error> {
    if id.len() <= MAX_MODEL_ID_BYTES {
        Ok(())
    } else {
        Err(Error::new(
            ErrorCode::InvalidConfig,
            format!(
                "the model id is {} bytes long; managed mode serves ids of at most {MAX_MODEL_ID_BYTES} bytes",
                id.len()
            ),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    StdinClosed,
    Signal,
}

impl StopReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::StdinClosed => "stdin_closed",
            Self::Signal => "signal",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Starting,
    Ready,
    Stopping,
    Stopped,
}

struct Output {
    phase: Phase,
    out: Box<dyn Write + Send>,
}

/// One managed instance's lifecycle: its records, its stop request and the
/// watchdog. Records are written under one lock, so `ready` can never follow
/// `stopping`.
pub struct Lifecycle {
    instance_id: String,
    stop: CancellationToken,
    output: Mutex<Output>,
    watchdog: Once,
}

impl Lifecycle {
    /// `instance_id` must have passed [`validate_instance_id`].
    pub fn new(instance_id: &str, out: Box<dyn Write + Send>) -> Arc<Self> {
        Arc::new(Self {
            instance_id: instance_id.to_string(),
            stop: CancellationToken::new(),
            output: Mutex::new(Output {
                phase: Phase::Starting,
                out,
            }),
            watchdog: Once::new(),
        })
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Watch stdin on its own thread. Start it before anything that blocks
    /// (model loading), so an early EOF cannot strand the process.
    pub fn start_lifeline(self: &Arc<Self>) -> io::Result<()> {
        let lifecycle = Arc::clone(self);
        std::thread::Builder::new()
            .name("managed-lifeline".into())
            .spawn(move || {
                let mut stdin = io::stdin().lock();
                let mut buf = [0u8; 256];
                loop {
                    match stdin.read(&mut buf) {
                        Ok(0) => break,
                        Ok(_) => {}
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                        Err(_) => break,
                    }
                }
                lifecycle.request_stop(StopReason::StdinClosed);
            })
            .map(|_| ())
    }

    /// Request a graceful shutdown: arm the watchdog, cancel, and announce
    /// `stopping`. Later requests only keep the first one's reason.
    pub fn request_stop(&self, reason: StopReason) {
        // The watchdog comes first: nothing after it may block the exit.
        self.watchdog.call_once(|| {
            let spawned = std::thread::Builder::new()
                .name("managed-watchdog".into())
                .spawn(|| {
                    std::thread::sleep(SHUTDOWN_GRACE);
                    hard_exit(FORCED_EXIT_CODE);
                });
            // Without a watchdog nothing bounds the shutdown, and `Once` would
            // not try again: exit now rather than risk outliving the grace.
            if spawned.is_err() {
                hard_exit(FORCED_EXIT_CODE);
            }
        });
        self.stop.cancel();
        let mut output = self.lock();
        if matches!(output.phase, Phase::Starting | Phase::Ready) {
            output.phase = Phase::Stopping;
            let record =
                base_record("stopping", Some(&self.instance_id)).with("reason", reason.as_str());
            write_line(&mut output.out, record);
        }
    }

    pub fn is_stopping(&self) -> bool {
        self.stop.is_cancelled()
    }

    pub fn cancelled(&self) -> WaitForCancellationFuture<'_> {
        self.stop.cancelled()
    }

    /// Announce readiness, unless a shutdown was already requested. Returns
    /// whether the record was written.
    pub fn announce_ready(&self, port: u16, model: &str, model_load_ms: u64) -> bool {
        let mut output = self.lock();
        if output.phase != Phase::Starting {
            return false;
        }
        output.phase = Phase::Ready;
        let record = ready_record(&self.instance_id, port, model, model_load_ms);
        write_line(&mut output.out, record);
        true
    }

    /// The server has drained; the process is about to exit normally.
    pub fn announce_stopped(&self) {
        let mut output = self.lock();
        if output.phase != Phase::Stopped {
            output.phase = Phase::Stopped;
            let record = base_record("stopped", Some(&self.instance_id));
            write_line(&mut output.out, record);
        }
    }

    /// Startup failed before readiness.
    pub fn announce_error(&self, error: &Error) {
        let mut output = self.lock();
        if output.phase != Phase::Stopped {
            output.phase = Phase::Stopped;
            let record = error_record(Some(&self.instance_id), error.code, &error.message);
            write_line(&mut output.out, record);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Output> {
        // A panic while writing a record leaves nothing half-updated worth
        // refusing over.
        self.output.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Write an `error` record for a failure before an instance id was accepted.
pub fn write_startup_error(out: &mut dyn Write, code: ErrorCode, message: &str) {
    write_line(out, error_record(None, code, message));
}

struct Record(serde_json::Map<String, serde_json::Value>);

impl Record {
    fn with(mut self, key: &str, value: impl Into<serde_json::Value>) -> Self {
        self.0.insert(key.to_string(), value.into());
        self
    }

    fn to_line(&self) -> String {
        serde_json::Value::Object(self.0.clone()).to_string()
    }
}

fn base_record(event: &str, instance_id: Option<&str>) -> Record {
    let record = Record(serde_json::Map::new())
        .with("schema_version", 1)
        .with("event", event)
        .with("module", "transcription");
    match instance_id {
        Some(id) => record.with("instance_id", id),
        None => record,
    }
}

fn ready_record(instance_id: &str, port: u16, model: &str, model_load_ms: u64) -> Record {
    base_record("ready", Some(instance_id))
        .with("protocol_version", PROTOCOL_VERSION)
        .with("version", env!("CARGO_PKG_VERSION"))
        .with("host", "127.0.0.1")
        .with("port", port)
        .with("model", model)
        .with("capabilities", CAPABILITIES.to_vec())
        .with("model_load_ms", model_load_ms)
}

fn error_record(instance_id: Option<&str>, code: ErrorCode, message: &str) -> Record {
    let message: String = truncate(message, MAX_MESSAGE_BYTES)
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    base_record("error", instance_id)
        .with("code", code.as_str())
        .with("message", message)
}

fn truncate(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Write one record as a single line. A record that would exceed the line
/// bound is never written; every field is bounded, so this is a safeguard.
/// Write errors are ignored: a closed stdout means the owner is gone, which
/// the lifeline reports on its own.
fn write_line(out: &mut dyn Write, record: Record) {
    let mut line = record.to_line();
    debug_assert!(line.len() < MAX_RECORD_BYTES, "lifecycle record too long");
    if line.len() >= MAX_RECORD_BYTES {
        return;
    }
    line.push('\n');
    let _ = out.write_all(line.as_bytes());
    let _ = out.flush();
}

/// End the process at once, from any thread.
fn hard_exit(code: i32) -> ! {
    // SAFETY: `_exit` takes no pointers and never returns. It skips atexit
    // handlers and destructors on purpose: a native thread stuck in inference
    // could block or crash them.
    unsafe { libc::_exit(code) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Shared {
        fn records(&self) -> Vec<serde_json::Value> {
            let bytes = self.0.lock().unwrap().clone();
            String::from_utf8(bytes)
                .unwrap()
                .lines()
                .map(|line| {
                    assert!(line.len() < MAX_RECORD_BYTES);
                    serde_json::from_str(line).unwrap()
                })
                .collect()
        }
    }

    #[test]
    fn instance_ids_are_bounded_and_safe() {
        for good in ["a", "gen-1", "0f8a.b_c-D", &"x".repeat(64)] {
            assert_eq!(validate_instance_id(good), Ok(good));
        }
        for bad in [
            "",
            &"x".repeat(65),
            "has space",
            "quote\"",
            "new\nline",
            "slash/",
            "é",
        ] {
            assert_eq!(validate_instance_id(bad), Err(INVALID_INSTANCE_ID));
        }
    }

    #[test]
    fn ready_record_has_the_protocol_shape() {
        let out = Shared::default();
        let lifecycle = Lifecycle::new("gen-1", Box::new(out.clone()));
        assert!(lifecycle.announce_ready(49152, "turbo", 362));
        let records = out.records();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].to_string(),
            format!(
                r#"{{"schema_version":1,"event":"ready","module":"transcription","instance_id":"gen-1","protocol_version":1,"version":"{}","host":"127.0.0.1","port":49152,"model":"turbo","capabilities":["openai_transcriptions","detailed_transcriptions","managed_shutdown"],"model_load_ms":362}}"#,
                env!("CARGO_PKG_VERSION")
            )
        );
    }

    #[test]
    fn the_longest_accepted_model_id_is_announced_unchanged() {
        let longest = "m".repeat(MAX_MODEL_ID_BYTES);
        assert!(validate_model_id(&longest).is_ok());
        let error = validate_model_id(&format!("{longest}m")).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidConfig);
        assert!(!error.message.contains(&longest));

        let out = Shared::default();
        let id = "i".repeat(MAX_INSTANCE_ID_BYTES);
        let lifecycle = Lifecycle::new(&id, Box::new(out.clone()));
        assert!(lifecycle.announce_ready(u16::MAX, &longest, u64::MAX));
        let records = out.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["model"], longest.as_str());
        assert_eq!(records[0]["instance_id"], id.as_str());
    }

    #[test]
    fn ready_never_follows_stopping_and_records_are_written_once() {
        let out = Shared::default();
        let lifecycle = Lifecycle::new("gen-1", Box::new(out.clone()));
        // Only flip the token here: arming the watchdog would end the test run.
        lifecycle.stop.cancel();
        {
            let mut output = lifecycle.lock();
            output.phase = Phase::Stopping;
        }
        assert!(!lifecycle.announce_ready(1, "turbo", 0));
        lifecycle.announce_stopped();
        lifecycle.announce_stopped();
        lifecycle.announce_error(&Error::internal("late"));
        let events: Vec<_> = out
            .records()
            .iter()
            .map(|r| r["event"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(events, ["stopped"]);
    }

    #[test]
    fn error_records_are_bounded_and_single_line() {
        let out = Shared::default();
        let lifecycle = Lifecycle::new(&"i".repeat(64), Box::new(out.clone()));
        let message = format!("line one\nline two \"{}\"", "\\é".repeat(4096));
        lifecycle.announce_error(&Error::new(ErrorCode::ModelLoadFailed, message));
        let records = out.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["event"], "error");
        assert_eq!(records[0]["code"], "model_load_failed");
        assert!(!records[0]["message"].as_str().unwrap().contains('\n'));

        let mut buf = Vec::new();
        write_startup_error(&mut buf, ErrorCode::InvalidArguments, INVALID_INSTANCE_ID);
        let value: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(value["code"], "invalid_arguments");
        assert!(value.get("instance_id").is_none());
    }
}
