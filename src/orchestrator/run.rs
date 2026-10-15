//! The `pumice run` adapter (p12-pumice-run): one audio file through the
//! shared flow execution (`flow.rs`) — transcription by the sibling
//! `pumice-transcribe` program, the formatter and the relay in-process —
//! under one overall deadline anchored to command start (D2), with
//! Ctrl-C/SIGTERM cancellation and bounded cleanup.
//!
//! What only the command does lives here: registering the signal handlers,
//! checking the audio file, resolving the sibling and checking the flow's
//! model with its `models list` before anything is transcribed. Each run
//! builds its own [`Runtime`]; nothing outlives the process.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use pumice_relay::process::CleanupScope;

use crate::dispatch;
use crate::orchestrator::cancel;
use crate::orchestrator::config;
use crate::orchestrator::error::StageError;
use crate::orchestrator::events::{EventSink, EventWriter, event};
use crate::orchestrator::flow::{
    self, AudioSource, Operation, PreparedFlow, Runtime, set_reference_location,
};
use crate::orchestrator::result::{RunOutput, StageReports, stage};
use crate::orchestrator::transcription::Transcribe;

/// Mirrors the transcription module's `MAX_FILE_BYTES`
/// (crates/transcription/src/audio.rs); the child enforces it again on its
/// own. Kept in sync by hand: `pumice` must not depend on the transcription
/// crate (arch-modules/D13).
const MAX_AUDIO_BYTES: u64 = 25_000_000;

/// The `models list` preflight subprocess never takes longer than this
/// (within the remaining overall budget).
const MODELS_LIST_CAP: Duration = Duration::from_secs(5);

/// What a run is asked to do; already parsed and validated arguments.
pub struct RunParams {
    pub audio: PathBuf,
    pub flow: String,
    pub config: Option<PathBuf>,
}

/// Runs one flow to its end: stages, cancellation and bounded cleanup.
/// `events` is the stderr JSONL writer, present only under `--json`.
pub async fn execute(
    params: &RunParams,
    operation_id: String,
    events: Option<&mut EventWriter>,
) -> RunOutput {
    let mut events = events;
    // The signal handlers go in before anything is prepared or spawned; a
    // registration failure stops the run before it starts.
    let (cancel, cancel_task) = match cancel::install() {
        Ok(installed) => installed,
        Err(_) => {
            let error = StageError::new(
                stage::PREPARE,
                "internal",
                "the cancellation signal handlers could not be registered",
                1,
            );
            if let Some(events) = events.as_mut() {
                events.emit(None, event::STARTED, None, None);
                events.emit(None, event::FINISHED, Some(0), Some(error.code));
            }
            return RunOutput {
                operation_id,
                flow: Some(params.flow.clone()),
                transcript: None,
                result: None,
                stages: StageReports::default(),
                total: Duration::ZERO,
                error: Some(error),
            };
        }
    };
    // The overall clock starts here, at command start (D2).
    let mut operation = Operation::start(
        operation_id,
        Instant::now(),
        Some(params.flow.clone()),
        cancel,
        events.map(|events| events as &mut dyn EventSink),
    );
    let mut runtime = None;
    let outcome = run_flow(params, &mut operation, &mut runtime).await;
    let output = operation
        .finish(outcome, runtime.as_ref().map(Runtime::scope))
        .await;
    cancel_task.abort();
    output
}

/// What the `prepare` stage hands the shared stages.
struct Prepared {
    runtime: Runtime,
    flow: PreparedFlow,
    audio: AudioSource,
    stt: Transcribe,
}

/// The four stages in order. `runtime` is filled in once prepared, so its
/// cleanup scope is drained after a later stage too. (Until the relay stage
/// runs, the scope is empty: a failed preparation has nothing to drain.)
async fn run_flow(
    params: &RunParams,
    operation: &mut Operation<'_>,
    runtime: &mut Option<Runtime>,
) -> Result<String, StageError> {
    let at = operation.stage_started(stage::PREPARE);
    let result = prepare(params, operation).await;
    operation.stage_done(stage::PREPARE, at, result.as_ref().err());
    let prepared = result?;
    let runtime = runtime.insert(prepared.runtime);
    operation
        .run_stages(runtime, &prepared.flow, &prepared.stt, &prepared.audio)
        .await
}

async fn prepare(params: &RunParams, operation: &Operation<'_>) -> Result<Prepared, StageError> {
    if let Some(reason) = cancel::current(operation.cancel()) {
        return Err(StageError::cancelled(stage::PREPARE, reason));
    }

    // The orchestrator configuration, the selected flow and its deadline.
    let loaded = config::load(params.config.as_deref())
        .map_err(|error| StageError::invalid_config(&error))?;
    let prepared_flow = flow::prepare_flow(&loaded, &params.flow, operation.arrival())?;
    let work_deadline = prepared_flow.deadlines.work();

    // The audio input: a regular file within the transcription byte
    // limit. The child still enforces codecs and duration itself.
    let audio = std::path::absolute(&params.audio).map_err(|_| {
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

    // The relay pipeline, the formatter registry and every enabled flow's
    // references; the cleanup scope collects provider trees abandoned by
    // cancellation.
    let runtime = Runtime::build(&loaded, Arc::new(CleanupScope::new()))?;

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
    let entries = match stt.models_list(models_deadline, operation.cancel()).await {
        Ok(entries) => entries,
        Err(failure) => {
            return Err(flow::map_stt_failure(
                stage::PREPARE,
                failure,
                work_deadline,
            ));
        }
    };
    let selected = &prepared_flow.flow;
    match entries
        .iter()
        .find(|entry| entry.id == selected.transcription_model)
    {
        None => {
            let mut error = StageError::new(
                stage::PREPARE,
                "transcription_model_not_found",
                format!(
                    "flow \"{}\" names a transcription model that is not in the transcription configuration",
                    selected.id
                ),
                2,
            );
            set_reference_location(
                &mut error,
                &loaded.path,
                selected.references.transcription_model,
            );
            return Err(error);
        }
        Some(entry) if !entry.exists => {
            let mut error = StageError::new(
                stage::PREPARE,
                "transcription_model_not_found",
                "the transcription model file is missing; install or download it first",
                2,
            );
            set_reference_location(
                &mut error,
                &loaded.path,
                selected.references.transcription_model,
            );
            return Err(error);
        }
        Some(_) => {}
    }

    Ok(Prepared {
        runtime,
        flow: prepared_flow,
        audio: AudioSource::File(audio),
        stt,
    })
}
