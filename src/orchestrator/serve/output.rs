//! What `pumice serve` writes (p13.1-flow-seam/D7).
//!
//! - `--json`: stdout carries bounded lifecycle records, one JSON object per
//!   line — `listening` (the claimed port, before the child is ready),
//!   `ready`, then `stopped`, or one `error` — and stderr carries
//!   metadata-only events through the P12 event writer.
//! - Plain: short human lines on stderr, starting with the frontend URL.
//!
//! Every line goes through a bounded writer thread, so a stalled consumer
//! never blocks requests or the shutdown. Nothing here carries content:
//! only ports, generations, durations and fixed codes.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use crate::orchestrator::events::{self, EventWriter, LineSender, LineWriter, Stream};
use crate::orchestrator::result::{MODULE, SCHEMA_VERSION};

/// The writers `pumice serve` owns; [`Outputs::finish`] flushes them.
pub struct Outputs {
    records: Option<LineWriter>,
    plain: Option<LineWriter>,
    events: Option<Arc<Mutex<Option<EventWriter>>>>,
}

impl Outputs {
    pub fn start(json: bool) -> Outputs {
        if json {
            Outputs {
                records: Some(LineWriter::start(Stream::Stdout, "pumice-serve-records")),
                plain: None,
                events: Some(Arc::new(Mutex::new(Some(EventWriter::start(
                    events::operation_id(),
                ))))),
            }
        } else {
            Outputs {
                records: None,
                plain: Some(LineWriter::start(Stream::Stderr, "pumice-serve-log")),
                events: None,
            }
        }
    }

    pub fn reporter(&self) -> Reporter {
        Reporter {
            records: self.records.as_ref().map(LineWriter::sender),
            plain: self.plain.as_ref().map(LineWriter::sender),
            events: self.events.clone(),
        }
    }

    /// Flushes every writer within its bounded budget. Reporters still
    /// alive only shorten what gets written.
    pub fn finish(self) {
        if let Some(events) = self.events
            && let Some(writer) = events
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
        {
            writer.finish();
        }
        if let Some(records) = self.records {
            records.finish();
        }
        if let Some(plain) = self.plain {
            plain.finish();
        }
    }
}

/// A cheap, cloneable handle for every task that reports something.
#[derive(Clone, Default)]
pub struct Reporter {
    records: Option<LineSender>,
    plain: Option<LineSender>,
    events: Option<Arc<Mutex<Option<EventWriter>>>>,
}

/// Service event names on stderr (`--json`).
pub mod event {
    pub const LISTENING: &str = "listening";
    pub const READY: &str = "ready";
    pub const CHILD_STARTING: &str = "child_starting";
    pub const CHILD_READY: &str = "child_ready";
    pub const CHILD_FAILED: &str = "child_failed";
    pub const CHILD_RESTARTING: &str = "child_restarting";
    pub const CHILD_RECYCLED: &str = "child_recycled";
    pub const TRANSCRIPTION_UNAVAILABLE: &str = "transcription_unavailable";
    pub const STOPPING: &str = "stopping";
    pub const STOPPED: &str = "stopped";
}

impl Reporter {
    /// Reports nothing (in-process tests).
    pub fn silent() -> Reporter {
        Reporter::default()
    }

    fn record(&self, event: &str, fields: Value) {
        let Some(records) = &self.records else { return };
        let mut record = json!({
            "schema_version": SCHEMA_VERSION,
            "event": event,
            "module": MODULE,
        });
        if let (Some(record), Value::Object(fields)) = (record.as_object_mut(), fields) {
            record.extend(fields);
        }
        records.send(record.to_string());
    }

    fn plain(&self, line: String) {
        if let Some(plain) = &self.plain {
            plain.send(line);
        }
    }

    fn event(
        &self,
        event: &'static str,
        generation: Option<u64>,
        elapsed_ms: Option<u64>,
        code: Option<&'static str>,
    ) {
        if let Some(events) = &self.events
            && let Some(writer) = events
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_mut()
        {
            writer.emit_generation(event, generation, elapsed_ms, code);
        }
    }

    /// The frontend port is claimed; the child is starting.
    pub fn listening(&self, port: u16) {
        self.record(
            event::LISTENING,
            json!({"version": env!("CARGO_PKG_VERSION"), "host": "127.0.0.1", "port": port}),
        );
        self.event(event::LISTENING, None, None, None);
        self.plain(format!(
            "pumice serve: listening on http://127.0.0.1:{port}; starting the transcription model"
        ));
    }

    /// The first child is ready: the service is usable.
    pub fn ready(&self, port: u16) {
        self.record(
            event::READY,
            json!({"version": env!("CARGO_PKG_VERSION"), "host": "127.0.0.1", "port": port}),
        );
        self.event(event::READY, None, None, None);
        self.plain(format!("pumice serve: ready on http://127.0.0.1:{port}"));
    }

    pub fn stopping(&self) {
        self.event(event::STOPPING, None, None, None);
        self.plain("pumice serve: stopping".to_owned());
    }

    pub fn stopped(&self, exit_code: u8) {
        self.record(event::STOPPED, json!({"exit_code": exit_code}));
        self.event(event::STOPPED, None, None, None);
        self.plain("pumice serve: stopped".to_owned());
    }

    /// A fatal error: one `error` record (or line) and nothing else on
    /// stdout. `message` is fixed text, or a configuration error naming the
    /// user's own file and position.
    pub fn error(&self, code: &str, cause_code: Option<&str>, message: &str) {
        let mut fields = json!({"code": code, "message": message});
        if let Some(cause) = cause_code {
            fields["cause_code"] = json!(cause);
        }
        self.record("error", fields);
        match cause_code {
            Some(cause) => self.plain(format!("error: {code} ({cause}): {message}")),
            None => self.plain(format!("error: {code}: {message}")),
        }
    }

    pub fn child_starting(&self, generation: u64) {
        self.event(event::CHILD_STARTING, Some(generation), None, None);
    }

    pub fn child_ready(&self, generation: u64, model_load_ms: u64) {
        self.event(
            event::CHILD_READY,
            Some(generation),
            Some(model_load_ms),
            None,
        );
        if generation > 1 {
            self.plain(format!(
                "pumice serve: transcription ready again (generation {generation})"
            ));
        }
    }

    pub fn child_failed(
        &self,
        generation: u64,
        code: &'static str,
        child_code: Option<&'static str>,
    ) {
        self.event(event::CHILD_FAILED, Some(generation), None, Some(code));
        match child_code {
            Some(child) => self.plain(format!(
                "pumice serve: transcription generation {generation} failed: {code} ({child})"
            )),
            None => self.plain(format!(
                "pumice serve: transcription generation {generation} failed: {code}"
            )),
        }
    }

    pub fn child_recycled(&self, generation: u64) {
        self.event(event::CHILD_RECYCLED, Some(generation), None, None);
        self.plain(format!(
            "pumice serve: recycling transcription generation {generation}"
        ));
    }

    pub fn child_restarting(&self, attempt: u32, of: u32, delay: Duration) {
        let delay_ms = delay.as_millis() as u64;
        self.event(event::CHILD_RESTARTING, None, Some(delay_ms), None);
        self.plain(format!(
            "pumice serve: restarting transcription in {delay_ms} ms (attempt {attempt} of {of})"
        ));
    }

    pub fn transcription_unavailable(&self, code: &'static str) {
        self.event(event::TRANSCRIPTION_UNAVAILABLE, None, None, Some(code));
        self.plain(format!(
            "pumice serve: transcription unavailable ({code}); restart pumice serve after fixing the cause"
        ));
    }
}
