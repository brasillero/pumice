//! Bounded, metadata-only stderr events for `pumice run --json`
//! (p12-pumice-run/D7): one JSON Line per lifecycle event, never any text.
//!
//! Events are best-effort: a full buffer drops them instead of blocking the
//! run, and stdout stays the authoritative result channel. Lines look like:
//!
//! ```json
//! {"schema_version":1,"operation_id":"…","seq":3,"module":"orchestrator","stage":"transcription","event":"stage_finished","elapsed_ms":490}
//! ```

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use serde::Serialize;

use crate::orchestrator::result::{MODULE, SCHEMA_VERSION};

/// Queued events before the writer drops new ones.
const EVENT_QUEUE: usize = 64;
/// How long `finish` waits for the writer thread to drain.
const DRAIN_WAIT: Duration = Duration::from_millis(500);

#[derive(Serialize)]
struct Event<'a> {
    schema_version: u32,
    operation_id: &'a str,
    seq: u64,
    module: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    stage: Option<&'a str>,
    event: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    elapsed_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'a str>,
    /// `pumice serve`'s transcription child generation; never set by
    /// `pumice run`.
    #[serde(skip_serializing_if = "Option::is_none")]
    generation: Option<u64>,
}

/// Where an operation's lifecycle events go. Only metadata crosses this
/// boundary: stage names, event names, elapsed milliseconds and error codes,
/// never any text. `Send` so a service can run operations on its own tasks.
pub trait EventSink: Send {
    fn emit(
        &mut self,
        stage: Option<&'static str>,
        event: &'static str,
        elapsed_ms: Option<u64>,
        code: Option<&'static str>,
    );
}

/// Emits events on a writer thread; sending never blocks the run.
pub struct EventWriter {
    lines: LineWriter,
    operation_id: String,
    seq: u64,
}

/// The lifecycle events.
pub mod event {
    pub const STARTED: &str = "started";
    pub const STAGE_STARTED: &str = "stage_started";
    pub const STAGE_FINISHED: &str = "stage_finished";
    pub const FINISHED: &str = "finished";
}

impl EventWriter {
    /// Starts the writer thread draining lines to stderr.
    pub fn start(operation_id: String) -> EventWriter {
        EventWriter {
            lines: LineWriter::start(Stream::Stderr, "pumice-run-events"),
            operation_id,
            seq: 0,
        }
    }

    /// Closes the queue and gives the writer a short budget to flush.
    pub fn finish(self) {
        self.lines.finish();
    }

    /// Queues one service event about a transcription child generation
    /// (`pumice serve`); dropped silently when the queue is full.
    pub fn emit_generation(
        &mut self,
        event: &'static str,
        generation: Option<u64>,
        elapsed_ms: Option<u64>,
        code: Option<&'static str>,
    ) {
        let stage = generation.map(|_| crate::orchestrator::result::stage::TRANSCRIPTION);
        self.queue(None, stage, event, elapsed_ms, code, generation);
    }

    /// Queues one event of another operation than the writer's own
    /// (`pumice serve`'s dictations share the service's writer); dropped
    /// silently when the queue is full.
    pub fn emit_for(
        &mut self,
        operation_id: &str,
        stage: Option<&'static str>,
        event: &'static str,
        elapsed_ms: Option<u64>,
        code: Option<&'static str>,
    ) {
        self.queue(Some(operation_id), stage, event, elapsed_ms, code, None);
    }

    fn queue(
        &mut self,
        operation_id: Option<&str>,
        stage: Option<&str>,
        event: &str,
        elapsed_ms: Option<u64>,
        code: Option<&str>,
        generation: Option<u64>,
    ) {
        let Ok(line) = serde_json::to_string(&Event {
            schema_version: SCHEMA_VERSION,
            operation_id: operation_id.unwrap_or(&self.operation_id),
            seq: self.seq,
            module: MODULE,
            stage,
            event,
            elapsed_ms,
            code,
            generation,
        }) else {
            return;
        };
        self.seq += 1;
        let _ = self.lines.send(line);
    }
}

/// Which standard stream a [`LineWriter`] writes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// A bounded queue of lines drained to one standard stream by its own
/// thread, so a blocked or closed consumer never stalls the caller: a full
/// queue drops the line, and a write failure ends the stream quietly.
/// `pumice run`'s events and `pumice serve`'s lifecycle records and stderr
/// lines all go through one.
pub struct LineWriter {
    sender: mpsc::SyncSender<String>,
    drained: Arc<AtomicBool>,
}

/// A cloneable handle that queues lines on a [`LineWriter`].
#[derive(Clone)]
pub struct LineSender(mpsc::SyncSender<String>);

impl LineSender {
    /// Queues one line (without its newline); false when it was dropped.
    pub fn send(&self, line: String) -> bool {
        self.0.try_send(line).is_ok()
    }
}

impl LineWriter {
    pub fn start(stream: Stream, thread_name: &str) -> LineWriter {
        let (sender, receiver) = mpsc::sync_channel::<String>(EVENT_QUEUE);
        let drained = Arc::new(AtomicBool::new(false));
        let thread_drained = Arc::clone(&drained);
        let spawned = std::thread::Builder::new()
            .name(thread_name.to_owned())
            .spawn(move || {
                match stream {
                    Stream::Stdout => write_lines(receiver, std::io::stdout().lock()),
                    Stream::Stderr => write_lines(receiver, std::io::stderr().lock()),
                }
                thread_drained.store(true, Ordering::Release);
            });
        // Lines are best-effort: without a writer thread the queue's
        // receiver is gone, every send fails silently and `finish` has
        // nothing to wait for.
        if spawned.is_err() {
            drained.store(true, Ordering::Release);
        }
        LineWriter { sender, drained }
    }

    pub fn sender(&self) -> LineSender {
        LineSender(self.sender.clone())
    }

    /// Queues one line (without its newline); false when it was dropped.
    pub fn send(&self, line: String) -> bool {
        self.sender.try_send(line).is_ok()
    }

    /// Closes the queue and gives the writer a short budget to flush. Lines
    /// queued through a [`LineSender`] still alive keep the thread running;
    /// the wait stays bounded either way.
    pub fn finish(self) {
        drop(self.sender);
        let deadline = std::time::Instant::now() + DRAIN_WAIT;
        while !self.drained.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn write_lines(receiver: mpsc::Receiver<String>, mut out: impl Write) {
    for line in receiver {
        // A blocked or closed stream must not stall or kill the caller:
        // write failures end the stream quietly.
        if writeln!(out, "{line}").is_err() || out.flush().is_err() {
            break;
        }
    }
    let _ = out.flush();
}

impl EventSink for EventWriter {
    /// Queues one event; dropped silently when the queue is full.
    fn emit(
        &mut self,
        stage: Option<&'static str>,
        event: &'static str,
        elapsed_ms: Option<u64>,
        code: Option<&'static str>,
    ) {
        self.queue(None, stage, event, elapsed_ms, code, None);
    }
}

/// An opaque operation id: random-looking, carrying no paths or text.
pub fn operation_id() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut hasher = DefaultHasher::new();
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0)
        .hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    COUNTER.fetch_add(1, Ordering::Relaxed).hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_ids_are_distinct_and_hex() {
        let a = operation_id();
        let b = operation_id();
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(a.len(), 16);
    }
}
