//! The formatting pipeline: provider selection, deadline enforcement, up to
//! `max_parallel` runs at once with a FIFO queue, and the raw-text guarantee.
//!
//! There is no fallback (owner decision 2026-10-07): a request runs exactly
//! one provider — the one its `model` names. A request without a model, like
//! every failure, behaves exactly like passthrough: the dictation comes back
//! exactly as extracted, with the reason in the log.
//!
//! Every outcome returns either formatted-and-cleaned text or the dictation
//! exactly as extracted — never partially cleaned, never trimmed. Only
//! metadata leaves this module: dictated text is never logged here; the S1.4
//! debug log consumes these outcomes in the API layer.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio::time::Instant;

use crate::cleanup::{CleanupError, cleanup};
use crate::config::Config;
use crate::prompts::compose_prompts;
use crate::providers::diagnostic::{self, Diagnostic};
use crate::providers::discovery::{Found, ProviderStatus};
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

/// A provider ready to run, its configured timeout and model (the model is
/// only shown in logs).
struct ProviderEntry {
    provider: Arc<dyn Provider>,
    timeout: Duration,
    model: String,
}

/// Selects the single provider a request runs, runs it inside the request's
/// budget and cleans the result; returns the raw dictation with the reason
/// whenever formatting cannot deliver.
///
/// Up to `max_parallel` formatting runs are active at a time; extra requests
/// wait in line (FIFO). A request still waiting when its total budget runs out
/// gets [`RawReason::Busy`]. The busy permit and the provider future both
/// release on cancellation, killing the CLI's process tree.
pub struct Pipeline {
    /// The built (enabled) providers, by ID.
    providers: BTreeMap<String, ProviderEntry>,
    /// Every provider ID the configuration knows about, enabled or not: IDs
    /// outside this set are [`RawReason::UnknownProvider`]; configured ones
    /// missing from `providers` are [`RawReason::ProviderDisabled`].
    configured: BTreeSet<String>,
    /// Enabled provider IDs in configuration (list) order.
    order: Vec<String>,
    total_timeout: Duration,
    /// Provider IDs startup detection confirmed installed. `None` when the
    /// pipeline was built without detection (the Phase 1 behavior): then
    /// every built provider counts as available. The generic loopback
    /// adapter has no executable to find and counts as available whenever it
    /// is built. Selection ignores this; only model listing filters by it.
    available: Option<BTreeSet<String>>,
    /// Limits how many provider calls run at once; extra requests wait in line.
    busy: Semaphore,
}

impl Pipeline {
    pub fn new(config: &Config, providers: Vec<Arc<dyn Provider>>) -> Pipeline {
        Pipeline::build(config, providers, None)
    }

    /// [`new`](Self::new) with startup detection cached in the pipeline:
    /// [`model_ids`](Self::model_ids) lists only built providers detection
    /// found. An enabled provider detection missed stays built and
    /// selectable — it fails at runtime with `NotInstalled` and the raw
    /// dictation comes back, exactly as before detection existed.
    pub fn with_detection(
        config: &Config,
        providers: Vec<Arc<dyn Provider>>,
        detection: Vec<ProviderStatus>,
    ) -> Pipeline {
        let available = detection
            .iter()
            .filter(|status| matches!(status.found, Found::Found(_) | Found::NotApplicable))
            .map(|status| status.id.to_owned())
            .collect();
        Pipeline::build(config, providers, Some(available))
    }

    fn build(
        config: &Config,
        providers: Vec<Arc<dyn Provider>>,
        available: Option<BTreeSet<String>>,
    ) -> Pipeline {
        let mut entries = BTreeMap::new();
        for provider in providers {
            // A built provider with no configured entry (not possible through
            // `build_from_config`) would run under the total budget alone.
            let settings = config.provider(provider.id());
            let timeout = settings.map_or(config.total_timeout, |settings| settings.timeout);
            let model = settings.map_or_else(String::new, |settings| settings.model.clone());
            entries.insert(
                provider.id().to_owned(),
                ProviderEntry {
                    provider,
                    timeout,
                    model,
                },
            );
        }
        Pipeline {
            providers: entries,
            configured: config
                .providers
                .iter()
                .map(|entry| entry.id.clone())
                .collect(),
            order: config
                .providers
                .iter()
                .filter(|entry| entry.settings.enabled)
                .map(|entry| entry.id.clone())
                .collect(),
            total_timeout: config.total_timeout,
            available,
            busy: Semaphore::new(config.max_parallel),
        }
    }

    /// The provider IDs offered as models on `GET /v1/models`: the enabled
    /// providers in configuration (list) order. With detection cached, only
    /// providers confirmed installed are listed. The built-in `passthrough`
    /// and `inspect` models are always listed last. Never invokes a CLI.
    pub fn model_ids(&self) -> Vec<&str> {
        let mut ids = Vec::with_capacity(self.providers.len() + 2);
        for id in &self.order {
            if self.providers.contains_key(id) && self.is_available(id) {
                ids.push(id.as_str());
            }
        }
        ids.push("passthrough");
        ids.push("inspect");
        ids
    }

