//! The flow execution seam shared by `pumice run` and the planned
//! `pumice serve` (p13.1-flow-seam): what is prepared once, separate from
//! what runs per operation.
//!
//! - [`Runtime`]: the validated relay pipeline, its cleanup scope and the
//!   formatter registry. `pumice run` builds one per invocation; a service
//!   builds one for its lifetime and runs every operation against it, so the
//!   pipeline's own concurrency limits keep applying (design §6).
//! - [`prepare_flow`]: selects a flow and anchors its deadline to an instant
//!   the caller passes in (command start, or HTTP request arrival), so no
//!   stage ever resets the overall budget (p12-pumice-run/D2).
//! - [`Operation`]: one execution. It records the stage reports and the raw
//!   transcript, emits metadata-only events to an [`EventSink`], watches a
//!   cancellation context any owner can fire, and runs transcription,
//!   formatter and relay through an injectable [`Transcriber`].
//!
//! The raw transcript is kept byte for byte and reported separately from the
//! final text, also when a later stage fails (D3/D7). No transcript or final
//! text ever reaches the events or an error message.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use pumice_formatter::{ConflictPolicy, FormatterDefinition};
use pumice_relay::pipeline::Pipeline;
use pumice_relay::process::CleanupScope;

use crate::orchestrator::cancel::{self, CancelGuard};
use crate::orchestrator::config::{Flow, LoadedConfig, TEARDOWN_RESERVE};
use crate::orchestrator::error::StageError;
use crate::orchestrator::events::{EventSink, event};
use crate::orchestrator::relay as relay_stage;
use crate::orchestrator::result::{
    ErrorLocation, FinalText, RunOutput, StageReport, StageReports, StageStatus, Transcript,
    millis, stage,
};
use crate::orchestrator::transcription::{SttFailure, TranscriptOutput};

/// Bounded drain of the cleanup scope before the result goes out.
const CLEANUP_LIMIT: Duration = Duration::from_secs(2);

/// The audio one operation transcribes. `pumice run` hands the transcription
/// child a file it has already checked; `pumice serve` holds a dictation's
/// upload in memory (never written to disk). No `Debug`: the bytes are the
/// user's audio.
pub enum AudioSource {
    File(PathBuf),
    Bytes(bytes::Bytes),
}

/// How an operation reaches the transcription module: the sibling
/// `pumice-transcribe file --json` subprocess for `pumice run`
/// ([`Transcribe`](crate::orchestrator::transcription::Transcribe)), the
/// warm managed child for `pumice serve`
/// ([`WarmTranscriber`](crate::orchestrator::serve::warm::WarmTranscriber)).
/// An implementation must stop at `deadline`
/// (failing with [`SttFailure::Timeout`]) and when `cancel` fires (failing
/// with [`SttFailure::Cancelled`]), with whatever it started torn down.
pub trait Transcriber: Sync {
    fn transcribe(
        &self,
        audio: &AudioSource,
        model: &str,
        deadline: Instant,
        cancel: &CancelGuard,
    ) -> impl Future<Output = Result<TranscriptOutput, SttFailure>> + Send;
}

/// What is prepared once and shared by every operation run against it.
pub struct Runtime {
    pipeline: Pipeline,
    scope: Arc<CleanupScope>,
    formatter_config: Option<pumice_formatter::Config>,
    /// The orchestrator file the flows came from, for error locations.
    orchestrator_path: PathBuf,
}

