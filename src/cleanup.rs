//! Minimal cleanup of a provider's final text.
//!
//! Handy pastes Pumice's response verbatim, so Pumice trusts what the model
//! returns and changes as little as possible (owner decision 2026-10-07):
//! formatting instructions belong in the prompt, not in post-processing.
//! Cleanup looks only at the reply, never at the request (owner decision
//! 2026-10-08). Only three rules remain:
//!
//! 1. Closed reasoning blocks (`<think>…</think>`) at the very start are
//!    removed. An opening tag that never closes is not a reasoning block:
//!    the reply is kept as is.
//! 2. Whitespace around the text is trimmed.
//! 3. An empty result is rejected.
//!
//! A rejection makes the request fail with an HTTP error, and the log names
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
    /// The reply was empty, or held only reasoning blocks.
    Empty,
}

impl fmt::Display for CleanupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CleanupError::Empty => f.write_str("the provider returned an empty reply"),
        }
    }
}

impl std::error::Error for CleanupError {}

/// Applies the three rules above to a provider's final `output`.
///
/// The pipeline never calls a provider for an empty dictation, so an empty
/// reply here is always a failure.
pub fn cleanup(output: &str) -> Result<String, CleanupError> {
    let result = strip_reasoning_tags(output).trim();
    if result.is_empty() {
        return Err(CleanupError::Empty);
    }
    Ok(result.to_owned())
}

/// Removes the closed reasoning blocks at the start of `work`, possibly more
/// than one. Stops at the first opening tag without its closing tag and
/// keeps the rest as is.
fn strip_reasoning_tags(mut work: &str) -> &str {
    loop {
        let start = work.trim_start();
        let Some((open, close)) = REASONING_OPENERS
            .iter()
            .find(|(open, _)| starts_with_ci(start, open))
        else {
            return work;
        };
        let Some(close_at) = find_ci(&start[open.len()..], close) else {
            return work;
        };
        work = &start[open.len() + close_at + close.len()..];
    }
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