    /// Whether detection confirmed `id` installed; without detection every
    /// built provider counts.
    fn is_available(&self, id: &str) -> bool {
        self.available
            .as_ref()
            .is_none_or(|available| available.contains(id))
    }

    /// Selection rule for the request's `model` field: the value is trimmed
    /// and matched case-insensitively against the configured provider IDs
    /// (Handy users type the field by hand), so `"Claude "` selects
    /// `claude`. A missing or blank value selects nothing — the request
    /// returns the original text with [`RawReason::NoModel`].
    /// `passthrough` selects the built-in unmodified transcript response.
    /// Returns the canonical configured ID — including a disabled one, which
    /// `format` reports as [`RawReason::ProviderDisabled`] — or `None` when
    /// the value names no configured provider (or names none at all).
    pub fn select(&self, requested: Option<&str>) -> Option<&str> {
        match requested.map(str::trim) {
            None | Some("") => None,
            Some(requested) if requested.eq_ignore_ascii_case("passthrough") => Some("passthrough"),
            Some(requested) if requested.eq_ignore_ascii_case("inspect") => Some("inspect"),
            Some(requested) => self
                .configured
                .iter()
                .find(|id| id.eq_ignore_ascii_case(requested))
                .map(String::as_str),
        }
    }

    /// Every enabled provider in configuration (list) order, as
    /// `(id, model)` pairs; shown once at startup.
    pub fn enabled_route(&self) -> Vec<(&'static str, &str)> {
        self.order
            .iter()
            .filter_map(|id| self.providers.get(id))
            .map(|entry| (entry.provider.id(), entry.model.as_str()))
            .collect()
    }

    /// Formats one extracted request. `started` is when the HTTP request
    /// arrived; the total budget counts from it.
    ///
    /// Outcome rule: exactly one provider runs — the selected one. On
    /// success the cleaned text returns ([`OutcomeKind::Formatted`]);
    /// a provider error — including a timeout — or a cleanup failure returns
    /// the raw dictation with [`RawReason::ProviderFailed`] or
    /// [`RawReason::CleanupFailed`]. When the budget dies before the attempt
    /// starts, the reason is [`RawReason::BudgetExhausted`] instead.
    pub async fn format(&self, request: &ExtractedRequest, started: Instant) -> FormatOutcome {
        // Explicit passthrough preserves even whitespace-only transcripts and
        // bypasses prompts, cleanup, the busy guard and provider selection.
        if self.select(request.model.as_deref()) == Some("passthrough") {
            return FormatOutcome::finished(
                request.raw_text.clone(),
                OutcomeKind::Passthrough,
                None,
                Vec::new(),
                started,
            );
        }
        // An unknown or disabled selected provider is a configuration
        // mistake, not a transient failure: return raw text at once so the
        // user notices, never silently formatting through another provider.
        let Some(selected) = self.select(request.model.as_deref()) else {
            let reason = match request.model.as_deref().map(str::trim) {
                None | Some("") => RawReason::NoModel,
                Some(_) => RawReason::UnknownProvider,
            };
            return self.raw(request, reason, started, Vec::new());
        };
        let Some(candidate) = self.providers.get(selected) else {
            return self.raw(request, RawReason::ProviderDisabled, started, Vec::new());
        };

        // Nothing to format: the original text (empty or whitespace only)
        // returns byte for byte, without running the provider. Checked after
        // selection so a wrong model still shows its own reason in the log.
        if request.is_empty() {
            return FormatOutcome::finished(
                request.raw_text.clone(),
                OutcomeKind::Empty,
                None,
                Vec::new(),
                started,
            );
        }

        let Some(total_deadline) = started.checked_add(self.total_timeout) else {
            return self.raw(request, RawReason::BudgetExhausted, started, Vec::new());
        };
        let total_deadline = total_deadline
            .checked_sub(RESPONSE_RESERVE)
            .unwrap_or(started);
        if total_deadline.duration_since(Instant::now()) < MIN_STARTUP {
            return self.raw(request, RawReason::BudgetExhausted, started, Vec::new());
        }

        // Waits in line (first come, first served) for a free slot until the
        // budget runs out. Held for the whole run; dropping it (including on
        // cancellation) releases the slot.
        let _permit = match tokio::time::timeout_at(total_deadline, self.busy.acquire()).await {
            Ok(Ok(permit)) => permit,
            Ok(Err(_)) | Err(_) => return self.raw(request, RawReason::Busy, started, Vec::new()),
        };
        // The budget was enough before waiting, so any shortfall now (even a
        // slot that freed just past the deadline) is the queue's: busy.
        if total_deadline.duration_since(Instant::now()) < MIN_STARTUP {
            return self.raw(request, RawReason::Busy, started, Vec::new());
        }

        let prompts = compose_prompts(request);
        let attempt_started = Instant::now();
        let (run, diagnostic) = diagnostic::capture(run_within_budget(
            candidate,
            prompts.format_input(),
            total_deadline,
        ))
        .await;
        let result = match run {
            Ok(output) => match cleanup(&output) {
                Ok(text) => Ok(text),
                Err(error) => Err(ChainFailure::Cleanup(error)),
            },
            Err(kind) => Err(ChainFailure::Provider(kind)),
        };
        let trail = vec![Attempt {
            provider: candidate.provider.id(),
            model: candidate.model.clone(),
            result: match &result {
                Ok(_) => AttemptResult::Formatted,
                Err(ChainFailure::Provider(kind)) => AttemptResult::Failed(*kind),
                Err(ChainFailure::Cleanup(error)) => AttemptResult::CleanupRejected(*error),
            },
            elapsed: attempt_started.elapsed(),
            diagnostic,
        }];
        match result {
            Ok(text) => FormatOutcome::finished(
                text,
                OutcomeKind::Formatted,
                Some(candidate.provider.id()),
                trail,
                started,
            ),
            Err(ChainFailure::Provider(kind)) => {
                self.raw(request, RawReason::ProviderFailed(kind), started, trail)
            }
            Err(ChainFailure::Cleanup(error)) => {
                self.raw(request, RawReason::CleanupFailed(error), started, trail)
            }
        }
    }

