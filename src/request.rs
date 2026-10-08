//! Validation and extraction of the chat-completion requests Pumice accepts.
//!
//! Pumice serves two shapes of client: Handy, which wraps the dictation in a
//! `<transcript>…</transcript>` envelope followed by its own formatting
//! prompt inside one user message, and plain dictation clients, whose whole
//! user message is the dictation. Extraction keeps the incoming user message
//! reconstructable byte for byte (`before_text + text + after_text`) while
//! exposing the transcript separately as `raw_text` for fallback and cleanup.

use std::fmt;

use serde::Deserialize;

const OPEN_TAG: &str = "<transcript>";
const CLOSE_TAG: &str = "</transcript>";

/// The subset of the OpenAI chat-completion request Pumice accepts.
///
/// Unknown fields (Handy sends `reasoning_effort`, other clients send more)
/// are ignored.
#[derive(Deserialize)]
pub struct ChatCompletionRequest {
    /// Requested model or provider ID. Absent or empty returns the original
    /// text ([`RawReason::NoModel`](crate::pipeline::RawReason::NoModel)).
    pub model: Option<String>,
    pub messages: Vec<Message>,
    pub stream: Option<bool>,
}

/// One chat message. Content is either a plain string or an array of parts.
#[derive(Deserialize)]
pub struct Message {
    pub role: String,
    pub content: Content,
}

impl fmt::Debug for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Holds dictated text: only the role and rough shape are log-safe.
        f.debug_struct("Message")
            .field("role", &self.role)
            .field("content", &self.content)
            .finish()
    }
}

/// Message content: either one string or an array of parts.
#[derive(Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Parts(Vec<ContentPart>),
}

impl fmt::Debug for Content {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Holds dictated text: only the shape is log-safe.
        match self {
            Content::Text(text) => f.debug_tuple("Text").field(&text.len()).finish(),
            Content::Parts(parts) => f.debug_tuple("Parts").field(&parts.len()).finish(),
        }
    }
}

/// One content part. Only text parts are supported; unknown fields of a text
/// part are ignored.
#[derive(Deserialize)]
pub struct ContentPart {
    #[serde(rename = "type")]
    pub part_type: String,
    #[serde(default)]
    pub text: Option<String>,
}

/// A validated request with the transcript span separated from its message.
///
/// `before_text + text + after_text` reconstructs the incoming user message
/// exactly. `text` is what formatting sends (literal transcript tags inside
/// plain dictation are escaped there); `raw_text` is the transcript as
/// dictated, unescaped and without the envelope's framing newlines, and is
/// what cleanup and the raw fallback use.
pub struct ExtractedRequest {
    pub model: Option<String>,
    /// Incoming `system`/`developer` messages, in order.
    pub system_texts: Vec<String>,
    /// Everything before the transcript, including the opening tag.
    pub before_text: String,
    /// The transcript as sent to formatting.
    pub text: String,
    /// Everything after the transcript, including the closing tag.
    pub after_text: String,
    /// The transcript exactly as dictated.
    pub raw_text: String,
}

// Manual impl: this holds dictated text, which must never reach logs through
// `{:?}`.
impl fmt::Debug for ExtractedRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtractedRequest")
            .field("model", &self.model)
            .field("system_texts_len", &self.system_texts.len())
            .field("before_text_len", &self.before_text.len())
            .field("text_len", &self.text.len())
            .field("after_text_len", &self.after_text.len())
            .field("raw_text_len", &self.raw_text.len())
            .field("is_empty", &self.is_empty())
            .finish()
    }
}

impl ExtractedRequest {
    /// True when the transcript holds no non-whitespace text. The pipeline
    /// answers these with an empty completion without invoking a CLI.
    pub fn is_empty(&self) -> bool {
        self.raw_text.trim().is_empty()
    }
}

