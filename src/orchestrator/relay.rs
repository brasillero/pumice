//! The relay stage of `pumice run`: the relay runs in-process from its
//! library (arch-modules/D13) — its own config loader, provider builders and
//! pipeline, never an HTTP server. The request is constructed directly from
//! the raw transcript (p12-pumice-run/D3): no `<transcript>` envelope
//! extraction, so dictated tags stay data.

use std::path::Path;
use std::sync::Arc;

use tokio::time::Instant;

use pumice_relay::config;
use pumice_relay::pipeline::{FormatOutcome, OutcomeKind, Pipeline, RawReason};
use pumice_relay::process::{CleanupScope, ProcessRunner};
use pumice_relay::providers::{self, ProviderError, ProviderErrorCode};
use pumice_relay::request::ExtractedRequest;

use crate::orchestrator::error::StageError;
use crate::orchestrator::result::{ErrorLocation, stage};

/// Loads the relay configuration and builds its providers and pipeline. Runs
/// no detection and starts no listener; provider CLIs are only resolved when
/// a call actually runs.
pub fn build_pipeline(
    config_path: &Path,
    scope: Arc<CleanupScope>,
) -> Result<Pipeline, StageError> {
    let loaded = config::load(Some(config_path)).map_err(|error| {
        let message = error.to_string();
        let mut stage_error = StageError::new(stage::PREPARE, "invalid_config", message, 2);
        if let Some(file) = error.path() {
            stage_error.set_location(ErrorLocation {
                file: file.to_path_buf(),
                line: error.line(),
                column: error.column(),
            });
        }
        stage_error
    })?;
    let runner = Arc::new(ProcessRunner::new().with_cleanup_scope(scope));
    let built = providers::build_from_config(&loaded.config, runner).map_err(|error| {
        StageError::new(
            stage::PREPARE,
            "invalid_config",
            format!("relay configuration: {error}"),
            2,
        )
    })?;
    Ok(Pipeline::new(&loaded.config, built))
}

/// True when `route` selects an enabled provider or the built-in passthrough
/// route. `inspect` is refused by the orchestrator configuration already.
/// Without startup detection every built provider counts as available.
pub fn route_enabled(pipeline: &Pipeline, route: &str) -> bool {
    pipeline
        .model_ids()
        .iter()
        .any(|id| id.eq_ignore_ascii_case(route))
}

/// The relay request for one flow: the raw transcript as the user message,
/// the formatter's rules (when one is selected) as the system prompt.
pub fn build_request(
    route: &str,
    raw_text: &str,
    formatter_system: Option<String>,
) -> ExtractedRequest {
    ExtractedRequest {
        model: Some(route.to_owned()),
        system_texts: formatter_system.into_iter().collect(),
        before_text: String::new(),
        text: raw_text.to_owned(),
        after_text: String::new(),
        raw_text: raw_text.to_owned(),
    }
}

/// Maps the pipeline outcome to the flow result or a relay-stage error.
/// `now` and `work_deadline` classify a spent overall flow budget as
/// `flow_timeout` rather than a relay timeout (p12-pumice-run/D2);
/// `flow_owned_deadline` is whether the deadline handed to the pipeline was
/// the flow's work deadline, so a timeout the pipeline reports as bound by
/// its caller ([`FormatOutcome::caller_deadline_bound`]) maps to
/// `flow_timeout` even though the pipeline's response reserve makes it fire
/// shortly before `work_deadline`.
pub fn map_outcome(
    outcome: FormatOutcome,
    now: Instant,
    work_deadline: Instant,
    flow_owned_deadline: bool,
) -> Result<String, StageError> {
    match outcome.kind {
        OutcomeKind::Formatted | OutcomeKind::Passthrough | OutcomeKind::Empty => Ok(outcome.text),
        OutcomeKind::Raw(RawReason::NoModel)
        | OutcomeKind::Raw(RawReason::UnknownProvider)
        | OutcomeKind::Raw(RawReason::ProviderDisabled) => Err(StageError::new(
            stage::RELAY,
            "relay_route_invalid",
            "the flow's relay route is not an enabled provider in the relay configuration",
            2,
        )),
        OutcomeKind::Raw(RawReason::Busy) => Err(StageError::new(
            stage::RELAY,
            "relay_failed",
            "the relay had no free slot for the provider call",
            1,
        )),
        OutcomeKind::Raw(RawReason::BudgetExhausted)
        | OutcomeKind::Raw(RawReason::ProviderFailed(ProviderError::Timeout)) => {
            Err(timeout_error(
                now,
                work_deadline,
                outcome.caller_deadline_bound && flow_owned_deadline,
            ))
        }
        OutcomeKind::Raw(RawReason::ProviderFailed(kind)) => Err(provider_error(kind)),
        OutcomeKind::Raw(RawReason::CleanupFailed(_)) => Err(StageError::new(
            stage::RELAY,
            "relay_invalid_output",
            "the provider returned an unusable reply",
            1,
        )),
        // `run` never selects the inspect route (rejected in the config).
        OutcomeKind::Inspect => Err(StageError::new(
            stage::RELAY,
            "internal",
            "unexpected inspect outcome",
            1,
        )),
    }
}

