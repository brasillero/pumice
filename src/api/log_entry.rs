//! The human-readable request log: one entry per completion request on the
//! ordinary log sink, never dictated text.
//!
//! A request that gets no completion (an HTTP error, or a client that
//! disconnects) still gets one line, labelled `REJECTED` or `DROPPED`.
//!
//! An entry is a header line with the outcome, the provider and model that
//! produced the text, the total time and the text length. When the provider
//! ran but failed, one indented attempt line follows with the safe reason:
//!
//! A request that could not be formatted is answered with an HTTP error
//! (the app keeps its own transcript) and labelled `FAILED`:
//!
//! ```text
//! 2026-10-06 14:04:01  #13  FAILED       HTTP 504: timed out  6.2s  0 chars  [requested: claude]
//!                           ✗ claude (haiku)        timed out          6.0s
//! ```

use std::fmt::Write as _;

use axum::http::StatusCode;
use std::time::Duration;

use crate::pipeline::{Attempt, AttemptResult, FormatOutcome, OutcomeKind, RawReason};
use crate::providers::ProviderError;
use crate::providers::diagnostic::Diagnostic;

/// Longest requested model name shown; a client controls it.
const MAX_REQUESTED_CHARS: usize = 40;

/// Width of the `timestamp  #id  ` prefix, so attempt lines line up under
/// the outcome.
const INDENT: usize = 26;

/// What the log entry for one request needs.
pub(super) struct Entry<'a> {
    pub number: u64,
    /// The `model` field the client sent, if any.
    pub requested: Option<&'a str>,
    pub outcome: &'a FormatOutcome,
}

/// Renders `entry`; `color` adds ANSI colors for a terminal.
pub(super) fn render(entry: &Entry<'_>, color: bool) -> String {
    render_at(entry, color, &local_timestamp())
}

fn render_at(entry: &Entry<'_>, color: bool, timestamp: &str) -> String {
    let paint = Paint(color);
    let outcome = entry.outcome;
    let failed = outcome
        .trail
        .iter()
        .any(|attempt| attempt.result != AttemptResult::Formatted);

    let (label, style, what) = match outcome.kind {
        OutcomeKind::Formatted => {
            let producer = outcome.trail.last().map_or_else(
                || outcome.provider.unwrap_or("-").to_owned(),
                provider_and_model,
            );
            ("formatted", Style::Green, producer)
        }
        OutcomeKind::Passthrough => (
            "passthrough",
            Style::Cyan,
            "original text returned as requested".to_owned(),
        ),
        OutcomeKind::Inspect => ("inspect", Style::Cyan, "request body echoed".to_owned()),
        OutcomeKind::Empty => ("empty", Style::Dim, "nothing to format".to_owned()),
        OutcomeKind::Raw(reason) => (
            "FAILED",
            Style::Red,
            format!(
                "HTTP {}: {}",
                failure_status(reason).as_u16(),
                raw_reason(reason, entry.requested)
            ),
        ),
    };

    let notes = format!("requested: {}", requested(entry.requested));

    let mut out = String::new();
    let _ = write!(
        out,
        "{}  {}  {}  {}  {}  {} chars  {}",
        paint.apply(Style::Dim, timestamp),
        paint.apply(Style::Dim, &format!("#{:<3}", entry.number)),
        paint.apply(style, &format!("{label:<11}")),
        what,
        seconds(outcome.elapsed),
        // A failure sends no text: the app keeps its own transcript.
        if matches!(outcome.kind, OutcomeKind::Raw(_)) {
            0
        } else {
            outcome.text.chars().count()
        },
        paint.apply(Style::Dim, &format!("[{notes}]")),
    );

    if failed {
        let name_width = outcome
            .trail
            .iter()
            .map(|attempt| provider_and_model(attempt).chars().count())
            .max()
            .unwrap_or(0);
        let rows: Vec<(&str, Style, String, &Attempt)> = outcome
            .trail
            .iter()
            .map(|attempt| {
                let (mark, style, result) = match attempt.result {
                    AttemptResult::Formatted => ("✓", Style::Green, "formatted".to_owned()),
                    AttemptResult::Failed(error) => {
                        let mut label = provider_error(error);
                        if let Some(summary) =
                            attempt.diagnostic.as_ref().and_then(Diagnostic::summary)
                        {
                            label.push_str(&format!(" ({summary})"));
                        }
                        ("✗", Style::Red, label)
                    }
                    AttemptResult::CleanupRejected(error) => ("✗", Style::Red, error.to_string()),
                };
                (mark, style, result, attempt)
            })
            .collect();
        let result_width = rows
            .iter()
            .map(|(_, _, result, _)| result.chars().count())
            .max()
            .unwrap_or(0);
        for (mark, style, result, attempt) in rows {
            let _ = write!(
                out,
                "\n{:INDENT$}{} {:<name_width$}  {:<result_width$}  {}",
                "",
                paint.apply(style, mark),
                provider_and_model(attempt),
                result,
                seconds(attempt.elapsed),
            );
        }
    }
    out
}