    /// The raw fallback: the dictation exactly as extracted, never cleaned
    /// or trimmed.
    fn raw(
        &self,
        request: &ExtractedRequest,
        reason: RawReason,
        started: Instant,
        trail: Vec<Attempt>,
    ) -> FormatOutcome {
        FormatOutcome::finished(
            request.raw_text.clone(),
            OutcomeKind::Raw(reason),
            None,
            trail,
            started,
        )
    }
}

/// A formatting attempt that did not produce usable text; decides the raw
/// outcome.
enum ChainFailure {
    Provider(ProviderErrorKind),
    Cleanup(CleanupError),
}

/// Runs one provider: its own timeout capped by the total deadline, plus a
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
    /// 1 when the provider ran, 0 when no CLI was started (selection
    /// errors, busy, an exhausted budget, nothing enabled).
    pub attempts: u8,
    /// The provider started for this request; empty when no CLI was started.
    pub trail: Vec<Attempt>,
    /// Time since the HTTP request arrived.
    pub elapsed: Duration,
}

/// The single provider run inside a request. Safe to log except
/// `diagnostic.detail`, which belongs in the debug log only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attempt {
    pub provider: &'static str,
    /// The configured model, as written in the configuration.
    pub model: String,
    pub result: AttemptResult,
    pub elapsed: Duration,
    /// What the CLI said when it failed; only its safe summary reaches the
    /// ordinary log.
    pub diagnostic: Option<Diagnostic>,
}

/// How the single [`Attempt`] ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttemptResult {
    Formatted,
    Failed(ProviderErrorKind),
    /// The provider answered, but output cleanup refused the text.
    CleanupRejected(CleanupError),
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
            .field("trail", &self.trail)
            .field("elapsed", &self.elapsed)
            .finish()
    }
}

impl FormatOutcome {
    fn finished(
        text: String,
        kind: OutcomeKind,
        provider: Option<&'static str>,
        trail: Vec<Attempt>,
        started: Instant,
    ) -> FormatOutcome {
        FormatOutcome {
            text,
            kind,
            provider,
            attempts: u8::try_from(trail.len()).unwrap_or(u8::MAX),
            trail,
            elapsed: started.elapsed(),
        }
    }
}

/// The kind of a [`FormatOutcome`]; safe to log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutcomeKind {
    /// Formatted and cleaned by `provider`.
    Formatted,
    /// Original transcript returned by explicit request, with no provider call.
    Passthrough,
    /// The full request body returned by explicit `inspect` selection, with no
    /// provider call and no transcript extraction.
    Inspect,
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
    /// The request named no provider: the `model` field was missing or
    /// blank, so the original text returns.
    NoModel,
    /// Every slot was taken and the request waited until its budget ran out
    /// (or until too little was left to start).
    Busy,
    /// The request's total budget is (nearly) spent before starting.
    BudgetExhausted,
    /// The selected provider failed, timed out or returned unusable output.
    ProviderFailed(ProviderErrorKind),
    /// Output cleanup refused the provider's text, for this reason.
    CleanupFailed(CleanupError),
}

/// Text-free summary of a provider failure. [`ProviderError`] already
/// carries no captured output or dictated text, so the kind is the error.
pub type ProviderErrorKind = ProviderError;

// The hard stop must fire inside the reserved part of the total budget.
const _: () = assert!(HARD_STOP_SLACK.as_millis() < RESPONSE_RESERVE.as_millis());
