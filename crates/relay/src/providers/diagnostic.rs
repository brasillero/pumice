//! Why a provider call failed, captured beside the safe [`ProviderError`].
//!
//! [`ProviderError`](super::ProviderError) carries no text by design. This
//! module records what the CLI actually said, in two parts:
//!
//! - safe fields ([`Diagnostic::summary`]) for the ordinary request log: the
//!   exit code, and from a JSON error envelope the HTTP status
//!   (`api_error_status`) and a short `subtype` token. These hold numbers and
//!   identifiers only, never free text;
//! - an excerpt of stderr and stdout ([`Diagnostic::detail`]) that may echo
//!   dictated text, so it goes to the opt-in debug log only.
//!
//! The pipeline opens a slot per attempt with [`capture`]; the CLI provider
//! fills it through [`record`] when a call fails. A task-local keeps the
//! provider interface unchanged.

use std::cell::RefCell;
use std::fmt;
use std::future::Future;

use serde_json::Value;

use crate::process::ProcessOutput;

/// Longest stderr and stdout excerpts kept, in bytes.
const MAX_EXCERPT: usize = 2_000;

/// Longest `subtype` token shown in the ordinary log.
const MAX_SUBTYPE: usize = 40;

tokio::task_local! {
    static SLOT: RefCell<Option<Diagnostic>>;
}

/// What one failed provider call left behind.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Diagnostic {
    pub exit_code: Option<i32>,
    /// HTTP status the CLI reported for its API call, when it says so.
    pub api_status: Option<u16>,
    /// The error envelope's `subtype`, when it is a short identifier.
    pub subtype: Option<String>,
    /// stderr tail and stdout excerpt. May contain dictated text: debug log
    /// only.
    pub detail: String,
}

// Manual impl: `detail` may contain dictated text.
impl fmt::Debug for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Diagnostic")
            .field("exit_code", &self.exit_code)
            .field("api_status", &self.api_status)
            .field("subtype", &self.subtype)
            .field("detail_len", &self.detail.len())
            .finish()
    }
}

impl Diagnostic {
    /// Builds a diagnostic from a finished call's output.
    pub fn from_output(output: &ProcessOutput) -> Diagnostic {
        let envelope = serde_json::from_slice::<Value>(&output.stdout)
            .ok()
            .filter(Value::is_object);
        let api_status = envelope
            .as_ref()
            .and_then(|envelope| envelope.get("api_error_status"))
            .and_then(Value::as_u64)
            .and_then(|status| u16::try_from(status).ok());
        let subtype = envelope
            .as_ref()
            .and_then(|envelope| envelope.get("subtype"))
            .and_then(Value::as_str)
            .filter(|token| is_identifier(token))
            .map(str::to_owned);

        let mut detail = String::new();
        let stderr = String::from_utf8_lossy(&output.stderr_tail);
        let stderr = stderr.trim();
        if !stderr.is_empty() {
            detail.push_str("stderr: ");
            detail.push_str(tail(stderr, MAX_EXCERPT));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stdout = stdout.trim();
        if !stdout.is_empty() {
            if !detail.is_empty() {
                detail.push('\n');
            }
            detail.push_str("stdout: ");
            detail.push_str(head(stdout, MAX_EXCERPT));
        }

        Diagnostic {
            exit_code: output.status.code(),
            api_status,
            subtype,
            detail,
        }
    }

    /// The safe fields as a short phrase, e.g. `exit 1, API status 400,
    /// error_during_execution`; `None` when there is nothing to show.
    pub fn summary(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(code) = self.exit_code {
            parts.push(format!("exit {code}"));
        }
        if let Some(status) = self.api_status {
            parts.push(format!("API status {status}"));
        }
        if let Some(subtype) = &self.subtype {
            parts.push(subtype.clone());
        }
        (!parts.is_empty()).then(|| parts.join(", "))
    }
}

/// Runs `future` with an empty diagnostic slot and returns what a provider
/// recorded in it.
pub async fn capture<F: Future>(future: F) -> (F::Output, Option<Diagnostic>) {
    SLOT.scope(RefCell::new(None), async move {
        let output = future.await;
        let diagnostic = SLOT.with(|slot| slot.borrow_mut().take());
        (output, diagnostic)
    })
    .await
}

/// Records why the current call failed. Does nothing outside [`capture`].
pub fn record(diagnostic: Diagnostic) {
    let _ = SLOT.try_with(|slot| *slot.borrow_mut() = Some(diagnostic));
}

fn is_identifier(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= MAX_SUBTYPE
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn head(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn tail(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn output(code: i32, stdout: &str, stderr: &str) -> ProcessOutput {
        use std::os::unix::process::ExitStatusExt;
        ProcessOutput {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr_tail: stderr.as_bytes().to_vec(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn summary_keeps_only_safe_fields() {
        let diagnostic = Diagnostic::from_output(&output(
            1,
            r#"{"type":"result","is_error":true,"subtype":"error_during_execution","api_error_status":400,"result":"secret dictation"}"#,
            "boom",
        ));
        assert_eq!(
            diagnostic.summary().as_deref(),
            Some("exit 1, API status 400, error_during_execution")
        );
        assert!(diagnostic.detail.contains("stderr: boom"));
        assert!(diagnostic.detail.contains("secret dictation"));
        assert!(!format!("{diagnostic:?}").contains("secret"));
    }

    #[cfg(unix)]
    #[test]
    fn free_text_subtype_is_dropped() {
        let diagnostic = Diagnostic::from_output(&output(
            2,
            r#"{"subtype":"a sentence with the dictation"}"#,
            "",
        ));
        assert_eq!(diagnostic.summary().as_deref(), Some("exit 2"));
    }

    #[test]
    fn excerpts_respect_char_boundaries() {
        let text = "é".repeat(MAX_EXCERPT);
        assert!(head(&text, MAX_EXCERPT).len() <= MAX_EXCERPT);
        assert!(tail(&text, MAX_EXCERPT).len() <= MAX_EXCERPT);
    }

    #[tokio::test]
    async fn capture_returns_what_was_recorded() {
        let ((), diagnostic) = capture(async {
            record(Diagnostic {
                exit_code: Some(3),
                ..Diagnostic::default()
            });
        })
        .await;
        assert_eq!(diagnostic.and_then(|d| d.exit_code), Some(3));
        record(Diagnostic::default()); // outside a capture: ignored
    }
}
