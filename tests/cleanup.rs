//! Tests for conservative output cleanup (`src/cleanup.rs`).
//!
//! Every rule has a positive case (wrapper removed) and a preservation case
//! (legitimate content kept). Preservation matters more: Handy pastes the
//! response verbatim, and stripping real dictated content is worse than
//! leaving a wrapper in place.

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

// Rule 2: one allow-listed preamble line.

#[test]
fn strips_english_preamble() {
    assert_eq!(
        ok(
            "Here is the cleaned text:\nThe budget is due Friday.",
            "the budget is due friday"
        ),
        "The budget is due Friday."
    );
}

#[test]
fn strips_preamble_case_insensitively_with_trailing_whitespace() {
    assert_eq!(
        ok(
            "AQUI ESTÁ O TEXTO CORRIGIDO:   \nReunião às nove.",
            "reunião às nove"
        ),
        "Reunião às nove."
    );
}

#[test]
fn strips_spanish_preamble() {
    assert_eq!(
        ok("Aquí está el texto formateado:\nHola Juan.", "hola juan"),
        "Hola Juan."
    );
}

#[test]
fn strips_preamble_from_crlf_output() {
    assert_eq!(
        ok("Formatted text:\r\nBody line.", "body line"),
        "Body line."
    );
}

#[test]
fn keeps_sentence_starting_with_here_is() {
    let output = "Here is my proposal for the budget.";
    assert_eq!(ok(output, "here is my proposal for the budget"), output);
}

#[test]
fn keeps_aqui_esta_minha_resposta() {
    let output = "Aqui está minha resposta para o cliente.";
    assert_eq!(
        ok(output, "aqui está minha resposta para o cliente"),
        output
    );
}

#[test]
fn keeps_dictation_starting_with_preamble_line() {
    let output = "Cleaned text:\nactual dictated notes";
    assert_eq!(ok(output, "Cleaned text:\nactual dictated notes"), output);
}

// Rule 3: one code fence pair enclosing the whole output.

#[test]
fn unwraps_plain_fence() {
    assert_eq!(ok("```\nHello\n```", "hello"), "Hello");
}

#[test]
fn unwraps_language_fence() {
    assert_eq!(
        ok("```python\nprint(\"hi\")\n```", "print hi"),
        "print(\"hi\")"
    );
}

#[test]
fn unwraps_fence_with_surrounding_newlines() {
    assert_eq!(ok("\n```\nHello\n```\n\n", "hello"), "Hello");
}

#[test]
fn keeps_internal_fence_inside_unwrapped_block() {
    assert_eq!(
        ok("```\nUse this:\n```\ncode\n```", "use this code"),
        "Use this:\n```\ncode"
    );
}

#[test]
fn keeps_fence_when_dictation_has_fence() {
    let output = "```\necho hi\n```";
    let raw = "run ```echo hi``` in the terminal please";
    assert_eq!(ok(output, raw), output);
}

#[test]
fn keeps_fence_that_does_not_enclose_everything() {
    let output = "Before\n```\ncode\n```\nAfter";
    assert_eq!(ok(output, "before and after"), output);
}

// Rule 4: one quote pair wrapping the whole output.

#[test]
fn strips_straight_double_quotes() {
    assert_eq!(ok("\"Hello there\"", "hello there"), "Hello there");
}

#[test]
fn strips_curly_quotes() {
    assert_eq!(ok("“Olá mundo”", "olá mundo"), "Olá mundo");
}

#[test]
fn strips_guillemets() {
    assert_eq!(ok("«relatório»", "relatório"), "relatório");
}

#[test]
fn strips_single_quotes() {
    assert_eq!(ok("'just a note'", "just a note"), "just a note");
}

#[test]
fn keeps_partial_quote_pair() {
    let output = "\"Hi,\" she said.";
    assert_eq!(ok(output, "hi she said"), output);
}

#[test]
fn keeps_apostrophes() {
    let output = "It's done.";
    assert_eq!(ok(output, "it's done"), output);
}

#[test]
fn keeps_quoted_speech_inside_text() {
    let output = "She said \"hello\" loudly.";
    assert_eq!(ok(output, "she said hello loudly"), output);
}

