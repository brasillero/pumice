//! Prompt composition: the client's request, passed through.
//!
//! Pumice adds no instructions of its own (owner decision 2026-10-08). The
//! system prompt is the client's own `system`/`developer` messages, joined
//! with blank lines; it is empty when the client sends none (Handy). The
//! user prompt is the incoming user message, unchanged, with its transcript
//! span kept separate only so `passthrough` and logs can find it.

use std::fmt;

use crate::providers::{FormatInput, UserPrompt};
use crate::request::ExtractedRequest;

/// The formatted prompts, ready for a provider call.
pub struct ComposedPrompts {
    pub system: String,
    pub user: UserPromptOwned,
}

// Manual impl: this holds dictated text, which must never reach logs through
// `{:?}`.
impl fmt::Debug for ComposedPrompts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComposedPrompts")
            .field("system_len", &self.system.len())
            .field("user", &self.user)
            .finish()
    }
}

impl ComposedPrompts {
    /// Borrows the composed prompts as the [`FormatInput`] providers take.
    pub fn format_input(&self) -> FormatInput<'_> {
        FormatInput {
            system_prompt: &self.system,
            user_prompt: UserPrompt {
                before_text: &self.user.before_text,
                after_text: &self.user.after_text,
            },
            text: &self.user.text,
        }
    }
}

/// The user message sent to a provider: `before_text + text + after_text`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct UserPromptOwned {
    /// The incoming message up to the transcript.
    pub before_text: String,
    /// The transcript span of the message (the whole message when it has no
    /// `<transcript>` envelope).
    pub text: String,
    /// The incoming message from the transcript on.
    pub after_text: String,
}

// Manual impl: this holds dictated text, which must never reach logs through
// `{:?}`.
impl fmt::Debug for UserPromptOwned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UserPromptOwned")
            .field("before_text_len", &self.before_text.len())
            .field("text_len", &self.text.len())
            .field("after_text_len", &self.after_text.len())
            .finish()
    }
}

impl UserPromptOwned {
    /// The complete user message: `before_text + text + after_text`.
    pub fn as_message(&self) -> String {
        let mut message =
            String::with_capacity(self.before_text.len() + self.text.len() + self.after_text.len());
        message.push_str(&self.before_text);
        message.push_str(&self.text);
        message.push_str(&self.after_text);
        message
    }
}

/// Builds the system prompt and user message for a provider call.
///
/// The system prompt is the incoming `system`/`developer` texts, in order,
/// joined with a blank line (whitespace-only ones skipped; empty when there
/// are none). The user message is the incoming one, unchanged.
pub fn compose_prompts(request: &ExtractedRequest) -> ComposedPrompts {
    let system_parts: Vec<&str> = request
        .system_texts
        .iter()
        .map(String::as_str)
        .filter(|text| !text.trim().is_empty())
        .collect();

    ComposedPrompts {
        system: system_parts.join("\n\n"),
        user: UserPromptOwned {
            before_text: request.before_text.clone(),
            text: request.text.clone(),
            after_text: request.after_text.clone(),
        },
    }
}