impl Runtime {
    /// Builds the relay pipeline (its providers' trees go to `scope`) and
    /// loads the formatter registry, then checks every enabled flow's relay
    /// route and formatter, so a misconfiguration fails before any inference
    /// or paid call. Runs no detection and starts no listener.
    pub fn build(loaded: &LoadedConfig, scope: Arc<CleanupScope>) -> Result<Runtime, StageError> {
        let pipeline =
            relay_stage::build_pipeline(&loaded.config.modules.relay.config, Arc::clone(&scope))?;

        // Every enabled flow's route must resolve to an enabled provider (or
        // passthrough) before anything runs.
        for other in loaded.config.flows.iter().filter(|other| other.enabled) {
            if !relay_stage::route_enabled(&pipeline, &other.relay_route) {
                let mut error = StageError::new(
                    stage::PREPARE,
                    "relay_route_invalid",
                    format!(
                        "flow \"{}\" names a relay route that is not an enabled provider in the relay configuration",
                        other.id
                    ),
                    2,
                );
                set_reference_location(&mut error, &loaded.path, other.references.relay_route);
                return Err(error);
            }
        }

        // The formatter configuration and every enabled flow's formatter id.
        let formatter_config = match &loaded.config.modules.formatter {
            Some(reference) => {
                let loaded_formatter =
                    pumice_formatter::load(Some(&reference.config)).map_err(|error| {
                        let mut stage_error =
                            StageError::new(stage::PREPARE, "invalid_config", error.to_string(), 2);
                        if let Some(file) = error.path() {
                            stage_error.set_location(ErrorLocation {
                                file: file.to_path_buf(),
                                line: error.line(),
                                column: error.column(),
                            });
                        }
                        stage_error
                    })?;
                Some(loaded_formatter.config)
            }
            None => None,
        };
        for other in loaded.config.flows.iter().filter(|other| other.enabled) {
            if let Some(id) = &other.formatter
                && resolve_formatter(id, formatter_config.as_ref()).is_none()
            {
                let mut error = StageError::new(
                    stage::PREPARE,
                    "formatter_not_found",
                    format!(
                        "flow \"{}\" names formatter \"{id}\", which is not registered in the formatter configuration",
                        other.id
                    ),
                    2,
                );
                set_reference_location(&mut error, &loaded.path, other.references.formatter);
                return Err(error);
            }
        }

        Ok(Runtime {
            pipeline,
            scope,
            formatter_config,
            orchestrator_path: loaded.path.clone(),
        })
    }

    /// The scope collecting provider trees a cancellation abandoned. An
    /// operation drains it before reporting; a service runs one operation
    /// at a time against it, so the drain only ever waits for its own work.
    pub fn scope(&self) -> &CleanupScope {
        &self.scope
    }
}

/// The overall flow deadline: the work stops [`TEARDOWN_RESERVE`] before
/// `arrival + total`, and every stage deadline is the earlier of its own cap
/// and that work deadline (D2; the budget never resets at a boundary).
#[derive(Clone, Copy, Debug)]
pub struct Deadlines {
    work: Instant,
}

impl Deadlines {
    pub fn anchored(arrival: Instant, total: Duration) -> Result<Deadlines, StageError> {
        let Some(total_deadline) = arrival.checked_add(total) else {
            return Err(StageError::new(
                stage::PREPARE,
                "internal",
                "budget overflow",
                1,
            ));
        };
        let work = total_deadline
            .checked_sub(TEARDOWN_RESERVE)
            .unwrap_or(arrival);
        Ok(Deadlines { work })
    }

    /// When the flow's work must stop.
    pub fn work(&self) -> Instant {
        self.work
    }

    /// A stage's deadline when it starts at `at` with its own `cap`.
    pub fn stage(&self, at: Instant, cap: Duration) -> Instant {
        at.checked_add(cap)
            .map_or(self.work, |own| own.min(self.work))
    }
}

/// A selected, enabled flow and its deadlines.
pub struct PreparedFlow {
    pub flow: Flow,
    pub deadlines: Deadlines,
}

/// Selects the enabled flow `id` and anchors its budget to `arrival`; a
/// budget already spent by then fails with `flow_timeout` before any work.
pub fn prepare_flow(
    loaded: &LoadedConfig,
    id: &str,
    arrival: Instant,
) -> Result<PreparedFlow, StageError> {
    let flow = match loaded.flow(id) {
        None => {
            return Err(StageError::new(
                stage::PREPARE,
                "flow_not_found",
                format!("no flow named \"{id}\" in the orchestrator configuration"),
                2,
            ));
        }
        Some(flow) if !flow.enabled => {
            return Err(StageError::new(
                stage::PREPARE,
                "flow_disabled",
                format!(
                    "flow \"{}\" is disabled; set \"enabled: true\" to use it",
                    flow.id
                ),
                2,
            ));
        }
        Some(flow) => flow.clone(),
    };
    let deadlines = Deadlines::anchored(arrival, flow.budgets.total)?;
    if Instant::now() >= deadlines.work() {
        return Err(flow_timeout(stage::PREPARE));
    }
    Ok(PreparedFlow { flow, deadlines })
}