#[test]
fn keeps_quotes_dictated_by_user() {
    let output = "\"orçamento\"";
    assert_eq!(ok(output, "\"orçamento\""), "\"orçamento\"");
}

// Rule 5: outside whitespace and mixed line endings.

#[test]
fn trims_outside_whitespace() {
    assert_eq!(ok("  \nHello\n\n", "hello"), "Hello");
}

#[test]
fn normalizes_mixed_line_endings() {
    assert_eq!(
        ok("One\r\nTwo\nThree\r\n", "one two three"),
        "One\nTwo\nThree"
    );
}

#[test]
fn keeps_pure_crlf_line_endings() {
    assert_eq!(ok("One\r\nTwo\r\n", "one two"), "One\r\nTwo");
}

#[test]
fn preserves_blank_lines_and_list_indentation() {
    let output = "Intro\n\n- item one\n  - nested\n\nEnd";
    assert_eq!(ok(output, "intro item nested end"), output);
}

// Rule 6: never turn a nonempty dictation into empty text.

#[test]
fn whitespace_only_output_fails_empty() {
    assert_eq!(cleanup("   ", "hello"), Err(CleanupError::Empty));
}

#[test]
fn preamble_only_output_fails_empty() {
    assert_eq!(
        cleanup("Here is the cleaned text:", "real dictation"),
        Err(CleanupError::Empty)
    );
}

#[test]
fn empty_output_and_empty_dictation_succeed() {
    assert_eq!(ok("", ""), "");
}

// Combined wrappers and realistic dictations.

#[test]
fn strips_think_then_preamble_then_fence() {
    let output =
        "<think>reasoning about the text</think>\nHere is the cleaned text:\n```\nHello world\n```";
    assert_eq!(ok(output, "hello world"), "Hello world");
}

#[test]
fn keeps_portuguese_list_with_accents() {
    let output = "Aqui está o texto corrigido:\n- Pão\n- Leite\n- Açúcar";
    assert_eq!(
        ok(output, "comprar pão leite e açúcar"),
        "- Pão\n- Leite\n- Açúcar"
    );
}

#[test]
fn keeps_numbered_list_intact() {
    let output = "1. Primeiro\n2. Segundo";
    assert_eq!(ok(output, "primeiro segundo"), output);
}

#[test]
fn keeps_emoji() {
    let output = "Formatted text:\nFeito ✅\nBom trabalho 🎉";
    assert_eq!(
        ok(output, "feito bom trabalho"),
        "Feito ✅\nBom trabalho 🎉"
    );
}

#[test]
fn idempotent_over_positive_fixtures() {
    const CASES: &[(&str, &str)] = &[
        (
            "<think>let me clean this</think>The report is late.",
            "the report is late",
        ),
        (
            "<THINK>first</THINK>\n<thinking>second</thinking>\nFinal answer.",
            "final answer",
        ),
        (
            "Here is the cleaned text:\nThe budget is due Friday.",
            "the budget is due friday",
        ),
        (
            "AQUI ESTÁ O TEXTO CORRIGIDO:   \nReunião às nove.",
            "reunião às nove",
        ),
        ("Aquí está el texto formateado:\nHola Juan.", "hola juan"),
        ("Formatted text:\r\nBody line.", "body line"),
        ("```\nHello\n```", "hello"),
        ("```python\nprint(\"hi\")\n```", "print hi"),
        ("```\nUse this:\n```\ncode\n```", "use this code"),
        ("\"Hello there\"", "hello there"),
        ("“Olá mundo”", "olá mundo"),
        ("«relatório»", "relatório"),
        ("'just a note'", "just a note"),
        ("  \nHello\n\n", "hello"),
        ("One\r\nTwo\nThree\r\n", "one two three"),
        (
            "Intro\n\n- item one\n  - nested\n\nEnd",
            "intro item nested end",
        ),
        (
            "<think>reasoning about the text</think>\nHere is the cleaned text:\n```\nHello world\n```",
            "hello world",
        ),
        (
            "Aqui está o texto corrigido:\n- Pão\n- Leite\n- Açúcar",
            "comprar pão leite e açúcar",
        ),
        ("1. Primeiro\n2. Segundo", "primeiro segundo"),
        (
            "Formatted text:\nFeito ✅\nBom trabalho 🎉",
            "feito bom trabalho",
        ),
    ];
    for (output, raw) in CASES {
        let once = cleanup(output, raw).unwrap_or_else(|e| panic!("first pass failed: {e:?}"));
        let twice = cleanup(&once, raw).unwrap_or_else(|e| panic!("second pass failed: {e:?}"));
        assert_eq!(once, twice, "cleanup is not idempotent for {output:?}");
    }
}

