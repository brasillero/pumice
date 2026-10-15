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
}

/// Emits events on a writer thread; sending never blocks the run.
pub struct EventWriter {
    sender: mpsc::SyncSender<String>,
    drained: Arc<AtomicBool>,
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
        let (sender, receiver) = mpsc::sync_channel::<String>(EVENT_QUEUE);
        let drained = Arc::new(AtomicBool::new(false));
        let thread_drained = Arc::clone(&drained);
        let spawned = std::thread::Builder::new()
            .name("pumice-run-events".to_owned())
            .spawn(move || {
                let mut stderr = std::io::stderr().lock();
                for line in receiver {
                    // A blocked or closed stderr must not stall or kill the
                    // run: write failures end the stream quietly.
                    if writeln!(stderr, "{line}").is_err() {
                        break;
                    }
                }
                let _ = stderr.flush();
                thread_drained.store(true, Ordering::Release);
            });
        // Events are best-effort: without a writer thread the queue's
        // receiver is gone, every send fails silently and `finish` has
        // nothing to wait for.
        if spawned.is_err() {
            drained.store(true, Ordering::Release);
        }
        EventWriter {
            sender,
            drained,
            operation_id,
            seq: 0,
        }
    }

    /// Queues one event; dropped silently when the queue is full.
    pub fn emit(
        &mut self,
        stage: Option<&'static str>,
        event: &'static str,
        elapsed_ms: Option<u64>,
        code: Option<&'static str>,
    ) {
        let Ok(line) = serde_json::to_string(&Event {
            schema_version: SCHEMA_VERSION,
            operation_id: &self.operation_id,
            seq: self.seq,
            module: MODULE,
            stage,
            event,
            elapsed_ms,
            code,
        }) else {
            return;
        };
        self.seq += 1;
        let _ = self.sender.try_send(line);
    }

    /// Closes the queue and gives the writer a short budget to flush.
    pub fn finish(self) {
        drop(self.sender);
        let deadline = std::time::Instant::now() + DRAIN_WAIT;
        while !self.drained.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
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