/// One flow execution: its stage reports, the raw transcript once there is
/// one, its events and its cancellation. Stages run in order; the caller
/// runs its own preparation between [`Operation::stage_started`] and
/// [`Operation::stage_done`] for `prepare`, then [`Operation::run_stages`],
/// then [`Operation::finish`].
pub struct Operation<'a> {
    operation_id: String,
    arrival: Instant,
    events: Option<&'a mut dyn EventSink>,
    cancel: CancelGuard,
    flow: Option<String>,
    stages: StageReports,
    transcript: Option<Transcript>,
}

impl<'a> Operation<'a> {
    /// Starts an operation whose clock runs from `arrival` and emits its
    /// `started` event. `flow` is the requested flow id, reported as is.
    pub fn start(
        operation_id: String,
        arrival: Instant,
        flow: Option<String>,
        cancel: CancelGuard,
        events: Option<&'a mut dyn EventSink>,
    ) -> Operation<'a> {
        let mut operation = Operation {
            operation_id,
            arrival,
            events,
            cancel,
            flow,
            stages: StageReports::default(),
            transcript: None,
        };
        operation.emit(None, event::STARTED, None, None);
        operation
    }

    pub fn arrival(&self) -> Instant {
        self.arrival
    }

    pub fn cancel(&self) -> &CancelGuard {
        &self.cancel
    }

    /// Names the flow once it is known: a service learns it while it reads
    /// the request, after the operation started.
    pub fn set_flow(&mut self, flow: String) {
        self.flow = Some(flow);
    }

    fn emit(
        &mut self,
        stage_name: Option<&'static str>,
        event_name: &'static str,
        elapsed_ms: Option<u64>,
        code: Option<&'static str>,
    ) {
        if let Some(events) = self.events.as_mut() {
            events.emit(stage_name, event_name, elapsed_ms, code);
        }
    }

    /// Reports a stage as started; pass the returned instant to
    /// [`Operation::stage_done`].
    pub fn stage_started(&mut self, stage_name: &'static str) -> Instant {
        let at = Instant::now();
        self.emit(Some(stage_name), event::STAGE_STARTED, None, None);
        at
    }

    /// Marks a stage finished and reports it.
    pub fn stage_done(
        &mut self,
        stage_name: &'static str,
        at: Instant,
        error: Option<&StageError>,
    ) {
        let elapsed = at.elapsed();
        let report = self.stage_report_mut(stage_name);
        let status = match error {
            None => StageStatus::Ok,
            Some(error) if error.code == "cancelled" => StageStatus::Cancelled,
            Some(_) => StageStatus::Error,
        };
        report.status = status;
        report.elapsed_ms = Some(millis(elapsed));
        self.emit(
            Some(stage_name),
            event::STAGE_FINISHED,
            Some(millis(elapsed)),
            error.map(|error| error.code),
        );
    }

    fn stage_report_mut(&mut self, stage_name: &'static str) -> &mut StageReport {
        match stage_name {
            stage::PREPARE => &mut self.stages.prepare,
            stage::TRANSCRIPTION => &mut self.stages.transcription,
            stage::FORMATTER => &mut self.stages.formatter,
            stage::RELAY => &mut self.stages.relay,
            _ => unreachable!("unknown stage"),
        }
    }

    /// Transcription, formatter and relay in order, for a prepared flow.
    /// The raw transcript survives any later failure (it is kept in the
    /// operation and reported by [`Operation::finish`]).
    pub async fn run_stages<T: Transcriber>(
        &mut self,
        runtime: &Runtime,
        prepared: &PreparedFlow,
        transcriber: &T,
        audio: &AudioSource,
    ) -> Result<String, StageError> {
        let raw = self
            .transcription_stage(prepared, transcriber, audio)
            .await?;

        // A whitespace-only transcript succeeds without a provider call; the
        // formatter and relay stages are skipped (design §3).
        if raw.trim().is_empty() {
            return Ok(raw);
        }

        let system = self.formatter_stage(runtime, prepared, &raw)?;
        self.relay_call(runtime, prepared, &raw, system).await
    }

    /// Ends the operation: bounded teardown of `scope` (the runtime's, when
    /// one was built), the `finished` event and the output. A teardown
    /// failure is never a success; with a primary failure it stays secondary
    /// (an event). Dropping this future mid-teardown loses no ownership:
    /// whatever was not finished stays in `scope`, so its owner can resume
    /// with another [`CleanupScope::drain`].
    pub async fn finish(
        mut self,
        outcome: Result<String, StageError>,
        scope: Option<&CleanupScope>,
    ) -> RunOutput {
        let unreaped = match scope {
            Some(scope) => scope.drain(CLEANUP_LIMIT).await,
            None => 0,
        };
        let mut outcome = outcome;
        if unreaped > 0 {
            self.emit(
                Some(stage::CLEANUP),
                event::STAGE_FINISHED,
                None,
                Some("cleanup_failed"),
            );
            if outcome.is_ok() {
                outcome = Err(StageError::new(
                    stage::CLEANUP,
                    "cleanup_failed",
                    "cleanup of a child process or temporary directory did not finish in time",
                    1,
                ));
            }
        }

        let total = self.arrival.elapsed();
        let (result, error) = match outcome {
            Ok(text) => (Some(FinalText { text }), None),
            Err(error) => (None, Some(error)),
        };
        self.emit(
            None,
            event::FINISHED,
            Some(millis(total)),
            error.as_ref().map(|error| error.code),
        );
        RunOutput {
            operation_id: self.operation_id,
            flow: self.flow,
            transcript: self.transcript,
            result,
            stages: self.stages,
            total,
            error,
        }
    }

    async fn transcription_stage<T: Transcriber>(
        &mut self,
        prepared: &PreparedFlow,
        transcriber: &T,
        audio: &AudioSource,
    ) -> Result<String, StageError> {
        let at = Instant::now();
        let work_deadline = prepared.deadlines.work();
        let deadline = prepared
            .deadlines
            .stage(at, prepared.flow.budgets.transcription);
        self.emit(Some(stage::TRANSCRIPTION), event::STAGE_STARTED, None, None);

        let outcome = transcriber
            .transcribe(
                audio,
                &prepared.flow.transcription_model,
                deadline,
                &self.cancel,
            )
            .await;
        let result = match outcome {
            Ok(output) => {
                // A success that lands past the stage deadline is rejected,
                // never accepted late (D2).
                let now = Instant::now();
                if now >= deadline {
                    Err(relay_stage::timeout_error_at(
                        stage::TRANSCRIPTION,
                        "transcription_timeout",
                        "the transcription exceeded its deadline",
                        now,
                        work_deadline,
                    ))
                } else {
                    Ok(output)
                }
            }
            Err(failure) => Err(map_stt_failure(
                stage::TRANSCRIPTION,
                failure,
                work_deadline,
            )),
        };

        match result {
            Ok(output) => {
                self.stages.transcription = StageReport {
                    model_load_ms: output.model_load_ms,
                    inference_ms: output.transcription_ms,
                    ..StageReport::measured(StageStatus::Ok, at.elapsed())
                };
                self.emit(
                    Some(stage::TRANSCRIPTION),
                    event::STAGE_FINISHED,
                    Some(millis(at.elapsed())),
                    None,
                );
                self.transcript = Some(Transcript {
                    text: output.text.clone(),
                    language: output.language,
                    audio_seconds: output.audio_seconds,
                });
                Ok(output.text)
            }
            Err(error) => {
                self.stage_done(stage::TRANSCRIPTION, at, Some(&error));
                Err(error)
            }
        }
    }

    /// Applies the selected formatter; `Ok(None)` for `formatter: null`,
    /// which leaves the stage skipped (nothing is injected — D4).
    fn formatter_stage(
        &mut self,
        runtime: &Runtime,
        prepared: &PreparedFlow,
        raw: &str,
    ) -> Result<Option<String>, StageError> {
        let Some(id) = prepared.flow.formatter.clone() else {
            return Ok(None);
        };
        let Some(definition) = resolve_formatter(id.as_str(), runtime.formatter_config.as_ref())
        else {
            let mut error = StageError::new(
                stage::FORMATTER,
                "formatter_not_found",
                "the selected formatter is not registered in the formatter configuration",
                2,
            );
            set_reference_location(
                &mut error,
                &runtime.orchestrator_path,
                prepared.flow.references.formatter,
            );
            return Err(error);
        };

        let at = self.stage_started(stage::FORMATTER);
        let deadline = prepared
            .deadlines
            .stage(at, prepared.flow.budgets.formatter);
        let result = apply_formatter_stage(
            raw,
            &prepared.flow.relay_route,
            &definition,
            deadline,
            prepared.deadlines.work(),
            &self.cancel,
            &Instant::now,
        );
        self.stage_done(stage::FORMATTER, at, result.as_ref().err());
        result.map(Some)
    }

    async fn relay_call(
        &mut self,
        runtime: &Runtime,
        prepared: &PreparedFlow,
        raw: &str,
        system: Option<String>,
    ) -> Result<String, StageError> {
        let at = Instant::now();
        let deadline = prepared.deadlines.stage(at, prepared.flow.budgets.relay);
        self.emit(Some(stage::RELAY), event::STAGE_STARTED, None, None);

        let result = self
            .relay_inner(runtime, prepared, raw, system, at, deadline)
            .await;
        self.stage_done(stage::RELAY, at, result.as_ref().err());
        result
    }

    async fn relay_inner(
        &mut self,
        runtime: &Runtime,
        prepared: &PreparedFlow,
        raw: &str,
        system: Option<String>,
        at: Instant,
        deadline: Instant,
    ) -> Result<String, StageError> {
        let work_deadline = prepared.deadlines.work();
        if let Some(reason) = cancel::current(&self.cancel) {
            return Err(StageError::cancelled(stage::RELAY, reason));
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(relay_stage::timeout_error_at(
                stage::RELAY,
                "relay_timeout",
                "the provider call exceeded its deadline",
                now,
                work_deadline,
            ));
        }

        let request = relay_stage::build_request(&prepared.flow.relay_route, raw, system);
        let mut cancelled = self.cancel.clone();
        let call = runtime
            .pipeline
            .format_until_observed(&request, at, Some(deadline), &());
        tokio::pin!(call);
        let outcome = tokio::select! {
            biased;
            reason = cancel::cancelled(&mut cancelled) => {
                // Dropping the call future kills the provider's process tree;
                // the cleanup scope (drained before reporting) reaps it.
                return Err(StageError::cancelled(stage::RELAY, reason));
            }
            outcome = &mut call => outcome,
        };

        let now = Instant::now();
        // Whether the deadline handed to the pipeline is the flow's work
        // deadline: then a caller-bound timeout is the overall budget firing.
        let flow_owned_deadline = deadline == work_deadline;
        let result = relay_stage::map_outcome(outcome, now, work_deadline, flow_owned_deadline);
        match result {
            // Never accept a success that lands past the stage deadline.
            Ok(_) if now >= deadline => Err(relay_stage::timeout_error_at(
                stage::RELAY,
                "relay_timeout",
                "the provider call exceeded its deadline",
                now,
                work_deadline,
            )),
            other => other,
        }
    }
}

