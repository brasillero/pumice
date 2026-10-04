//! The formatting pipeline: provider selection and the fallback chain,
//! deadline enforcement, one active run at a time, and the raw-text
//! fallback guarantee.
//!
//! Every outcome returns either formatted-and-cleaned text or the dictation
//! exactly as extracted — never partially cleaned, never trimmed. Only
//! metadata leaves this module: dictated text is never logged here (event
//! logging arrives in S1.4 on top of these outcomes).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio::time::Instant;

use crate::cleanup::cleanup;
use crate::config::{Config, PromptSettings};
use crate::prompts::compose_with_settings;
use crate::providers::{FormatInput, Provider, ProviderError};
use crate::request::ExtractedRequest;

/// Part of the total budget reserved for killing the CLI and building the
/// response; a run never starts when only the reserve is left.
const RESPONSE_RESERVE: Duration = Duration::from_millis(250);

/// A run refuses to start with less than this slice of budget left: too
/// little time remains to spawn a CLI and get a useful result.
const MIN_STARTUP: Duration = Duration::from_millis(100);

/// Grace between a provider's deadline and the hard stop that cancels it.
/// The process runner enforces the deadline itself; this slack only backs up
/// a provider that ignores it (dropping its future kills the process tree).
/// It stays below `RESPONSE_RESERVE`, so even then the response goes out
/// before the total timeout and Handy still receives the raw text.
const HARD_STOP_SLACK: Duration = Duration::from_millis(150);

/// A provider ready to run and its configured timeout.
struct ProviderEntry {
    provider: Arc<dyn Provider>,
    timeout: Duration,
}

/// Selects a provider, runs it inside the request's budget and cleans the
/// result; returns the raw dictation with the reason whenever formatting
/// cannot deliver.
///
/// One formatting run is active at a time: a concurrent request receives the
/// raw dictation immediately ([`RawReason::Busy`]). The busy permit and the
/// provider future both release on cancellation, killing the CLI's process
/// tree.
pub struct Pipeline {
    /// The built (enabled) providers, by ID.
    providers: BTreeMap<String, ProviderEntry>,
    /// Every provider ID the configuration knows about, enabled or not: IDs
    /// outside this set are [`RawReason::UnknownProvider`]; configured ones
    /// missing from `providers` are [`RawReason::ProviderDisabled`].
    configured: BTreeSet<String>,
    default_provider: String,
    /// Providers to try after the selected one fails, in configuration
    /// order.
    fallback_order: Vec<String>,
    total_timeout: Duration,
    prompt_settings: PromptSettings,
    busy: Semaphore,
}

impl Pipeline {
    pub fn new(config: &Config, providers: Vec<Arc<dyn Provider>>) -> Pipeline {
        let mut entries = BTreeMap::new();
        for provider in providers {
            // A built provider with no configured entry (not possible through
            // `build_from_config`) would run under the total budget alone.
            let timeout = config
                .providers
                .get(provider.id())
                .map_or(config.total_timeout, |settings| settings.timeout);
            entries.insert(
                provider.id().to_owned(),
                ProviderEntry { provider, timeout },
            );
        }
        Pipeline {
            providers: entries,
            configured: config.providers.keys().cloned().collect(),
            default_provider: config.default_provider.clone(),
            fallback_order: config.fallback_order.clone(),
            total_timeout: config.total_timeout,
            prompt_settings: config.prompts.clone(),
            busy: Semaphore::new(1),
        }
    }

