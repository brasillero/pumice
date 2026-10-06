//! The formatting pipeline: provider selection and the fallback chain,
//! deadline enforcement, one active run at a time, and the raw-text
//! fallback guarantee.
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

use crate::cleanup::cleanup;
use crate::config::{Config, PromptSettings};
use crate::prompts::compose_with_settings;
use crate::providers::diagnostic::{self, Diagnostic};
use crate::providers::discovery::{Found, ProviderStatus};
use crate::providers::{self, FormatInput, Provider, ProviderError};
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
    /// Provider IDs startup detection confirmed installed. `None` when the
    /// pipeline was built without detection (the Phase 1 behavior): then
    /// every built provider counts as available. The generic loopback
    /// adapter has no executable to find and counts as available whenever it
    /// is built. Selection and fallback ignore this; only model listing
    /// filters by it.
    available: Option<BTreeSet<String>>,
    busy: Semaphore,
}

impl Pipeline {
    pub fn new(config: &Config, providers: Vec<Arc<dyn Provider>>) -> Pipeline {
        Pipeline::build(config, providers, None)
    }

    /// [`new`](Self::new) with startup detection cached in the pipeline:
    /// [`model_ids`](Self::model_ids) lists only built providers detection
    /// found. An enabled provider detection missed stays built and
    /// selectable — it fails at runtime with `NotInstalled` and the fallback
    /// chain runs, exactly as before detection existed.
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
            let settings = config.providers.get(provider.id());
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
            configured: config.providers.keys().cloned().collect(),
            default_provider: config.default_provider.clone(),
            fallback_order: config.fallback_order.clone(),
            total_timeout: config.total_timeout,
            prompt_settings: config.prompts.clone(),
            available,
            busy: Semaphore::new(1),
        }
    }

    /// The provider IDs offered as models on `GET /v1/models`: the default
    /// provider first, then the remaining enabled providers in registry
    /// order, so a client that picks the first model gets the default. With
    /// detection cached, only providers confirmed installed are listed.
    /// The built-in `passthrough` and `inspect` models are always listed last.
    /// Never invokes a CLI.
    pub fn model_ids(&self) -> Vec<&str> {
        let mut ids = Vec::with_capacity(self.providers.len());
        let default = self.default_provider.as_str();
        if self.providers.contains_key(default) && self.is_available(default) {
            ids.push(default);
        }
        for descriptor in providers::PROVIDERS {
            if descriptor.id != default
                && self.providers.contains_key(descriptor.id)
                && self.is_available(descriptor.id)
            {
                ids.push(descriptor.id);
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
    /// `claude`. An absent or empty value selects the default provider.
    /// `passthrough` selects the built-in unmodified transcript response.
    /// Returns the canonical configured ID — including a disabled one, which
    /// `format` reports as [`RawReason::ProviderDisabled`] — or `None` when
    /// the value names no configured provider.
    pub fn select(&self, requested: Option<&str>) -> Option<&str> {
        match requested.map(str::trim) {
            None | Some("") => Some(self.default_provider.as_str()),
            Some(requested) if requested.eq_ignore_ascii_case("passthrough") => Some("passthrough"),
            Some(requested) if requested.eq_ignore_ascii_case("inspect") => Some("inspect"),
            Some(requested) => self
                .configured
                .iter()
                .find(|id| id.eq_ignore_ascii_case(requested))
                .map(String::as_str),
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
        // Explicit passthrough preserves even whitespace-only transcripts and
        // bypasses prompts, cleanup, the busy guard and the fallback chain.
        if self.select(request.model.as_deref()) == Some("passthrough") {
            return FormatOutcome::finished(
                request.raw_text.clone(),
                OutcomeKind::Passthrough,
                None,
                Vec::new(),
                started,
            );
        }
        if request.is_empty() {
            return FormatOutcome::finished(
                String::new(),
                OutcomeKind::Empty,
                None,
                Vec::new(),
                started,
            );
        }

        // An unknown or disabled selected provider is a configuration
        // mistake, not a transient failure: return raw text at once so the
        // user notices, never silently formatting through another provider.
        let Some(selected) = self.select(request.model.as_deref()) else {
            return self.raw(request, RawReason::UnknownProvider, started, Vec::new());
        };
        if !self.providers.contains_key(selected) {
            return self.raw(request, RawReason::ProviderDisabled, started, Vec::new());
        }

        // Held for the whole run; dropping it (including on cancellation)
        // releases the permit.
        let Ok(_permit) = self.busy.try_acquire() else {
            return self.raw(request, RawReason::Busy, started, Vec::new());
        };

        let Some(total_deadline) = started.checked_add(self.total_timeout) else {
            return self.raw(request, RawReason::BudgetExhausted, started, Vec::new());
        };
        let total_deadline = total_deadline
            .checked_sub(RESPONSE_RESERVE)
            .unwrap_or(started);
        if total_deadline.duration_since(Instant::now()) < MIN_STARTUP {
            return self.raw(request, RawReason::BudgetExhausted, started, Vec::new());
        }

        let prompts = compose_with_settings(request, &self.prompt_settings);
        let candidates = self.candidates(selected);

        let mut trail: Vec<Attempt> = Vec::with_capacity(candidates.len());
        let mut last_failure = None;
        for candidate in candidates.iter().copied() {
            // The whole chain shares one budget: never start a CLI when
            // only the startup slice is left.
            if total_deadline.duration_since(Instant::now()) < MIN_STARTUP {
                break;
            }
            let attempt_started = Instant::now();
            let (run, diagnostic) = diagnostic::capture(run_within_budget(
                candidate,
                prompts.format_input(),
                total_deadline,
            ))
            .await;
            let result = match run {
                Ok(output) => match cleanup(&output, &request.raw_text) {
                    Ok(text) => Ok(text),
                    Err(_) => Err(ChainFailure::Cleanup),
                },
                Err(kind) => Err(ChainFailure::Provider(kind)),
            };
            trail.push(Attempt {
                provider: candidate.provider.id(),
                model: candidate.model.clone(),
                result: match &result {
                    Ok(_) => AttemptResult::Formatted,
                    Err(ChainFailure::Provider(kind)) => AttemptResult::Failed(*kind),
                    Err(ChainFailure::Cleanup) => AttemptResult::CleanupRejected,
                },
                elapsed: attempt_started.elapsed(),
                diagnostic,
            });
            match result {
                Ok(text) => {
                    return FormatOutcome::finished(
                        text,
                        OutcomeKind::Formatted,
                        Some(candidate.provider.id()),
                        trail,
                        started,
                    );
                }
                Err(failure) => last_failure = Some(failure),
            }
        }

        let reason = match last_failure {
            Some(ChainFailure::Provider(kind)) => RawReason::ProviderFailed(kind),
            Some(ChainFailure::Cleanup) => RawReason::CleanupFailed,
            // The budget died before the first candidate started; the
            // pre-loop check normally returns earlier, so the trail is empty.
            None => RawReason::BudgetExhausted,
        };
        self.raw(request, reason, started, trail)
    }

    /// The chain a request without a model runs: `(provider, model)` for the
    /// default provider, then its fallbacks. Shown once at startup.
    pub fn default_route(&self) -> Vec<(&'static str, &str)> {
        self.candidates(&self.default_provider)
            .into_iter()
            .map(|entry| (entry.provider.id(), entry.model.as_str()))
            .collect()
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
    /// Every provider started for this request, in order; empty when no
    /// CLI was started.
    pub trail: Vec<Attempt>,
    /// Time since the HTTP request arrived.
    pub elapsed: Duration,
}

/// One provider run inside a request. Safe to log except
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

/// How one [`Attempt`] ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttemptResult {
    Formatted,
    Failed(ProviderErrorKind),
    /// The provider answered, but output cleanup refused the text.
    CleanupRejected,
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