fn provider_and_model(attempt: &Attempt) -> String {
    if attempt.model.is_empty() {
        attempt.provider.to_owned()
    } else {
        format!("{} ({})", attempt.provider, attempt.model)
    }
}

/// The client's `model` field, shortened and with control characters
/// escaped; a missing or blank value is shown as `(none)`.
fn requested(requested: Option<&str>) -> String {
    match requested.map(str::trim) {
        None | Some("") => "(none)".to_owned(),
        Some(model) => {
            let short: String = model.chars().take(MAX_REQUESTED_CHARS).collect();
            let escaped: String = short.escape_debug().collect();
            if model.chars().count() > MAX_REQUESTED_CHARS {
                format!("{escaped}…")
            } else {
                escaped
            }
        }
    }
}

/// The HTTP status a request that could not be formatted is answered with.
/// The app then pastes its own transcript (Handy and OpenWhispr both do).
pub(super) fn failure_status(reason: RawReason) -> StatusCode {
    match reason {
        RawReason::NoModel => StatusCode::BAD_REQUEST,
        RawReason::UnknownProvider | RawReason::ProviderDisabled => StatusCode::NOT_FOUND,
        RawReason::Busy => StatusCode::SERVICE_UNAVAILABLE,
        RawReason::BudgetExhausted | RawReason::ProviderFailed(ProviderError::Timeout) => {
            StatusCode::GATEWAY_TIMEOUT
        }
        RawReason::ProviderFailed(
            ProviderError::RateLimited { .. } | ProviderError::QuotaExceeded { .. },
        ) => StatusCode::TOO_MANY_REQUESTS,
        RawReason::ProviderFailed(_) | RawReason::CleanupFailed(_) => StatusCode::BAD_GATEWAY,
    }
}

/// A safe, text-free description of why a request could not be formatted:
/// written to the log and sent as the HTTP error message.
pub(super) fn raw_reason(reason: RawReason, requested_model: Option<&str>) -> String {
    match reason {
        RawReason::UnknownProvider => format!(
            "no provider named \"{}\" in the config",
            requested(requested_model)
        ),
        RawReason::ProviderDisabled => format!(
            "provider \"{}\" is disabled in the config",
            requested(requested_model)
        ),
        RawReason::NoModel => "the request named no provider".to_owned(),
        RawReason::Busy => "another dictation was still being formatted".to_owned(),
        RawReason::BudgetExhausted => "the total time budget ran out".to_owned(),
        RawReason::ProviderFailed(error) => provider_error(error),
        RawReason::CleanupFailed(error) => error.to_string(),
    }
}

/// A short label for a provider failure, with the retry hint when known.
fn provider_error(error: ProviderError) -> String {
    let with_retry = |label: &str, retry_after: Option<Duration>| match retry_after {
        Some(after) => format!("{label} (retry in {}s)", after.as_secs()),
        None => label.to_owned(),
    };
    match error {
        ProviderError::NotInstalled => "not installed".to_owned(),
        ProviderError::NotLoggedIn => "not logged in".to_owned(),
        ProviderError::Timeout => "timed out".to_owned(),
        ProviderError::QuotaExceeded { retry_after } => with_retry("quota exhausted", retry_after),
        ProviderError::RateLimited { retry_after } => with_retry("rate limited", retry_after),
        ProviderError::Other { code } => code.to_string(),
    }
}

