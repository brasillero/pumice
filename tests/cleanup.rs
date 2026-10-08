//! Tests for conservative output cleanup (`src/cleanup.rs`).
//!
//! Cleanup is minimal on purpose (owner decisions 2026-10-07 and
//! 2026-10-08): only closed reasoning blocks at the start, outer whitespace
//! and empty replies are handled, looking at the reply alone. Everything
//! else the model returns is kept as is.

use pumice::cleanup::{CleanupError, cleanup};

/// Runs cleanup and panics with context when it fails.
fn ok(output: &str) -> String {
    cleanup(output).expect("cleanup should succeed")
}

// Rule 1: reasoning tags at the start of the output.

#[test]
fn strips_single_think_block() {
    assert_eq!(
        ok("<think>let me clean this</think>The report is late."),
        "The report is late."
    );
}

#[test]
fn strips_multiple_case_insensitive_reasoning_blocks() {
    assert_eq!(
        ok("<THINK>first</THINK>\n<thinking>second</thinking>\nFinal answer."),
        "Final answer."
    );
}

#[test]
fn unclosed_reasoning_tag_is_kept_as_text() {
    for output in ["<think>never closed", "<thinking>also never closed"] {
        assert_eq!(ok(output), output);
    }
}

#[test]
fn unclosed_tag_after_a_closed_block_is_kept() {
    assert_eq!(
        ok("<think>reasoning</think>\n<think> stays in the answer"),
        "<think> stays in the answer"
    );
}

#[test]
fn keeps_reasoning_span_inside_text() {
    let output = "Read the <think> section again before replying.";
    assert_eq!(ok(output), output);
}

// Rule 2: outer whitespace.

#[test]
fn trims_outer_whitespace_only() {
    assert_eq!(
        ok("\n  Line one.\n\n  Line two.  \n"),
        "Line one.\n\n  Line two."
    );
}

// Rule 3: an empty reply fails, with its reason.

#[test]
fn empty_reply_fails() {
    for output in ["", "   \n", "<think>only thinking</think>"] {
        assert_eq!(cleanup(output), Err(CleanupError::Empty));
    }
}

#[test]
fn errors_describe_the_reason() {
    assert_eq!(
        CleanupError::Empty.to_string(),
        "the provider returned an empty reply"
    );
}

// Everything else is the model's choice and stays untouched.

#[test]
fn keeps_the_models_wording_as_is() {
    for output in [
        "Here is the formatted text:\nThe report is ready.",
        "Formatted text:\nThe report is ready.",
        "\"Ship it on Friday.\"",
        "“Ship it on Friday.”",
        "```\ncode block\n```",
        "Line one.\r\nLine two.\nLine three.",
    ] {
        assert_eq!(ok(output), output);
    }
}
