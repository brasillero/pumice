//! Common [`Provider`] implementation for adapters that run a CLI.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use super::diagnostic::{self, Diagnostic};
use super::{FormatInput, Provider, ProviderError, ProviderFuture};
use crate::process::{CliInvocation, ProcessRunner};

/// A CLI-specific adapter: how to call the CLI and parse its output. It does
/// not spawn processes, manage directories or enforce deadlines.
pub trait CliAdapter: Send + Sync {
    fn id(&self) -> &'static str;

    /// Describes the call for `input`; the invocation's parser reads the
    /// result.
    fn invocation(&self, input: FormatInput<'_>) -> Result<CliInvocation, ProviderError>;
}

/// Runs a [`CliAdapter`] through the shared [`ProcessRunner`].
pub struct CliProvider<A> {
    pub adapter: A,
    pub runner: Arc<ProcessRunner>,
    /// Provider timeout, capped by the caller's total deadline.
    pub timeout: Duration,
}

impl<A: CliAdapter> CliProvider<A> {
    pub fn new(adapter: A, runner: Arc<ProcessRunner>, timeout: Duration) -> CliProvider<A> {
        CliProvider {
            adapter,
            runner,
            timeout,
        }
    }
}

impl<A: CliAdapter> Provider for CliProvider<A> {
    fn id(&self) -> &'static str {
        self.adapter.id()
    }

    fn format<'a>(&'a self, input: FormatInput<'a>, deadline: Instant) -> ProviderFuture<'a> {
        Box::pin(async move {
            let invocation = self.adapter.invocation(input)?;
            let parser = invocation.parser;
            let deadline = Instant::now()
                .checked_add(self.timeout)
                .map_or(deadline, |own| own.min(deadline));
            let output = self.runner.run(invocation, deadline).await?;
            let result = parser(&output);
            if result.is_err() {
                diagnostic::record(Diagnostic::from_output(&output));
            }
            result
        })
    }
}

/// The CLI's stdout as text. Invalid UTF-8 is invalid output: a lossy
/// decode would put U+FFFD into a "successful" result instead of returning
/// the original text.
pub(crate) fn stdout_text(output: &crate::process::ProcessOutput) -> Result<&str, ProviderError> {
    std::str::from_utf8(&output.stdout)
        .map_err(|_| ProviderError::other(super::ProviderErrorCode::InvalidOutput))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ProviderErrorCode;

    #[cfg(unix)]
    fn output(stdout: &[u8]) -> crate::process::ProcessOutput {
        use std::os::unix::process::ExitStatusExt;
        crate::process::ProcessOutput {
            status: std::process::ExitStatus::from_raw(0),
            stdout: stdout.to_vec(),
            stderr_tail: Vec::new(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn stdout_text_rejects_invalid_utf8() {
        assert_eq!(stdout_text(&output("olá".as_bytes())), Ok("olá"));
        assert_eq!(
            stdout_text(&output(b"ok \xff")),
            Err(ProviderError::other(ProviderErrorCode::InvalidOutput))
        );
    }
}
