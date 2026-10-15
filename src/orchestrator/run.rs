//! The `pumice run` flow (p12-pumice-run): one audio file through
//! transcription (the sibling `pumice-transcribe` program), the formatter and
//! the relay (both in-process), under one overall deadline that stages can
//! only shorten (D2), with Ctrl-C/SIGTERM cancellation and bounded cleanup.
//!
//! The raw transcript is kept byte for byte and reported separately from the
//! final text, also when a later stage fails (D3/D7). No transcript or final
//! text ever reaches stderr, the events stream or an error message.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use pumice_formatter::{ConflictPolicy, FormatterDefinition};
use pumice_relay::pipeline::Pipeline;
use pumice_relay::process::CleanupScope;

use crate::dispatch;
use crate::orchestrator::cancel::{self, CancelGuard};
use crate::orchestrator::config::{self, Flow, TEARDOWN_RESERVE};
use crate::orchestrator::error::StageError;
use crate::orchestrator::events::{EventWriter, event};
use crate::orchestrator::relay as relay_stage;
use crate::orchestrator::result::{
    ErrorLocation, FinalText, FlowError, MODULE, RunEnvelope, SCHEMA_VERSION, StageReport,
    StageReports, StageStatus, Timings, Transcript, stage,
};
use crate::orchestrator::transcription::{SttFailure, Transcribe};

/// Mirrors the transcription module's `MAX_FILE_BYTES`
/// (crates/transcription/src/audio.rs); the child enforces it again on its
/// own. Kept in sync by hand: `pumice` must not depend on the transcription
/// crate (arch-modules/D13).
const MAX_AUDIO_BYTES: u64 = 25_000_000;

/// The `models list` preflight subprocess never takes longer than this
/// (within the remaining overall budget).
const MODELS_LIST_CAP: Duration = Duration::from_secs(5);

/// Bounded drain of the cleanup scope before the result goes out.
const CLEANUP_LIMIT: Duration = Duration::from_secs(2);

/// What a run is asked to do; already parsed and validated arguments.
pub struct RunParams {
    pub audio: PathBuf,
    pub flow: String,
    pub config: Option<PathBuf>,
}

/// Everything the CLI needs to report: the envelope fields and, through the
/// error, the exit code.
pub struct RunOutput {
    pub operation_id: String,
    pub flow: Option<String>,
    pub transcript: Option<Transcript>,
    pub result: Option<FinalText>,
    pub stages: StageReports,
    pub total: Duration,
    pub error: Option<StageError>,
}

impl RunOutput {
    pub fn exit_code(&self) -> u8 {
        self.error.as_ref().map_or(0, |error| error.exit)
    }

    /// The one stdout document for `--json`.
    pub fn into_envelope(self) -> RunEnvelope {
        RunEnvelope {
            schema_version: SCHEMA_VERSION,
            operation_id: self.operation_id,
            module: MODULE,
            ok: self.error.is_none(),
            flow: self.flow,
            transcript: self.transcript,
            result: self.result,
            stages: self.stages,
            timings_ms: Timings {
                total: millis(self.total),
            },
            error: self.error.as_ref().map(flow_error),
        }
    }
}

fn flow_error(error: &StageError) -> FlowError {
    FlowError {
        stage: error.stage.to_owned(),
        code: error.code.to_owned(),
        message: error.message.clone(),
        retryable: false,
        cause_code: error.cause_code.as_deref().map(str::to_owned),
        location: error.location.as_deref().cloned(),
    }
}

fn millis(duration: Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}

/// Runs one flow to its end: stages, cancellation and bounded cleanup.
/// `events` is the stderr JSONL writer, present only under `--json`.
pub async fn execute(
    params: &RunParams,
    operation_id: String,
    events: Option<&mut EventWriter>,
) -> RunOutput {
    let (cancel, cancel_task) = cancel::install();
    let mut execution = Execution {
        params,
        events,
        cancel,
        operation_id,
        started: Instant::now(),
        stages: StageReports::default(),
        flow: Some(params.flow.clone()),
        transcript: None,
        scope: None,
        total: Duration::ZERO,
    };
    execution.emit(None, event::STARTED, None, None);
    let output = execution.run().await;
    cancel_task.abort();
    output
}