/// Maps a transcription failure to the stage's error.
pub fn map_stt_failure(
    stage_name: &'static str,
    failure: SttFailure,
    work_deadline: Instant,
) -> StageError {
    match failure {
        SttFailure::NotInstalled => StageError::new(
            stage_name,
            "transcription_not_installed",
            "pumice-transcribe is not installed next to pumice",
            1,
        ),
        SttFailure::Timeout => relay_stage::timeout_error_at(
            stage_name,
            "transcription_timeout",
            "the transcription module exceeded its deadline",
            Instant::now(),
            work_deadline,
        ),
        SttFailure::Cancelled(reason) => StageError::cancelled(stage_name, reason),
        SttFailure::Protocol(message) => {
            StageError::new(stage_name, "transcription_protocol_error", message, 1)
        }
        SttFailure::Unavailable(message) => {
            StageError::new(stage_name, "transcription_unavailable", message, 1)
        }
        SttFailure::Busy => StageError::new(
            stage_name,
            "transcription_busy",
            "the transcription module is still busy with an earlier transcription",
            1,
        ),
        SttFailure::Module { code, location } => {
            // The child's own message is never forwarded (it could carry
            // dictated text): a fixed message per recognized code.
            let message = match code.as_str() {
                "invalid_arguments" => "invalid arguments to the transcription module",
                "invalid_config" => "the transcription configuration is invalid",
                "invalid_input" | "input_too_large" | "audio_too_long" | "unsupported_format" => {
                    "the transcription module refused the audio input"
                }
                "model_not_found" => "the transcription model was not found",
                _ => "the transcription engine failed",
            };
            let (code_out, exit) = match code.as_str() {
                "invalid_arguments" => ("invalid_arguments", 2),
                "invalid_config" => ("invalid_config", 2),
                "invalid_input" | "input_too_large" | "audio_too_long" | "unsupported_format" => {
                    ("transcription_invalid_input", 2)
                }
                "model_not_found" => ("transcription_model_not_found", 2),
                _ => ("transcription_engine_failed", 1),
            };
            let mut error = StageError::new(stage_name, code_out, message, exit).with_cause(code);
            if let Some(location) = location {
                error.locate(location);
            }
            error
        }
    }
}

