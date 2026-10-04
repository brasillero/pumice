//! Conservative cleanup of a provider's final text.
//!
//! Handy pastes Pumice's response verbatim into the user's text field, so any
//! wrapper a CLI adds (reasoning tags, preamble lines, code fences, quote
//! pairs) would end up in the user's document. Every rule here is exact and
//! uses the original dictation as a preservation guard: when the dictation
//! itself contains the wrapper pattern, the output is kept untouched. When
//! cleanup would lose a nonempty dictation, it fails instead, so the caller
//! can fall back to the raw text.

use std::fmt;

/// Standalone leading lines a CLI may print before the formatted text. Each
/// entry must match the whole first line (case-insensitive, trailing
/// whitespace ignored); a line that merely starts with one of these is never
/// removed. Extend this list when a new recurring wrapper is observed.
const PREAMBLES: &[&str] = &[
    // English
    "Here is the cleaned text:",
    "Here is the cleaned-up text:",
    "Here is the formatted text:",
    "Here is the corrected text:",
    "Here's the cleaned text:",
    "Here's the formatted text:",
    "Here's the corrected text:",
    "Cleaned text:",
    "Formatted text:",
    // Portuguese
    "Aqui está o texto corrigido:",
    "Aqui está o texto formatado:",
    "Aqui está o texto limpo:",
    "Texto corrigido:",
    "Texto formatado:",
    // Spanish
    "Aquí está el texto corregido:",
    "Aquí está el texto formateado:",
];

/// Reasoning-tag wrappers removed from the start of the output, as
/// (opening, closing) pairs. Tag names are matched case-insensitively.
const REASONING_OPENERS: &[(&str, &str)] =
    &[("<think>", "</think>"), ("<thinking>", "</thinking>")];

/// Quote wrappers removed when they enclose the whole output, as
/// (opening, closing) pairs.
const QUOTE_PAIRS: &[(&str, &str)] = &[("\"", "\""), ("“", "”"), ("'", "'"), ("«", "»")];

/// Why cleanup refused to return text.
///
/// It carries no dictated text, so both `Display` and `Debug` are safe to
/// log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupError {
    /// Cleanup removed everything although the dictation was nonempty. The
    /// caller must fall back to the raw dictation.
    Empty,
    /// The output opens a reasoning block it never closes. The caller must
    /// fall back to the raw dictation.
    UnclosedReasoning,
}

impl fmt::Display for CleanupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CleanupError::Empty => f.write_str("cleanup removed all dictated text"),
            CleanupError::UnclosedReasoning => {
                f.write_str("output opens a reasoning block it never closes")
            }
        }
    }
}

impl std::error::Error for CleanupError {}

/// Removes formatting wrappers from a provider's final `output`.
///
/// `raw_text` is the original dictation and acts as a preservation guard:
/// any wrapper pattern the dictation itself contains is kept. Fails with
/// [`CleanupError::Empty`] when cleanup would erase a nonempty dictation,
/// and with [`CleanupError::UnclosedReasoning`] when a reasoning block is
/// opened but never closed; in both cases the caller sends the raw dictation
/// to the user.
pub fn cleanup(output: &str, raw_text: &str) -> Result<String, CleanupError> {
    let mut work = output;

    // 1. Balanced reasoning blocks at the start, possibly more than one.
    work = strip_reasoning_tags(work, raw_text)?;

    // 2. One allow-listed preamble line.
    let start = work.trim_start();
    let line = first_line(start);
    let raw_line = first_line(raw_text.trim_start()).to_lowercase();
    if is_preamble(line) && raw_line != line.to_lowercase() {
        work = start.split_once('\n').map_or("", |(_, rest)| rest);
    }

    // 3. One code fence pair enclosing the whole remaining output.
    if !raw_text.contains("```")
        && let Some(inner) = unwrap_enclosing_fence(work.trim())
    {
        work = inner;
    }

    // 4. One quote pair wrapping the whole output.
    let trimmed = work.trim();
    let raw_trimmed = raw_text.trim();
    for &(open, close) in QUOTE_PAIRS {
        if trimmed.len() < open.len() + close.len() {
            continue;
        }
        if raw_trimmed.starts_with(open) && raw_trimmed.ends_with(close) {
            continue;
        }
        if trimmed.starts_with(open) && trimmed.ends_with(close) {
            work = &trimmed[open.len()..trimmed.len() - close.len()];
            break;
        }
    }

    // 5. Trim outside whitespace; normalize CRLF only when line endings mix.
    let mut result = work.trim().to_string();
    if mixes_line_endings(&result) {
        result = result.replace("\r\n", "\n");
    }

    // 6. Never turn a nonempty dictation into empty text.
    if result.is_empty() && !raw_text.trim().is_empty() {
        return Err(CleanupError::Empty);
    }
    Ok(result)
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

/// True when `line` (already stripped of its terminator and trailing
/// whitespace) is one of the known preamble lines.
fn is_preamble(line: &str) -> bool {
    let line = line.to_lowercase();
    PREAMBLES.iter().any(|p| p.to_lowercase() == line)
}

/// Unwraps one fence pair enclosing all of `t` (`t` must be trimmed already).
/// Returns the inner text, or `None` when `t` is not exactly one fenced
/// block. Internal fences stay untouched.
fn unwrap_enclosing_fence(t: &str) -> Option<&str> {
    let opener_end = t.find('\n')?;
    let language = t[..opener_end].strip_prefix("```")?;
    if language.chars().any(char::is_whitespace) {
        return None;
    }
    let inner = t[opener_end + 1..].strip_suffix("```")?;
    if !inner.ends_with('\n') {
        return None;
    }
    Some(inner)
}

/// True when `s` contains both CRLF and at least one lone `\r` or `\n`.
fn mixes_line_endings(s: &str) -> bool {
    let mut has_crlf = false;
    let mut has_lone = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' if chars.peek() == Some(&'\n') => {
                chars.next();
                has_crlf = true;
            }
            '\r' | '\n' => has_lone = true,
            _ => {}
        }
    }
    has_crlf && has_lone
}

/// The first line of `s` without its terminator, with trailing whitespace
/// removed.
fn first_line(s: &str) -> &str {
    s.split('\n').next().unwrap_or_default().trim_end()
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
