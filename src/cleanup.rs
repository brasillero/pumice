//! Minimal cleanup of a provider's final text.
//!
//! Handy pastes Pumice's response verbatim, so Pumice trusts what the model
//! returns and changes as little as possible (owner decision 2026-10-07):
//! formatting instructions belong in the prompt, not in post-processing.
//! Only three rules remain:
//!
//! 1. Reasoning blocks (`<think>…</think>`) at the very start are removed;
//!    they are never dictated text. An opened block that never closes is
//!    rejected.
//! 2. Whitespace around the text is trimmed.
//! 3. An empty result for a nonempty dictation is rejected.
//!
//! A rejection makes the caller return the original text, and the log names
//! the reason.

use std::fmt;

/// Reasoning-tag wrappers removed from the start of the output, as
/// (opening, closing) pairs. Tag names are matched case-insensitively.
const REASONING_OPENERS: &[(&str, &str)] =
    &[("<think>", "</think>"), ("<thinking>", "</thinking>")];

/// Why cleanup refused to return text.
///
/// It carries no dictated text, so both `Display` and `Debug` are safe to
/// log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupError {
    /// The reply was empty (or only a reasoning block) although the
    /// dictation was not.
    Empty,
    /// The reply opens a reasoning block it never closes.
    UnclosedReasoning,
}

impl fmt::Display for CleanupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CleanupError::Empty => f.write_str("the provider returned an empty reply"),
            CleanupError::UnclosedReasoning => {
                f.write_str("the reply opened a reasoning block it never closed")
            }
        }
    }
}

impl std::error::Error for CleanupError {}

/// Applies the three rules above to a provider's final `output`.
/// `raw_text` is the original dictation: when it mentions reasoning tags
/// itself, rule 1 is skipped.
pub fn cleanup(output: &str, raw_text: &str) -> Result<String, CleanupError> {
    let result = strip_reasoning_tags(output, raw_text)?.trim();
    if result.is_empty() && !raw_text.trim().is_empty() {
        return Err(CleanupError::Empty);
    }
    Ok(result.to_owned())
}

/// Removes the balanced reasoning blocks at the start of `work`, possibly
/// more than one. Skipped entirely when the dictation mentions reasoning
/// tags, where stripping could delete real content.
fn strip_reasoning_tags<'a>(mut work: &'a str, raw_text: &str) -> Result<&'a str, CleanupError> {
    if find_ci(raw_text, "<think").is_some() || find_ci(raw_text, "</think").is_some() {
        return Ok(work);
    }
    loop {
        let start = work.trim_start();
        let Some((open, close)) = REASONING_OPENERS
            .iter()
            .find(|(open, _)| starts_with_ci(start, open))
        else {
            break;
        };
        let Some(close_at) = find_ci(&start[open.len()..], close) else {
            return Err(CleanupError::UnclosedReasoning);
        };
        work = &start[open.len() + close_at + close.len()..];
    }
    Ok(work)
}

/// Case-insensitive `starts_with` for ASCII prefixes.
fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

/// Case-insensitive substring search. Returns the byte offset of the match.
fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}