/// Attaches the orchestrator file's `file:line:column` of the reference the
/// error names, when the span survived parsing.
pub fn set_reference_location(error: &mut StageError, orchestrator: &Path, at: Option<(u64, u64)>) {
    if let Some((line, column)) = at {
        error.locate(ErrorLocation {
            file: orchestrator.to_path_buf(),
            line: Some(line),
            column: Some(column),
        });
    }
}

fn flow_timeout(stage_name: &'static str) -> StageError {
    StageError::new(
        stage_name,
        "flow_timeout",
        "the flow's total budget is exhausted",
        1,
    )
}

/// Resolves a formatter id: built-ins first, then the configured registry.
fn resolve_formatter(
    id: &str,
    config: Option<&pumice_formatter::Config>,
) -> Option<FormatterDefinition> {
    if id == pumice_formatter::BUILTIN_CLEAN {
        return Some(pumice_formatter::builtin_clean());
    }
    config.and_then(|config| config.definition(id).cloned())
}

/// Builds the relay request and applies the formatter. The request is exactly
/// `{"model": route, "stream": false, "messages": [{"role": "user",
/// "content": raw}]}`; the formatter inserts its system message and nothing
/// else changes (p11-formatter/D2). Returns the formatter's system text for
/// the relay's `ExtractedRequest`, which the orchestrator constructs directly
/// from the raw transcript (D3) instead of re-parsing the transformed JSON
/// through the envelope extraction.
///
/// The stage is synchronous and cannot be preempted: cancellation and the
/// deadline are checked before and after, and a late success is rejected.
/// `now` is injectable so the late-success check is tested without real
/// sleeps.
fn apply_formatter_stage(
    raw: &str,
    route: &str,
    definition: &FormatterDefinition,
    deadline: Instant,
    work_deadline: Instant,
    cancel: &CancelGuard,
    now: &dyn Fn() -> Instant,
) -> Result<String, StageError> {
    if let Some(reason) = cancel::current(cancel) {
        return Err(StageError::cancelled(stage::FORMATTER, reason));
    }
    if now() >= deadline {
        return Err(relay_stage::timeout_error_at(
            stage::FORMATTER,
            "formatter_timeout",
            "the formatter exceeded its deadline",
            now(),
            work_deadline,
        ));
    }
    let request = serde_json::json!({
        "model": route,
        "stream": false,
        "messages": [{"role": "user", "content": raw}],
    })
    .to_string();
    if request.len() > pumice_formatter::cli::MAX_REQUEST_BYTES {
        return Err(StageError::new(
            stage::FORMATTER,
            "formatter_invalid_request",
            "the transcription is too large to format",
            1,
        ));
    }
    let _transformed = pumice_formatter::apply(&request, definition, ConflictPolicy::Reject)
        .map_err(map_formatter_error)?;
    if let Some(reason) = cancel::current(cancel) {
        return Err(StageError::cancelled(stage::FORMATTER, reason));
    }
    if now() >= deadline {
        return Err(relay_stage::timeout_error_at(
            stage::FORMATTER,
            "formatter_timeout",
            "the formatter exceeded its deadline",
            now(),
            work_deadline,
        ));
    }
    Ok(definition.system.clone())
}