/// How a completion request ended without a completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Unanswered<'a> {
    /// Answered with an HTTP error; the message is the fixed, text-free one
    /// the client received.
    Rejected { status: u16, message: &'a str },
    /// The client went away before the answer was ready; any CLI run was
    /// cancelled.
    Disconnected,
}

/// Renders the entry for a request that got no completion, so every request
/// still leaves exactly one entry.
pub(super) fn render_unanswered(
    number: u64,
    unanswered: Unanswered<'_>,
    elapsed: Duration,
    color: bool,
) -> String {
    render_unanswered_at(number, unanswered, elapsed, color, &local_timestamp())
}

fn render_unanswered_at(
    number: u64,
    unanswered: Unanswered<'_>,
    elapsed: Duration,
    color: bool,
    timestamp: &str,
) -> String {
    let paint = Paint(color);
    let (label, what) = match unanswered {
        Unanswered::Rejected { status, message } => {
            ("REJECTED", format!("HTTP {status}: {message}"))
        }
        Unanswered::Disconnected => (
            "DROPPED",
            "the client disconnected before the answer was ready".to_owned(),
        ),
    };
    format!(
        "{}  {}  {}  {}  {}",
        paint.apply(Style::Dim, timestamp),
        paint.apply(Style::Dim, &format!("#{:<3}", number)),
        paint.apply(Style::Red, &format!("{label:<11}")),
        what,
        seconds(elapsed),
    )
}

fn seconds(elapsed: Duration) -> String {
    format!("{:.1}s", elapsed.as_secs_f64())
}

fn local_timestamp() -> String {
    jiff::Zoned::now().strftime("%Y-%m-%d %H:%M:%S").to_string()
}

#[derive(Clone, Copy)]
enum Style {
    Dim,
    Green,
    Red,
    Cyan,
}

struct Paint(bool);

