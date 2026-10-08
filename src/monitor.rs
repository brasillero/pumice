//! The live event stream of completion requests (S4.6).
//!
//! Events carry dictated text only when the API layer was told text is
//! allowed — which is when the debug log is enabled. This module has no
//! terminal code; it only defines the typed stream and the sink trait.

use std::sync::mpsc;
use std::time::Instant;

use crate::pipeline::{AttemptResult, OutcomeKind};
use crate::providers::diagnostic::Diagnostic;

/// Receives the live events of every completion request. Production sends
/// them to the terminal view; tests capture them. Must be quick and never
/// block: it runs on the request's task.
pub trait Monitor: Send + Sync {
    fn event(&self, event: Event);
}

/// Discards every event: the monitor when no live view runs.
pub struct NoMonitor;

impl Monitor for NoMonitor {
    fn event(&self, _event: Event) {}
}

/// Forwards events over a standard channel to the live view's thread. A
/// closed receiver only loses the event.
pub struct ChannelMonitor(mpsc::Sender<Event>);

impl ChannelMonitor {
    pub fn new(sender: mpsc::Sender<Event>) -> ChannelMonitor {
        ChannelMonitor(sender)
    }
}

impl Monitor for ChannelMonitor {
    fn event(&self, event: Event) {
        let _ = self.0.send(event);
    }
}

/// One step of one completion request.
#[derive(Clone)]
pub struct Event {
    /// The request number, the same `#N` as the log line and the
    /// `chatcmpl-pumice-N` response id.
    pub number: u64,
    /// When the step happened.
    pub at: Instant,
    pub kind: EventKind,
}

#[derive(Clone)]
pub enum EventKind {
    /// The request reached the completion handler.
    Arrived,
    /// The body was read and parsed. `model` is the `model` field exactly as
    /// the client sent it. `input` is the dictation as extracted (for
    /// `inspect`, the whole body), only when text is allowed.
    Parsed {
        model: Option<String>,
        input: Option<String>,
    },
    /// Every slot was taken; the request waits in line.
    Queued,
    /// The provider's CLI is starting.
    Started {
        provider: &'static str,
        model: String,
    },
    /// The provider run and output cleanup ended. The diagnostic's `detail`
    /// (CLI output, which can quote the dictation) is empty unless text is
    /// allowed; its other fields are safe.
    AttemptEnded {
        result: AttemptResult,
        diagnostic: Option<Diagnostic>,
    },
    /// The response was handed to the HTTP server. `outcome` is `None` for a
    /// request rejected before formatting. `detail` is the same text-free
    /// text the log line shows (outcome name, or the error message the client
    /// received). `reply` is the text sent back, only when text is allowed
    /// and the status is 200.
    Responded {
        status: u16,
        outcome: Option<OutcomeKind>,
        detail: String,
        reply: Option<String>,
    },
    /// The client went away before the answer was ready; any CLI run was
    /// cancelled.
    Dropped,
}