    /// Formats one extracted request. `started` is when the HTTP request
    /// arrived; the total budget counts from it.
    ///
    /// Outcome rule: the first candidate whose output cleans successfully
    /// wins ([`OutcomeKind::Formatted`]; `attempts` counts the providers
    /// tried, starting at 1). A provider error — including a timeout — or
    /// a cleanup failure moves to the next candidate in `fallback_order`.
    /// When no candidate is left, the raw dictation returns with the last
    /// failure ([`RawReason::ProviderFailed`] or [`RawReason::CleanupFailed`]);
    /// when the budget dies before the first candidate starts, the reason
    /// is [`RawReason::BudgetExhausted`] instead. A budget stop later in
    /// the chain keeps the last failure.
    pub async fn format(&self, request: &ExtractedRequest, started: Instant) -> FormatOutcome {
        if request.is_empty() {
            return FormatOutcome::finished(String::new(), OutcomeKind::Empty, None, 0, started);
        }

        let selected = request.model.as_deref().unwrap_or(&self.default_provider);
        // An unknown or disabled selected provider is a configuration
        // mistake, not a transient failure: return raw text at once so the
        // user notices, never silently formatting through another provider.
        if !self.configured.contains(selected) {
            return self.raw(request, RawReason::UnknownProvider, started, 0);
        }
        if !self.providers.contains_key(selected) {
            return self.raw(request, RawReason::ProviderDisabled, started, 0);
        }

        // Held for the whole run; dropping it (including on cancellation)
        // releases the permit.
        let Ok(_permit) = self.busy.try_acquire() else {
            return self.raw(request, RawReason::Busy, started, 0);
        };

        let Some(total_deadline) = started.checked_add(self.total_timeout) else {
            return self.raw(request, RawReason::BudgetExhausted, started, 0);
        };
        let total_deadline = total_deadline
            .checked_sub(RESPONSE_RESERVE)
            .unwrap_or(started);
        if total_deadline.duration_since(Instant::now()) < MIN_STARTUP {
            return self.raw(request, RawReason::BudgetExhausted, started, 0);
        }

        let prompts = compose_with_settings(request, &self.prompt_settings);
        let candidates = self.candidates(selected);

        let mut attempts: u8 = 0;
        let mut last_failure = None;
        for candidate in candidates.iter().copied() {
            // The whole chain shares one budget: never start a CLI when
            // only the startup slice is left.
            if total_deadline.duration_since(Instant::now()) < MIN_STARTUP {
                break;
            }
            attempts = attempts.saturating_add(1);
            match run_within_budget(candidate, prompts.format_input(), total_deadline).await {
                Ok(output) => match cleanup(&output, &request.raw_text) {
                    Ok(text) => {
                        return FormatOutcome::finished(
                            text,
                            OutcomeKind::Formatted,
                            Some(candidate.provider.id()),
                            attempts,
                            started,
                        );
                    }
                    Err(_) => last_failure = Some(ChainFailure::Cleanup),
                },
                Err(kind) => last_failure = Some(ChainFailure::Provider(kind)),
            }
        }

        let reason = match last_failure {
            Some(ChainFailure::Provider(kind)) => RawReason::ProviderFailed(kind),
            Some(ChainFailure::Cleanup) => RawReason::CleanupFailed,
            // The budget died before the first candidate started; the
            // pre-loop check normally returns earlier, so `attempts` is 0.
            None => RawReason::BudgetExhausted,
        };
        self.raw(request, reason, started, attempts)
    }

    /// Builds the fallback chain for `selected`: the selected provider
    /// first, then every `fallback_order` entry in order. A reappearing
    /// selected provider, duplicates, and providers that are disabled or
    /// were never built are skipped, so no provider runs twice.
    fn candidates(&self, selected: &str) -> Vec<&ProviderEntry> {
        let mut chain = Vec::with_capacity(self.fallback_order.len() + 1);
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        if let Some(entry) = self.providers.get(selected) {
            seen.insert(selected);
            chain.push(entry);
        }
        for id in &self.fallback_order {
            if !seen.insert(id) {
                continue;
            }
            // A configured provider missing from `providers` is disabled;
            // `fallback_order` validation already rejects unknown ids.
            if let Some(entry) = self.providers.get(id) {
                chain.push(entry);
            }
        }
        chain
    }

