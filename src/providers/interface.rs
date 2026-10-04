//! The interface every provider implements, and its safe error type.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// Future returned by [`Provider::format`]: the formatted text or a safe error.
pub type ProviderFuture<'a> =
    Pin<Box<dyn Future<Output = Result<String, ProviderError>> + Send + 'a>>;

/// The user message around the dictated text.
///
/// Keeping the text span separate preserves an incoming message (such as
/// Handy's) without duplicating the transcript or substituting into a
/// template. The message sent to the CLI is `before_text + text + after_text`.
#[derive(Clone, Copy, Default)]
pub struct UserPrompt<'a> {
    pub before_text: &'a str,
    pub after_text: &'a str,
}

/// Everything a provider needs to format one dictation.
#[derive(Clone, Copy)]
pub struct FormatInput<'a> {
    pub system_prompt: &'a str,
    pub user_prompt: UserPrompt<'a>,
    pub text: &'a str,
}

// Manual impls: these hold dictated text and prompts, which must never reach
// logs through `{:?}`.
impl fmt::Debug for UserPrompt<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UserPrompt")
            .field("before_text_len", &self.before_text.len())
            .field("after_text_len", &self.after_text.len())
            .finish()
    }
}

impl fmt::Debug for FormatInput<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FormatInput")
            .field("system_prompt_len", &self.system_prompt.len())
            .field("user_prompt", &self.user_prompt)
            .field("text_len", &self.text.len())
            .finish()
    }
}

/// A formatting backend, identified by a string ID such as `"claude"`.
pub trait Provider: Send + Sync {
    fn id(&self) -> &'static str;

    /// Formats `input`, finishing before `deadline`. Returns the provider's
    /// final text after protocol parsing; output cleanup happens later.
    fn format<'a>(
        &'a self,
        input: FormatInput<'a>,
        deadline: tokio::time::Instant,
    ) -> ProviderFuture<'a>;
}

/// Safe classification of a provider failure.
///
/// It carries no captured output, diagnostic text or dictated text, so both
/// `Display` and `Debug` are safe to log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderError {
    NotInstalled,
    NotLoggedIn,
    Timeout,
    QuotaExceeded { retry_after: Option<Duration> },
    RateLimited { retry_after: Option<Duration> },
    Other { code: ProviderErrorCode },
}

/// Detail for [`ProviderError::Other`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderErrorCode {
    Spawn,
    Io,
    InvalidConfiguration,
    AuthenticationRejected,
    UnsupportedShim,
    InvalidOutput,
    OutputTooLarge,
    UnexpectedToolActivity,
    NonzeroExit,
}

impl ProviderError {
    /// Shorthand for `ProviderError::Other { code }`.
    pub fn other(code: ProviderErrorCode) -> ProviderError {
        ProviderError::Other { code }
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderError::NotInstalled => f.write_str("the CLI is not installed"),
            ProviderError::NotLoggedIn => f.write_str("the CLI is not logged in"),
            ProviderError::Timeout => f.write_str("the CLI timed out"),
            ProviderError::QuotaExceeded { .. } => f.write_str("the CLI usage quota is exhausted"),
            ProviderError::RateLimited { .. } => f.write_str("the CLI is rate limited"),
            ProviderError::Other { code } => code.fmt(f),
        }
    }
}

impl fmt::Display for ProviderErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ProviderErrorCode::Spawn => "the CLI could not be started",
            ProviderErrorCode::Io => "I/O error while running the CLI",
            ProviderErrorCode::InvalidConfiguration => "invalid provider configuration",
            ProviderErrorCode::AuthenticationRejected => "the CLI rejected its credentials",
            ProviderErrorCode::UnsupportedShim => {
                "the CLI is a script wrapper that is not supported"
            }
            ProviderErrorCode::InvalidOutput => "the CLI returned output that could not be parsed",
            ProviderErrorCode::OutputTooLarge => "the CLI returned too much output",
            ProviderErrorCode::UnexpectedToolActivity => "the CLI tried to use a tool",
            ProviderErrorCode::NonzeroExit => "the CLI reported a failure",
        })
    }
}

impl std::error::Error for ProviderError {}
