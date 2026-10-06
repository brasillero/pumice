//! The human-readable request log: one entry per completion request on the
//! ordinary log sink, never dictated text.
//!
//! An entry is a header line with the outcome, the provider and model that
//! produced the text, the total time and the text length. When a provider
//! failed along the way, one indented line per attempt follows, so a fallback
//! shows where it came from and why:
//!
//! ```text
//! 2026-10-06 14:04:01  #13  formatted  codex (gpt-6.1-sol)  9.1s  98 chars  [requested: claude, fallback]
//!                           ✗ claude (haiku)        timed out          6.0s
//!                           ✓ codex (gpt-6.1-sol)   formatted          3.1s
//! ```

use std::fmt::Write as _;
use std::time::Duration;

use crate::pipeline::{Attempt, AttemptResult, FormatOutcome, OutcomeKind, RawReason};
use crate::providers::ProviderError;

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
    let fell_back = outcome
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
            "RAW",
            Style::Yellow,
            format!(
                "original text returned: {}",
                raw_reason(reason, entry.requested, outcome.trail.len())
            ),
        ),
    };

    let mut notes = vec![format!("requested: {}", requested(entry.requested))];
    if fell_back && outcome.kind == OutcomeKind::Formatted {
        notes.push("fallback".to_owned());
    }

    let mut out = String::new();
    let _ = write!(
        out,
        "{}  {}  {}  {}  {}  {} chars  {}",
        paint.apply(Style::Dim, timestamp),
        paint.apply(Style::Dim, &format!("#{:<3}", entry.number)),
        paint.apply(style, &format!("{label:<11}")),
        what,
        seconds(outcome.elapsed),
        outcome.text.chars().count(),
        paint.apply(Style::Dim, &format!("[{}]", notes.join(", "))),
    );

    if fell_back {
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
                    AttemptResult::Failed(error) => ("✗", Style::Red, provider_error(error)),
                    AttemptResult::CleanupRejected => {
                        ("✗", Style::Red, "output rejected by cleanup".to_owned())
                    }
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
/// escaped; a missing or blank value means the default provider.
fn requested(requested: Option<&str>) -> String {
    match requested.map(str::trim) {
        None | Some("") => "default".to_owned(),
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

fn raw_reason(reason: RawReason, requested_model: Option<&str>, attempts: usize) -> String {
    match reason {
        RawReason::UnknownProvider => format!(
            "no provider named \"{}\" in the config",
            requested(requested_model)
        ),
        RawReason::ProviderDisabled => format!(
            "provider \"{}\" is disabled in the config",
            requested(requested_model)
        ),
        RawReason::Busy => "another dictation was still being formatted".to_owned(),
        RawReason::BudgetExhausted => "the total time budget ran out".to_owned(),
        RawReason::ProviderFailed(error) if attempts <= 1 => provider_error(error),
        RawReason::ProviderFailed(_) => "every provider failed".to_owned(),
        RawReason::CleanupFailed if attempts <= 1 => "output rejected by cleanup".to_owned(),
        RawReason::CleanupFailed => "every provider failed".to_owned(),
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
    Yellow,
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
            Style::Yellow => "33",
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
            "2026-10-06 14:04:01  #12   formatted    claude (haiku)  9.1s  9 chars  [requested: default]"
        );
    }

    #[test]
    fn fallback_lists_every_attempt_with_its_reason() {
        let outcome = outcome(
            OutcomeKind::Formatted,
            Some("codex"),
            vec![
                attempt(
                    "claude",
                    "haiku",
                    AttemptResult::Failed(ProviderError::Timeout),
                    6_000,
                ),
                attempt("codex", "gpt-6.1-sol", AttemptResult::Formatted, 3_100),
            ],
        );
        let text = plain(&Entry {
            number: 13,
            requested: Some("claude"),
            outcome: &outcome,
        });
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert!(
            lines[0].contains("formatted    codex (gpt-6.1-sol)"),
            "{text}"
        );
        assert!(lines[0].contains("[requested: claude, fallback]"), "{text}");
        assert!(
            lines[1].contains("✗ claude (haiku)") && lines[1].contains("timed out"),
            "{text}"
        );
        assert!(
            lines[2].contains("✓ codex (gpt-6.1-sol)") && lines[2].contains("3.1s"),
            "{text}"
        );
    }

    #[test]
    fn raw_after_chain_says_every_provider_failed() {
        let outcome = outcome(
            OutcomeKind::Raw(RawReason::ProviderFailed(ProviderError::RateLimited {
                retry_after: Some(Duration::from_secs(60)),
            })),
            None,
            vec![
                attempt(
                    "claude",
                    "haiku",
                    AttemptResult::Failed(ProviderError::NotLoggedIn),
                    900,
                ),
                attempt(
                    "codex",
                    "gpt-6.1-sol",
                    AttemptResult::Failed(ProviderError::RateLimited {
                        retry_after: Some(Duration::from_secs(60)),
                    }),
                    2_100,
                ),
            ],
        );
        let text = plain(&Entry {
            number: 14,
            requested: None,
            outcome: &outcome,
        });
        assert!(
            text.contains("RAW          original text returned: every provider failed"),
            "{text}"
        );
        assert!(text.contains("not logged in"), "{text}");
        assert!(text.contains("rate limited (retry in 60s)"), "{text}");
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