/// The deadline classification shared by every stage: a spent overall flow
/// budget is `flow_timeout`, otherwise the stage's own timeout.
pub fn timeout_error_at(
    stage: &'static str,
    stage_code: &'static str,
    stage_message: &'static str,
    now: Instant,
    work_deadline: Instant,
) -> StageError {
    if now >= work_deadline {
        StageError::new(
            stage,
            "flow_timeout",
            "the flow's total budget is exhausted",
            1,
        )
    } else {
        StageError::new(stage, stage_code, stage_message, 1)
    }
}

/// A relay-stage timeout: `flow_timeout` when the overall flow budget was
/// the binding limit — either already spent now, or reported by the pipeline
/// as the caller-bound deadline it enforced (which fires one response
/// reserve before `work_deadline`) — and `relay_timeout` otherwise.
fn timeout_error(now: Instant, work_deadline: Instant, caller_bound_flow: bool) -> StageError {
    if caller_bound_flow {
        return StageError::new(
            stage::RELAY,
            "flow_timeout",
            "the flow's total budget is exhausted",
            1,
        );
    }
    timeout_error_at(
        stage::RELAY,
        "relay_timeout",
        "the provider call exceeded its deadline",
        now,
        work_deadline,
    )
}

fn provider_error(kind: ProviderError) -> StageError {
    let (code, message) = match kind {
        ProviderError::NotInstalled => {
            ("relay_not_installed", "the provider's CLI is not installed")
        }
        ProviderError::NotLoggedIn => ("relay_not_logged_in", "the provider is not logged in"),
        ProviderError::Timeout => unreachable!("handled by the caller"),
        ProviderError::QuotaExceeded { .. } => (
            "relay_quota_exceeded",
            "the provider usage quota is exhausted",
        ),
        ProviderError::RateLimited { .. } => ("relay_rate_limited", "the provider is rate limited"),
        ProviderError::Other {
            code: ProviderErrorCode::InvalidOutput,
        } => (
            "relay_invalid_output",
            "the provider returned output that could not be parsed",
        ),
        ProviderError::Other { code } => ("relay_failed", provider_code_message(code)),
    };
    StageError::new(stage::RELAY, code, message, 1).with_cause(provider_cause(kind))
}

fn provider_code_message(code: ProviderErrorCode) -> &'static str {
    match code {
        ProviderErrorCode::Spawn => "the provider's CLI could not be started",
        ProviderErrorCode::Io => "I/O error while running the provider",
        ProviderErrorCode::InvalidConfiguration => "invalid provider configuration",
        ProviderErrorCode::AuthenticationRejected => "the provider rejected its credentials",
        ProviderErrorCode::UnsupportedShim => "the provider's CLI is an unsupported script wrapper",
        ProviderErrorCode::OutputTooLarge => "the provider returned too much output",
        ProviderErrorCode::InputTooLarge => "the dictation is too large for the provider",
        ProviderErrorCode::UnsupportedSystemPrompt => "the provider cannot take this system prompt",
        ProviderErrorCode::UnexpectedToolActivity => "the provider tried to use a tool",
        ProviderErrorCode::NonzeroExit => "the provider reported a failure",
        ProviderErrorCode::EndpointUnavailable => "the provider endpoint is unavailable",
        ProviderErrorCode::HttpStatus(_) => "the provider endpoint returned an unexpected status",
        ProviderErrorCode::InvalidOutput => unreachable!("mapped to relay_invalid_output"),
    }
}

/// The provider error as a stable snake_case cause code.
fn provider_cause(kind: ProviderError) -> String {
    let cause = match kind {
        ProviderError::NotInstalled => "not_installed",
        ProviderError::NotLoggedIn => "not_logged_in",
        ProviderError::Timeout => "timeout",
        ProviderError::QuotaExceeded { .. } => "quota_exceeded",
        ProviderError::RateLimited { .. } => "rate_limited",
        ProviderError::Other { code } => match code {
            ProviderErrorCode::Spawn => "spawn",
            ProviderErrorCode::Io => "io",
            ProviderErrorCode::InvalidConfiguration => "invalid_configuration",
            ProviderErrorCode::AuthenticationRejected => "authentication_rejected",
            ProviderErrorCode::UnsupportedShim => "unsupported_shim",
            ProviderErrorCode::InvalidOutput => "invalid_output",
            ProviderErrorCode::OutputTooLarge => "output_too_large",
            ProviderErrorCode::InputTooLarge => "input_too_large",
            ProviderErrorCode::UnsupportedSystemPrompt => "unsupported_system_prompt",
            ProviderErrorCode::UnexpectedToolActivity => "unexpected_tool_activity",
            ProviderErrorCode::NonzeroExit => "nonzero_exit",
            ProviderErrorCode::EndpointUnavailable => "endpoint_unavailable",
            ProviderErrorCode::HttpStatus(_) => "http_status",
        },
    };
    cause.to_owned()
}