    /// The raw fallback: the dictation exactly as extracted, never cleaned
    /// or trimmed.
    fn raw(
        &self,
        request: &ExtractedRequest,
        reason: RawReason,
        started: Instant,
        attempts: u8,
    ) -> FormatOutcome {
        FormatOutcome::finished(
            request.raw_text.clone(),
            OutcomeKind::Raw(reason),
            None,
            attempts,
            started,
        )
    }
}

/// The last failure while walking the fallback chain; decides the raw
/// outcome when no candidate formatted successfully.
enum ChainFailure {
    Provider(ProviderErrorKind),
    Cleanup,
}

/// Runs one candidate: its own timeout capped by the total deadline, plus a
/// hard stop shortly after that deadline in case the provider ignores it.
async fn run_within_budget(
    entry: &ProviderEntry,
    input: FormatInput<'_>,
    total_deadline: Instant,
) -> Result<String, ProviderErrorKind> {
    let deadline = Instant::now()
        .checked_add(entry.timeout)
        .map_or(total_deadline, |own| own.min(total_deadline));
    let hard_stop = deadline.checked_add(HARD_STOP_SLACK).unwrap_or(deadline);
    match tokio::time::timeout_at(hard_stop, entry.provider.format(input, deadline)).await {
        Ok(result) => result,
        Err(_) => Err(ProviderError::Timeout),
    }
}

/// What happened to one dictation: the text to send back and safe metadata.
pub struct FormatOutcome {
    /// Formatted and cleaned text, or the raw dictation exactly as
    /// extracted.
    pub text: String,
    pub kind: OutcomeKind,
    /// The provider that produced `text`; `None` for an empty transcript
    /// and every raw outcome.
    pub provider: Option<&'static str>,
    /// Providers tried for this request: 1 when the first succeeds, more
    /// after fallbacks, 0 when no CLI was started (selection errors, busy,
    /// an exhausted budget).
    pub attempts: u8,
    /// Time since the HTTP request arrived.
    pub elapsed: Duration,
}

// Manual impl: `text` may be the raw dictation, which must never reach logs
// through `{:?}`.
impl fmt::Debug for FormatOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FormatOutcome")
            .field("text_len", &self.text.len())
            .field("kind", &self.kind)
            .field("provider", &self.provider)
            .field("attempts", &self.attempts)
            .field("elapsed", &self.elapsed)
            .finish()
    }
}

impl FormatOutcome {
    fn finished(
        text: String,
        kind: OutcomeKind,
        provider: Option<&'static str>,
        attempts: u8,
        started: Instant,
    ) -> FormatOutcome {
        FormatOutcome {
            text,
            kind,
            provider,
            attempts,
            elapsed: started.elapsed(),
        }
    }
}

/// The kind of a [`FormatOutcome`]; safe to log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutcomeKind {
    /// Formatted and cleaned by `provider`.
    Formatted,
    /// The transcript held no text; no CLI was invoked.
    Empty,
    /// The raw dictation is returned; the reason says why.
    Raw(RawReason),
}

/// Why the raw dictation is returned instead of formatted text; safe to log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawReason {
    /// The requested provider ID is not in the configuration.
    UnknownProvider,
    /// The requested provider is configured but disabled.
    ProviderDisabled,
    /// Another formatting run is active.
    Busy,
    /// The request's total budget is (nearly) spent before starting.
    BudgetExhausted,
    /// Every candidate failed, timed out or returned unusable output;
    /// carries the last failure.
    ProviderFailed(ProviderErrorKind),
    /// Output cleanup refused the provider's text.
    CleanupFailed,
}

/// Text-free summary of a provider failure. [`ProviderError`] already
/// carries no captured output or dictated text, so the kind is the error.
pub type ProviderErrorKind = ProviderError;

// The hard stop must fire inside the reserved part of the total budget.
const _: () = assert!(HARD_STOP_SLACK.as_millis() < RESPONSE_RESERVE.as_millis());