impl Paint {
    fn apply(&self, style: Style, text: &str) -> String {
        if !self.0 {
            return text.to_owned();
        }
        let code = match style {
            Style::Dim => "2",
            Style::Green => "32",
            Style::Red => "31",
            Style::Cyan => "36",
        };
        format!("\x1b[{code}m{text}\x1b[0m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ProviderError;

    fn attempt(provider: &'static str, model: &str, result: AttemptResult, ms: u64) -> Attempt {
        Attempt {
            provider,
            model: model.to_owned(),
            result,
            elapsed: Duration::from_millis(ms),
            diagnostic: None,
        }
    }

    fn outcome(
        kind: OutcomeKind,
        provider: Option<&'static str>,
        trail: Vec<Attempt>,
    ) -> FormatOutcome {
        FormatOutcome {
            text: "olá mundo".to_owned(),
            kind,
            provider,
            attempts: trail.len() as u8,
            trail,
            elapsed: Duration::from_millis(9_100),
        }
    }

    #[test]
    fn cleanup_failure_names_its_cause() {
        use crate::cleanup::CleanupError;
        assert_eq!(
            raw_reason(RawReason::CleanupFailed(CleanupError::Empty), None),
            "the provider returned an empty reply"
        );
        assert_eq!(
            raw_reason(
                RawReason::CleanupFailed(CleanupError::UnclosedReasoning),
                None
            ),
            "the reply opened a reasoning block it never closed"
        );
    }

    #[test]
    fn rejected_and_dropped_requests_get_one_line() {
        let rejected = render_unanswered_at(
            7,
            Unanswered::Rejected {
                status: 400,
                message: "the request body is not valid JSON",
            },
            Duration::from_millis(3),
            false,
            "2026-10-06 14:04:01",
        );
        assert_eq!(
            rejected,
            "2026-10-06 14:04:01  #7    REJECTED     HTTP 400: the request body is not valid JSON  0.0s"
        );
        let dropped = render_unanswered_at(
            8,
            Unanswered::Disconnected,
            Duration::from_millis(2_100),
            false,
            "2026-10-06 14:04:01",
        );
        assert!(dropped.contains("#8 "), "{dropped}");
        assert!(dropped.contains("DROPPED"), "{dropped}");
        assert!(dropped.ends_with("2.1s"), "{dropped}");
        assert!(!dropped.contains('\n'));
    }

    fn plain(entry: &Entry<'_>) -> String {
        render_at(entry, false, "2026-10-06 14:04:01")
    }

    #[test]
    fn formatted_shows_provider_model_and_no_attempt_lines() {
        let outcome = outcome(
            OutcomeKind::Formatted,
            Some("claude"),
            vec![attempt("claude", "haiku", AttemptResult::Formatted, 2_800)],
        );
        let text = plain(&Entry {
            number: 12,
            requested: None,
            outcome: &outcome,
        });
        assert_eq!(
            text,
            "2026-10-06 14:04:01  #12   formatted    claude (haiku)  9.1s  9 chars  [requested: (none)]"
        );
    }

    #[test]
    fn failure_lists_the_single_attempt_with_its_reason() {
        let outcome = outcome(
            OutcomeKind::Raw(RawReason::ProviderFailed(ProviderError::Timeout)),
            None,
            vec![attempt(
                "claude",
                "haiku",
                AttemptResult::Failed(ProviderError::Timeout),
                6_000,
            )],
        );
        let text = plain(&Entry {
            number: 13,
            requested: Some("claude"),
            outcome: &outcome,
        });
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert!(lines[0].contains("FAILED       HTTP 504: timed out"), "{text}");
        assert!(lines[0].contains("[requested: claude]"), "{text}");
        assert!(!lines[0].contains("fallback"), "{text}");
        assert!(
            lines[1].contains("✗ claude (haiku)") && lines[1].contains("timed out"),
            "{text}"
        );
    }

    #[test]
    fn failure_names_the_status_and_the_provider_error() {
        let outcome = outcome(
            OutcomeKind::Raw(RawReason::ProviderFailed(ProviderError::RateLimited {
                retry_after: Some(Duration::from_secs(60)),
            })),
            None,
            vec![attempt(
                "claude",
                "haiku",
                AttemptResult::Failed(ProviderError::RateLimited {
                    retry_after: Some(Duration::from_secs(60)),
                }),
                2_100,
            )],
        );
        let text = plain(&Entry {
            number: 14,
            requested: None,
            outcome: &outcome,
        });
        assert!(
            text.contains("FAILED       HTTP 429: rate limited (retry in 60s)"),
            "{text}"
        );
        assert!(text.contains("rate limited (retry in 60s)"), "{text}");
    }

    #[test]
    fn failure_shows_the_safe_diagnostic_summary_only() {
        let mut failed = attempt(
            "claude",
            "haiku",
            AttemptResult::Failed(ProviderError::other(
                crate::providers::ProviderErrorCode::NonzeroExit,
            )),
            2_100,
        );
        failed.diagnostic = Some(Diagnostic {
            exit_code: Some(1),
            api_status: Some(400),
            subtype: None,
            detail: "stderr: secret dictation".to_owned(),
        });
        let outcome = outcome(
            OutcomeKind::Raw(RawReason::ProviderFailed(ProviderError::other(
                crate::providers::ProviderErrorCode::NonzeroExit,
            ))),
            None,
            vec![failed],
        );
        let text = plain(&Entry {
            number: 1,
            requested: None,
            outcome: &outcome,
        });
        assert!(text.contains("(exit 1, API status 400)"), "{text}");
        assert!(!text.contains("secret"), "{text}");
    }

    #[test]
    fn unknown_model_is_named_and_escaped() {
        let outcome = outcome(
            OutcomeKind::Raw(RawReason::UnknownProvider),
            None,
            Vec::new(),
        );
        let text = plain(&Entry {
            number: 1,
            requested: Some("gpt\n4"),
            outcome: &outcome,
        });
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(
            text.contains("no provider named \"gpt\\n4\" in the config"),
            "{text}"
        );
    }

    #[test]
    fn color_wraps_only_when_enabled() {
        let outcome = outcome(OutcomeKind::Passthrough, None, Vec::new());
        let entry = Entry {
            number: 2,
            requested: Some("passthrough"),
            outcome: &outcome,
        };
        assert!(!plain(&entry).contains('\x1b'));
        assert!(render_at(&entry, true, "t").contains("\x1b[36m"));
    }
}