/// Everything prepared before transcription starts.
struct Prepared {
    flow: Flow,
    work_deadline: Instant,
    audio: PathBuf,
    stt: Transcribe,
    pipeline: Pipeline,
    formatter_config: Option<pumice_formatter::Config>,
}

struct Execution<'a> {
    params: &'a RunParams,
    events: Option<&'a mut EventWriter>,
    cancel: CancelGuard,
    operation_id: String,
    started: Instant,
    stages: StageReports,
    flow: Option<String>,
    transcript: Option<Transcript>,
    scope: Option<Arc<CleanupScope>>,
    total: Duration,
}

impl Execution<'_> {
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

    /// Marks a stage finished and reports it.
    fn stage_done(&mut self, stage_name: &'static str, at: Instant, error: Option<&StageError>) {
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

    async fn run(&mut self) -> RunOutput {
        let outcome = self.stages_run().await;

        // Bounded teardown: reap every provider tree a cancellation
        // abandoned before the result goes out. A teardown failure is never
        // a success; with a primary failure it stays secondary (an event).
        let unreaped = match &self.scope {
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

        self.total = self.started.elapsed();
        let (result, error) = match outcome {
            Ok(text) => (Some(FinalText { text }), None),
            Err(error) => (None, Some(error)),
        };
        self.emit(
            None,
            event::FINISHED,
            Some(millis(self.total)),
            error.as_ref().map(|error| error.code),
        );
        RunOutput {
            operation_id: self.operation_id.clone(),
            flow: self.flow.clone(),
            transcript: self.transcript.take(),
            result,
            stages: std::mem::take(&mut self.stages),
            total: self.total,
            error,
        }
    }

    /// The four stages in order. The raw transcript survives any later
    /// failure through `self.transcript`.
    async fn stages_run(&mut self) -> Result<String, StageError> {
        let prepared = self.prepare().await?;
        let raw = self.transcription_stage(&prepared).await?;

        // A whitespace-only transcript succeeds without a provider call; the
        // formatter and relay stages are skipped (design §3).
        if raw.trim().is_empty() {
            return Ok(raw);
        }

        let system = self.formatter_stage(&prepared, &raw)?;
        self.relay_call(&prepared, &raw, system).await
    }

    async fn prepare(&mut self) -> Result<Prepared, StageError> {
        let at = Instant::now();
        self.emit(Some(stage::PREPARE), event::STAGE_STARTED, None, None);
        let result = self.prepare_inner().await;
        self.stage_done(stage::PREPARE, at, result.as_ref().err());
        result
    }

    async fn prepare_inner(&mut self) -> Result<Prepared, StageError> {
        if let Some(reason) = cancel::current(&self.cancel) {
            return Err(StageError::cancelled(stage::PREPARE, reason));
        }

        // The orchestrator configuration and the selected flow.
        let loaded = config::load(self.params.config.as_deref())
            .map_err(|error| StageError::invalid_config(&error))?;
        let flow = match loaded.flow(&self.params.flow) {
            None => {
                return Err(StageError::new(
                    stage::PREPARE,
                    "flow_not_found",
                    format!(
                        "no flow named \"{}\" in the orchestrator configuration",
                        self.params.flow
                    ),
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
        let budgets = flow.budgets;

        // One overall clock from command start; stages can only shorten it
        // (D2). Work stops 2.5 s before the total deadline, reserved for
        // teardown and output.
        let Some(total_deadline) = self.started.checked_add(budgets.total) else {
            return Err(StageError::new(
                stage::PREPARE,
                "internal",
                "budget overflow",
                1,
            ));
        };
        let work_deadline = total_deadline
            .checked_sub(TEARDOWN_RESERVE)
            .unwrap_or(self.started);
        if Instant::now() >= work_deadline {
            return Err(flow_timeout(stage::PREPARE));
        }

        // The audio input: a regular file within the transcription byte
        // limit. The child still enforces codecs and duration itself.
        let audio = std::path::absolute(&self.params.audio).map_err(|_| {
            StageError::new(
                stage::PREPARE,
                "transcription_invalid_input",
                "the audio path cannot be resolved",
                2,
            )
        })?;
        match std::fs::metadata(&audio) {
            Ok(metadata) if !metadata.is_file() => {
                return Err(StageError::new(
                    stage::PREPARE,
                    "transcription_invalid_input",
                    "the audio path is not a regular file",
                    2,
                ));
            }
            Ok(metadata) if metadata.len() > MAX_AUDIO_BYTES => {
                return Err(StageError::new(
                    stage::PREPARE,
                    "transcription_invalid_input",
                    "the audio file is too large",
                    2,
                ));
            }
            Ok(_) => {}
            Err(_) => {
                return Err(StageError::new(
                    stage::PREPARE,
                    "transcription_invalid_input",
                    "the audio file does not exist or cannot be read",
                    2,
                ));
            }
        }

        // The relay configuration, its providers and the pipeline; the
        // cleanup scope collects provider trees abandoned by cancellation.
        let scope = Arc::new(CleanupScope::new());
        let pipeline =
            relay_stage::build_pipeline(&loaded.config.modules.relay.config, Arc::clone(&scope))?;
        self.scope = Some(scope);

        // Every enabled flow's route must resolve to an enabled provider (or
        // passthrough) before anything runs.
        for other in loaded.config.flows.iter().filter(|other| other.enabled) {
            if !relay_stage::route_enabled(&pipeline, &other.relay_route) {
                return Err(StageError::new(
                    stage::PREPARE,
                    "relay_route_invalid",
                    format!(
                        "flow \"{}\" names a relay route that is not an enabled provider in the relay configuration",
                        other.id
                    ),
                    2,
                ));
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
                return Err(StageError::new(
                    stage::PREPARE,
                    "formatter_not_found",
                    format!(
                        "flow \"{}\" names formatter \"{id}\", which is not registered in the formatter configuration",
                        other.id
                    ),
                    2,
                ));
            }
        }

        // The transcription sibling (never PATH) and its model list, which
        // also validates the transcription configuration.
        let binary = dispatch::sibling_transcribe().map_err(|_| {
            StageError::new(
                stage::PREPARE,
                "transcription_not_installed",
                "pumice-transcribe is not installed next to pumice",
                1,
            )
        })?;
        let stt = Transcribe::new(binary, loaded.config.modules.transcription.config.clone());
        let models_deadline = work_deadline.min(
            Instant::now()
                .checked_add(MODELS_LIST_CAP)
                .unwrap_or(work_deadline),
        );
        let entries = match stt.models_list(models_deadline, &self.cancel).await {
            Ok(entries) => entries,
            Err(failure) => {
                return Err(self.map_stt_failure(stage::PREPARE, failure, work_deadline));
            }
        };
        match entries
            .iter()
            .find(|entry| entry.id == flow.transcription_model)
        {
            None => {
                return Err(StageError::new(
                    stage::PREPARE,
                    "transcription_model_not_found",
                    format!(
                        "flow \"{}\" names a transcription model that is not in the transcription configuration",
                        flow.id
                    ),
                    2,
                ));
            }
            Some(entry) if !entry.exists => {
                return Err(StageError::new(
                    stage::PREPARE,
                    "transcription_model_not_found",
                    "the transcription model file is missing; install or download it first",
                    2,
                ));
            }
            Some(_) => {}
        }

        Ok(Prepared {
            flow,
            work_deadline,
            audio,
            stt,
            pipeline,
            formatter_config,
        })
    }

    async fn transcription_stage(&mut self, prepared: &Prepared) -> Result<String, StageError> {
        let at = Instant::now();
        let deadline = stage_deadline(
            at,
            prepared.flow.budgets.transcription,
            prepared.work_deadline,
        );
        self.emit(Some(stage::TRANSCRIPTION), event::STAGE_STARTED, None, None);

        let outcome = prepared
            .stt
            .transcribe_file(
                &prepared.audio,
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
                        prepared.work_deadline,
                    ))
                } else {
                    Ok(output)
                }
            }
            Err(failure) => {
                Err(self.map_stt_failure(stage::TRANSCRIPTION, failure, prepared.work_deadline))
            }
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
        prepared: &Prepared,
        raw: &str,
    ) -> Result<Option<String>, StageError> {
        let Some(id) = prepared.flow.formatter.clone() else {
            return Ok(None);
        };
        let Some(definition) = resolve_formatter(id.as_str(), prepared.formatter_config.as_ref())
        else {
            return Err(StageError::new(
                stage::FORMATTER,
                "formatter_not_found",
                "the selected formatter is not registered in the formatter configuration",
                2,
            ));
        };

        let at = Instant::now();
        self.emit(Some(stage::FORMATTER), event::STAGE_STARTED, None, None);
        let deadline = stage_deadline(at, prepared.flow.budgets.formatter, prepared.work_deadline);
        let result = apply_formatter_stage(
            raw,
            &prepared.flow.relay_route,
            &definition,
            deadline,
            prepared.work_deadline,
            &self.cancel,
            &Instant::now,
        );
        self.stage_done(stage::FORMATTER, at, result.as_ref().err());
        result.map(Some)
    }

    async fn relay_call(
        &mut self,
        prepared: &Prepared,
        raw: &str,
        system: Option<String>,
    ) -> Result<String, StageError> {
        let at = Instant::now();
        let deadline = stage_deadline(at, prepared.flow.budgets.relay, prepared.work_deadline);
        self.emit(Some(stage::RELAY), event::STAGE_STARTED, None, None);

        let result = self.relay_inner(prepared, raw, system, at, deadline).await;
        self.stage_done(stage::RELAY, at, result.as_ref().err());
        result
    }

    async fn relay_inner(
        &mut self,
        prepared: &Prepared,
        raw: &str,
        system: Option<String>,
        at: Instant,
        deadline: Instant,
    ) -> Result<String, StageError> {
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
                prepared.work_deadline,
            ));
        }

        let request = relay_stage::build_request(&prepared.flow.relay_route, raw, system);
        let mut cancelled = self.cancel.clone();
        let call = prepared
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
        let flow_owned_deadline = deadline == prepared.work_deadline;
        let result =
            relay_stage::map_outcome(outcome, now, prepared.work_deadline, flow_owned_deadline);
        match result {
            // Never accept a success that lands past the stage deadline.
            Ok(_) if now >= deadline => Err(relay_stage::timeout_error_at(
                stage::RELAY,
                "relay_timeout",
                "the provider call exceeded its deadline",
                now,
                prepared.work_deadline,
            )),
            other => other,
        }
    }

    /// Maps a transcription-child failure to the stage's error.
    fn map_stt_failure(
        &self,
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
            SttFailure::Module {
                code,
                message,
                location,
            } => {
                let fallback = match code.as_str() {
                    "invalid_arguments" => "invalid arguments to the transcription module",
                    "invalid_config" => "the transcription configuration is invalid",
                    "invalid_input" | "input_too_large" | "audio_too_long"
                    | "unsupported_format" => "the transcription module refused the audio input",
                    "model_not_found" => "the transcription model was not found",
                    _ => "the transcription engine failed",
                };
                let (code_out, exit) = match code.as_str() {
                    "invalid_arguments" => ("invalid_arguments", 2),
                    "invalid_config" => ("invalid_config", 2),
                    "invalid_input" | "input_too_large" | "audio_too_long"
                    | "unsupported_format" => ("transcription_invalid_input", 2),
                    "model_not_found" => ("transcription_model_not_found", 2),
                    _ => ("transcription_engine_failed", 1),
                };
                let mut error = StageError::new(
                    stage_name,
                    code_out,
                    message.unwrap_or_else(|| fallback.to_owned()),
                    exit,
                )
                .with_cause(code);
                if let Some(location) = location {
                    error.set_location(location);
                }
                error
            }
        }
    }
}

/// A stage's deadline: the earlier of its own cap and the remaining overall
/// work budget (D2; the budget never resets at a boundary).
fn stage_deadline(at: Instant, cap: Duration, work_deadline: Instant) -> Instant {
    at.checked_add(cap)
        .map_or(work_deadline, |own| own.min(work_deadline))
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