#[test]
fn quote_rule_keeps_text_that_opens_and_closes_with_separate_quotes() {
    let out = "\"Hi,\" she said, \"bye.\"";
    assert_eq!(cleanup(out, "hi she said bye").unwrap(), out);
}

#[test]
fn quote_rule_keeps_wrapped_text_with_inner_apostrophe() {
    let out = "'it's done'";
    assert_eq!(cleanup(out, "its done").unwrap(), out);
}

#[test]
fn fence_with_crlf_line_endings_is_unwrapped() {
    let out = "```\r\nFormatted text.\r\n```";
    assert_eq!(cleanup(out, "formatted text").unwrap(), "Formatted text.");
}

#[test]
fn keeps_dictated_heading_the_model_punctuated_like_a_preamble() {
    // The user dictated a heading; the model only added a colon.
    assert_eq!(
        ok(
            "Formatted text:\nThe report is ready.",
            "formatted text\nthe report is ready"
        ),
        "Formatted text:\nThe report is ready."
    );
}

#[test]
fn still_strips_a_preamble_the_dictation_does_not_open_with() {
    assert_eq!(
        ok(
            "Formatted text:\nSend the formatted text to John.",
            "send the formatted text to john"
        ),
        "Send the formatted text to John."
    );
}

#[test]
fn keeps_dictated_quotes_when_the_model_changes_their_style() {
    assert_eq!(
        ok("“Ship it on Friday.”", "\"ship it on friday\""),
        "“Ship it on Friday.”"
    );
    assert_eq!(
        ok("\"Ship it on Friday.\"", "“ship it on friday”"),
        "\"Ship it on Friday.\""
    );
}

#[test]
fn strips_added_outer_quotes_around_separate_dictated_quotations() {
    assert_eq!(
        ok("\"“Hello” and “goodbye”\"", "“hello” and “goodbye”"),
        "“Hello” and “goodbye”"
    );
}

#[test]
fn strips_a_preamble_on_top_of_the_dictated_heading() {
    assert_eq!(
        ok(
            "Formatted text:\nFormatted text:\nHello.",
            "formatted text\nhello"
        ),
        "Formatted text:\nHello."
    );
}

#[test]
fn keeps_a_dictated_quotation_with_nested_quotes_of_another_style() {
    assert_eq!(
        ok("\"Say «hello».\"", "\"say «hello»\""),
        "\"Say «hello».\""
    );
}

#[test]
fn separate_single_quoted_phrases_are_not_one_quotation() {
    assert_eq!(
        ok("\"'Hello' and 'goodbye'\"", "'hello' and 'goodbye'"),
        "'Hello' and 'goodbye'"
    );
}

#[test]
fn keeps_a_single_quoted_dictation_with_an_apostrophe() {
    assert_eq!(ok("'Don't stop.'", "'don't stop'"), "'Don't stop.'");
}

#[test]
fn keeps_headings_the_dictation_repeats() {
    assert_eq!(
        ok(
            "Formatted text:\nFormatted text:\nHello.",
            "formatted text\nformatted text\nhello"
        ),
        "Formatted text:\nFormatted text:\nHello."
    );
}

#[test]
fn strips_an_added_preamble_separated_by_a_blank_line() {
    assert_eq!(
        ok(
            "Formatted text:\n\nFormatted text:\nHello.",
            "formatted text\nhello"
        ),
        "Formatted text:\nHello."
    );
}

#[test]
fn keeps_a_dictated_heading_the_model_moved_to_its_own_line() {
    assert_eq!(
        ok(
            "Formatted text:\nThe report is ready.",
            "formatted text. the report is ready"
        ),
        "Formatted text:\nThe report is ready."
    );
}