/// Why a request is not a usable dictation.
///
/// It carries no request text, so both `Display` and `Debug` are safe to log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    /// No user message at all.
    MissingUserMessage,
    /// Conversation histories (more than one user message) are unsupported.
    MultipleUserMessages,
    /// Only `system`, `developer` and `user` roles are supported.
    UnsupportedRole,
    /// Only text strings and text parts are supported.
    UnsupportedContent,
    /// Transcript tags with an opening tag cannot form one envelope: a
    /// closing tag comes before the opening tag, none comes after it, or a
    /// second opening tag competes before the closing one. (Text with no
    /// opening tag at all is plain dictation, never malformed.)
    MalformedEnvelope,
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RequestError::MissingUserMessage => "the request has no user message",
            RequestError::MultipleUserMessages => "the request has more than one user message",
            RequestError::UnsupportedRole => "the request has an unsupported message role",
            RequestError::UnsupportedContent => {
                "the request has unsupported non-text message content"
            }
            RequestError::MalformedEnvelope => "the request has a malformed <transcript> envelope",
        })
    }
}

impl std::error::Error for RequestError {}

/// Validates `request` and extracts the dictation.
///
/// Accepts any number of `system`/`developer` messages plus exactly one
/// `user` message. With a `<transcript>…</transcript>` envelope the
/// transcript is what's inside it and the surrounding message is kept
/// unchanged. Without one, the whole user message is the dictation and is
/// wrapped in Pumice's own envelope for formatting.
pub fn extract_request(request: ChatCompletionRequest) -> Result<ExtractedRequest, RequestError> {
    let model = request.model.and_then(|model| {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    });

    let mut system_texts = Vec::new();
    let mut user_text: Option<String> = None;
    for message in request.messages {
        match message.role.as_str() {
            "system" | "developer" => system_texts.push(content_to_text(message.content)?),
            "user" => {
                if user_text.is_some() {
                    return Err(RequestError::MultipleUserMessages);
                }
                user_text = Some(content_to_text(message.content)?);
            }
            _ => return Err(RequestError::UnsupportedRole),
        }
    }
    let user_text = user_text.ok_or(RequestError::MissingUserMessage)?;

    let opens: Vec<usize> = user_text
        .match_indices(OPEN_TAG)
        .map(|(index, _)| index)
        .collect();
    let closes: Vec<usize> = user_text
        .match_indices(CLOSE_TAG)
        .map(|(index, _)| index)
        .collect();

    match opens.first() {
        None => {
            // No opening tag: the whole message is the text, passed through
            // unchanged (Pumice adds no envelope of its own).
            Ok(ExtractedRequest {
                model,
                system_texts,
                before_text: String::new(),
                text: user_text.clone(),
                after_text: String::new(),
                raw_text: user_text,
            })
        }
        Some(&open) => {
            // The first opening tag forms the envelope. A closing tag before
            // it, none after it, or a second opening tag before it, makes
            // the envelope ambiguous. Tags mentioned later in the message
            // (Handy's prompt names them) are literal prompt text.
            if closes.iter().any(|&close| close < open) {
                return Err(RequestError::MalformedEnvelope);
            }
            let Some(&close) = closes.iter().find(|&&close| close > open) else {
                return Err(RequestError::MalformedEnvelope);
            };
            if opens.iter().any(|&other| other > open && other < close) {
                return Err(RequestError::MalformedEnvelope);
            }
            let open_end = open + OPEN_TAG.len();
            // Strip exactly one framing line break ('\n' or '\r\n') after the
            // opening tag and one before the closing tag; all other whitespace
            // is transcript. The stripped line breaks stay in before/after so
            // the message still reconstructs byte for byte.
            let mut text_start = open_end + leading_line_break(&user_text[open_end..]);
            let text_end = close - trailing_line_break(&user_text[..close]);
            // When the only content between the tags is one line break, both
            // framing strips can claim the same characters. Clamp so the slice
            // is empty instead of crossing, then the usual empty-transcript
            // path returns the raw text unchanged.
            if text_start > text_end {
                text_start = text_end;
            }
            let transcript = user_text[text_start..text_end].to_owned();
            Ok(ExtractedRequest {
                model,
                system_texts,
                before_text: user_text[..text_start].to_owned(),
                text: transcript.clone(),
                after_text: user_text[text_end..].to_owned(),
                raw_text: transcript,
            })
        }
    }
}

/// Length of the line break `text` starts with: 2 for `\r\n`, 1 for `\n`,
/// else 0.
fn leading_line_break(text: &str) -> usize {
    if text.starts_with("\r\n") {
        2
    } else if text.starts_with('\n') {
        1
    } else {
        0
    }
}

/// Length of the line break `text` ends with: 2 for `\r\n`, 1 for `\n`,
/// else 0.
fn trailing_line_break(text: &str) -> usize {
    if text.ends_with("\r\n") {
        2
    } else if text.ends_with('\n') {
        1
    } else {
        0
    }
}