fn map_formatter_error(error: pumice_formatter::Error) -> StageError {
    use pumice_formatter::ErrorCode;
    let (code, exit) = match error.code {
        ErrorCode::FormatterInstructionConflict => ("formatter_instruction_conflict", 1),
        ErrorCode::InvalidInput | ErrorCode::InputTooLarge => ("formatter_invalid_request", 1),
        ErrorCode::FormatterNotFound => ("formatter_not_found", 2),
        ErrorCode::InvalidArguments | ErrorCode::InvalidConfig => ("invalid_config", 2),
        ErrorCode::Internal => ("internal", 1),
    };
    let mut stage_error = StageError::new(stage::FORMATTER, code, error.message, exit)
        .with_cause(error.code.as_str());
    if let Some(location) = error.location {
        stage_error.set_location(ErrorLocation {
            file: location.file,
            line: location.line,
            column: location.column,
        });
    }
    stage_error
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumice_formatter::ErrorCode;

    fn definition() -> FormatterDefinition {
        FormatterDefinition {
            id: "test".to_owned(),
            description: "test".to_owned(),
            system: "THE RULES".to_owned(),
        }
    }

    fn never_cancelled() -> CancelGuard {
        let (_sender, receiver) = tokio::sync::watch::channel(None);
        receiver
    }

    #[test]
    fn formatter_inserts_its_rules_and_returns_them() {
        let now = Instant::now();
        let system = apply_formatter_stage(
            "ola mundo",
            "claude",
            &definition(),
            now + Duration::from_secs(30),
            now + Duration::from_secs(60),
            &never_cancelled(),
            &Instant::now,
        )
        .expect("formatter applies");
        assert_eq!(system, "THE RULES");
    }

    #[test]
    fn formatter_late_success_is_rejected() {
        // The injected clock jumps past the stage deadline between the checks:
        // the synchronous transform succeeded but its result is rejected.
        let start = Instant::now();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let clock = || {
            if calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 1 {
                start + Duration::from_secs(10)
            } else {
                start
            }
        };
        // The stage deadline fires before the overall work budget.
        let error = apply_formatter_stage(
            "ola",
            "claude",
            &definition(),
            start + Duration::from_secs(5),
            start + Duration::from_secs(60),
            &never_cancelled(),
            &clock,
        )
        .expect_err("late success rejected");
        assert_eq!(error.code, "formatter_timeout");
        assert_eq!(error.stage, stage::FORMATTER);

        // When the overall work budget fired first, it is a flow timeout.
        // (The caller always passes the stage deadline as the earlier of the
        // stage cap and the work budget: here both are start + 5 s.)
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let clock = || {
            if calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 1 {
                start + Duration::from_secs(10)
            } else {
                start
            }
        };
        let error = apply_formatter_stage(
            "ola",
            "claude",
            &definition(),
            start + Duration::from_secs(5),
            start + Duration::from_secs(5),
            &never_cancelled(),
            &clock,
        )
        .expect_err("late success rejected");
        assert_eq!(error.code, "flow_timeout");
    }

    #[test]
    fn formatter_deadline_spent_before_start_never_transforms() {
        let start = Instant::now();
        let error = apply_formatter_stage(
            "ola",
            "claude",
            &definition(),
            start,
            start + Duration::from_secs(60),
            &never_cancelled(),
            &Instant::now,
        )
        .expect_err("spent deadline rejected");
        assert_eq!(error.code, "formatter_timeout");
    }

    #[test]
    fn formatter_error_mapping_is_stable() {
        let conflict =
            map_formatter_error(pumice_formatter::Error::formatter_instruction_conflict());
        assert_eq!(conflict.code, "formatter_instruction_conflict");
        assert_eq!(conflict.exit, 1);

        let invalid = map_formatter_error(pumice_formatter::Error::invalid_input("bad request"));
        assert_eq!(invalid.code, "formatter_invalid_request");
        assert_eq!(invalid.exit, 1);

        let not_found = map_formatter_error(pumice_formatter::Error::formatter_not_found("x"));
        assert_eq!(not_found.code, "formatter_not_found");
        assert_eq!(not_found.exit, 2);

        let internal =
            map_formatter_error(pumice_formatter::Error::new(ErrorCode::Internal, "boom"));
        assert_eq!(internal.code, "internal");
    }
}
