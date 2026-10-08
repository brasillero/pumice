//! Tests for conservative output cleanup (`src/cleanup.rs`).
//!
//! Cleanup is minimal on purpose (owner decision 2026-10-07): only reasoning
//! blocks at the start, outer whitespace and empty replies are handled.
//! Everything else the model returns is kept as is.

use pumice::cleanup::{CleanupError, cleanup};

/// Runs cleanup and panics with context when it fails.
fn ok(output: &str, raw: &str) -> String {
    cleanup(output, raw).expect("cleanup should succeed")
}

// Rule 1: reasoning tags at the start of the output.

#[test]
fn strips_single_think_block() {
    assert_eq!(
        ok(
            "<think>let me clean this</think>The report is late.",
            "the report is late"
        ),
        "The report is late."
    );
}

#[test]
fn strips_multiple_case_insensitive_reasoning_blocks() {
    assert_eq!(
        ok(
            "<THINK>first</THINK>\n<thinking>second</thinking>\nFinal answer.",
            "final answer"
        ),
        "Final answer."
    );
}

#[test]
fn unclosed_reasoning_block_fails() {
    assert_eq!(
        cleanup("<think>never closed", "some dictation"),
        Err(CleanupError::UnclosedReasoning)
    );
    assert_eq!(
        cleanup("<thinking>also never closed", "some dictation"),
        Err(CleanupError::UnclosedReasoning)
    );
}

#[test]
fn keeps_reasoning_span_inside_text() {
    let output = "Read the <think> section again before replying.";
    assert_eq!(ok(output, "read the think section again"), output);
}

#[test]
fn keeps_reasoning_tags_dictated_by_user() {
    let raw = "explain what a <think> tag does in templates";
    let output = "<think>the user wants an explanation</think>Use a <think> tag like this.";
    assert_eq!(ok(output, raw), output);
}

// Rule 2: outer whitespace.

#[test]
fn trims_outer_whitespace_only() {
    assert_eq!(
        ok("\n  Line one.\n\n  Line two.  \n", "x"),
        "Line one.\n\n  Line two."
    );
}

// Rule 3: an empty reply for a nonempty dictation fails, with its reason.

#[test]
fn empty_reply_fails() {
    assert_eq!(cleanup("   \n", "some dictation"), Err(CleanupError::Empty));
    assert_eq!(
        cleanup("<think>only thinking</think>", "some dictation"),
        Err(CleanupError::Empty)
    );
}

#[test]
fn empty_reply_for_an_empty_dictation_is_fine() {
    assert_eq!(ok("", "  "), "");
}

#[test]
fn errors_describe_the_reason() {
    assert_eq!(
        CleanupError::Empty.to_string(),
        "the provider returned an empty reply"
    );
    assert_eq!(
        CleanupError::UnclosedReasoning.to_string(),
        "the reply opened a reasoning block it never closed"
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
        assert_eq!(ok(output, "texto ditado em português"), output);
    }
}