/// Concatenates content parts into one message text, rejecting non-text parts.
fn content_to_text(content: Content) -> Result<String, RequestError> {
    match content {
        Content::Text(text) => Ok(text),
        Content::Parts(parts) => {
            let mut text = String::new();
            for part in parts {
                if part.part_type != "text" {
                    return Err(RequestError::UnsupportedContent);
                }
                text.push_str(part.text.as_deref().unwrap_or_default());
            }
            Ok(text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconstructs_handy_envelope_without_framing_newlines() {
        let content = "<transcript>\nOlá mundo\n</transcript>\n\nClean it.";
        let extracted = extract(content);
        assert_eq!(extracted.before_text, "<transcript>\n");
        assert_eq!(extracted.text, "Olá mundo");
        assert_eq!(extracted.raw_text, "Olá mundo");
        assert_eq!(extracted.after_text, "\n</transcript>\n\nClean it.");
        assert_eq!(
            format!(
                "{}{}{}",
                extracted.before_text, extracted.text, extracted.after_text
            ),
            content
        );
    }

    #[test]
    fn envelope_without_framing_newlines_still_splits() {
        let content = "<transcript>Olá</transcript>";
        let extracted = extract(content);
        assert_eq!(extracted.before_text, "<transcript>");
        assert_eq!(extracted.text, "Olá");
        assert_eq!(extracted.after_text, "</transcript>");
    }

    #[test]
    fn preserves_transcript_whitespace_beyond_framing() {
        let extracted = extract("<transcript>\n primeiro\n\n segundo \n</transcript>");
        assert_eq!(extracted.raw_text, " primeiro\n\n segundo ");
        assert!(!extracted.is_empty());
    }

    #[test]
    fn empty_envelope_with_single_framing_newline_is_empty() {
        let content = "<transcript>\n</transcript>";
        let extracted = extract(content);
        assert_eq!(extracted.text, "");
        assert_eq!(extracted.raw_text, "");
        assert!(extracted.is_empty());
        assert_eq!(
            format!(
                "{}{}{}",
                extracted.before_text, extracted.text, extracted.after_text
            ),
            content
        );
    }

    #[test]
    fn empty_envelope_without_framing_newlines_is_empty() {
        let content = "<transcript></transcript>";
        let extracted = extract(content);
        assert_eq!(extracted.text, "");
        assert_eq!(extracted.raw_text, "");
        assert!(extracted.is_empty());
        assert_eq!(
            format!(
                "{}{}{}",
                extracted.before_text, extracted.text, extracted.after_text
            ),
            content
        );
    }

    #[test]
    fn empty_envelope_with_double_framing_newline_is_empty() {
        let content = "<transcript>\n\n</transcript>";
        let extracted = extract(content);
        assert_eq!(extracted.text, "");
        assert_eq!(extracted.raw_text, "");
        assert!(extracted.is_empty());
        assert_eq!(
            format!(
                "{}{}{}",
                extracted.before_text, extracted.text, extracted.after_text
            ),
            content
        );
    }

    #[test]
    fn crlf_framing_is_stripped_like_lf() {
        let content = "<transcript>\r\nhello\r\n</transcript>\r\nClean it.";
        let extracted = extract(content);
        assert_eq!(extracted.text, "hello");
        assert_eq!(extracted.raw_text, "hello");
        assert_eq!(
            format!(
                "{}{}{}",
                extracted.before_text, extracted.text, extracted.after_text
            ),
            content
        );
    }

    #[test]
    fn empty_crlf_envelope_is_empty() {
        for content in [
            "<transcript>\r\n</transcript>",
            "<transcript>\r\n\r\n</transcript>",
        ] {
            let extracted = extract(content);
            assert_eq!(extracted.text, "", "{content:?}");
            assert_eq!(
                format!(
                    "{}{}{}",
                    extracted.before_text, extracted.text, extracted.after_text
                ),
                content
            );
        }
    }

    fn extract(content: &str) -> ExtractedRequest {
        extract_request(ChatCompletionRequest {
            model: None,
            messages: vec![Message {
                role: "user".to_owned(),
                content: Content::Text(content.to_owned()),
            }],
            stream: None,
        })
        .expect("request should extract")
    }
}
