//! The live event stream of completion requests (S4.6).
//!
//! Events carry dictated text only when the API layer was told text is
//! allowed — which is when the debug log is enabled. This module has no
//! terminal code; it only defines the typed stream and the sink trait.

use std::sync::mpsc;
use std::time::Instant;

use crate::pipeline::{AttemptResult, OutcomeKind};
use crate::providers::diagnostic::Diagnostic;

/// Maximum bytes kept for any single text field in a live event. Longer
/// strings are cut on a character boundary and marked with their original
/// size, bounding the view's memory when many large bodies arrive.
pub const MAX_TEXT: usize = 64 * 1024;

/// Keeps at most [`MAX_TEXT`] bytes of `text`, cutting on a character
/// boundary. When the string is cut, a marker line with the original byte
/// count is appended.
pub fn capped(text: String) -> String {
    if text.len() <= MAX_TEXT {
        return text;
    }
    let mut len = 0;
    for c in text.chars() {
        let next = len + c.len_utf8();
        if next > MAX_TEXT {
            break;
        }
        len = next;
    }
    format!("{}\n… [cut: {} bytes in all]", &text[..len], text.len())
}

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
    /// The request reached the completion handler. `client` is the app name the
    /// request states itself in its `X-Title` header (Handy sends `X-Title:
    /// Handy`), trimmed and cut to 32 characters; `None` when absent, empty or
    /// not printable ASCII. It is not dictated text, so it is sent whether or
    /// not text is allowed.
    Arrived { client: Option<String> },
    /// The request as received, only when text is allowed: every header
    /// (credential values masked; grouped by name, not necessarily in wire
    /// order) and the body as text (lossy UTF-8).
    /// Emitted right after the body was read, before it is parsed, so rejected
    /// bodies are visible too.
    Received {
        headers: Vec<(String, String)>,
        body: String,
    },
    /// The body was read and parsed. `model` is the `model` field as the
    /// client sent it. `text` is the extracted request (for `inspect`, the
    /// whole body as the input), only when text is allowed.
    Parsed {
        model: Option<String>,
        text: Option<ParsedText>,
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
    /// The exact HTTP response handed to the server, only when text is allowed:
    /// status, every response header and the body (lossy UTF-8). Emitted after
    /// `Responded`, from the same request.
    Sent {
        status: u16,
        headers: Vec<(String, String)>,
        body: String,
    },
    /// The client went away before the answer was ready; any CLI run was
    /// cancelled.
    Dropped,
}

/// What Pumice extracted from a dictation request.
// No `Debug`: it holds dictated text, which must never reach logs through `{:?}`.
#[derive(Clone)]
pub struct ParsedText {
    /// The `system`/`developer` messages, in order.
    pub system: Vec<String>,
    /// The user message up to the input (Handy: through the opening tag).
    pub before: String,
    /// The input itself.
    pub input: String,
    /// The user message after the input.
    pub after: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capped_cuts_on_char_boundary() {
        let text = "a".repeat(MAX_TEXT - 1) + "é";
        let result = capped(text.clone());
        assert!(result.contains(" bytes in all]"));
        let marker = "\n… [cut:";
        let marker_pos = result.find(marker).expect("cut marker");
        let prefix = &result[..marker_pos];
        // The cut landed before the multi-byte character, so the kept prefix
        // is shorter than the original and ends on a character boundary.
        assert!(prefix.len() < text.len());
        assert!(text.is_char_boundary(prefix.len()));
        assert!(prefix.chars().all(|c| c == 'a'));
    }

    #[test]
    fn capped_keeps_short_text_unchanged() {
        let text = "é".repeat(MAX_TEXT / 2);
        assert_eq!(capped(text.clone()), text);
    }
}
