//! The `pumice run` result envelope (p12-pumice-run/D7): one
//! `schema_version: 1` JSON document on stdout under `--json`, with the raw
//! transcript and the final text as separate fields.
//!
//! [`RunOutput`] is what one flow execution produced, independent of how it
//! is presented: the CLI writes it as this envelope or as plain text, and a
//! future HTTP presenter can reuse the same envelope.
//!
//! Content-bearing types ([`RunOutput`], [`RunEnvelope`], [`Transcript`],
//! [`FinalText`]) deliberately have no `Debug`: dictated text must never
//! reach a log through `{:?}`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Serialize, Serializer};

use crate::orchestrator::error::StageError;

pub const SCHEMA_VERSION: u32 = 1;
pub const MODULE: &str = "orchestrator";

/// Everything one flow execution produced: the envelope fields and, through
/// the error, the CLI's exit code.
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

    /// The one result document (stdout under `--json`).
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

/// A duration in whole milliseconds, saturating.
pub fn millis(duration: Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}

/// The full result document.
#[derive(Serialize)]
pub struct RunEnvelope {
    pub schema_version: u32,
    pub operation_id: String,
    pub module: &'static str,
    pub ok: bool,
    /// Omitted when the arguments never named a usable flow.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flow: Option<String>,
    /// The raw transcription, kept byte for byte; present once transcription
    /// succeeded, also when a later stage failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<Transcript>,
    /// The final text; only on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<FinalText>,
    pub stages: StageReports,
    pub timings_ms: Timings,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<FlowError>,
}

/// The raw transcript exactly as the transcription module returned it.
#[derive(Serialize)]
pub struct Transcript {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub audio_seconds: f64,
}

/// The final text after the formatter and the relay.
#[derive(Serialize)]
pub struct FinalText {
    pub text: String,
}

/// One report per stage, always all four. A `skipped` stage has a null
/// elapsed; an attempted stage is measured through failure or cancellation.
#[derive(Debug, Default, Serialize)]
pub struct StageReports {
    pub prepare: StageReport,
    pub transcription: StageReport,
    pub formatter: StageReport,
    pub relay: StageReport,
}

#[derive(Debug, Default, Serialize)]
pub struct StageReport {
    pub status: StageStatus,
    /// Null for a skipped stage.
    pub elapsed_ms: Option<u64>,
    /// STT detail reported by the transcription child, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_load_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inference_ms: Option<u64>,
}

impl StageReport {
    pub fn measured(status: StageStatus, elapsed: Duration) -> StageReport {
        StageReport {
            status,
            elapsed_ms: Some(millis(elapsed)),
            model_load_ms: None,
            inference_ms: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StageStatus {
    Ok,
    Error,
    Cancelled,
    #[default]
    Skipped,
}

/// Stage names as they appear in `error.stage` and stderr events.
pub mod stage {
    pub const PREPARE: &str = "prepare";
    pub const TRANSCRIPTION: &str = "transcription";
    pub const FORMATTER: &str = "formatter";
    pub const RELAY: &str = "relay";
    pub const CLEANUP: &str = "cleanup";
}

/// Wall-clock timings measured by the orchestrator itself.
#[derive(Debug, Serialize)]
pub struct Timings {
    pub total: u64,
}

/// Where a configuration problem was found.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorLocation {
    #[serde(serialize_with = "serialize_path_lossy")]
    pub file: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u64>,
}

/// A failure with a stable code; `retryable` is always false (a paid call is
/// never replayed automatically).
#[derive(Debug, Serialize)]
pub struct FlowError {
    pub stage: String,
    pub code: String,
    pub message: String,
    pub retryable: bool,
    /// The module's own error code, when one was mapped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<ErrorLocation>,
}

/// Every path in the envelope goes through here: paths are display
/// metadata, so a path that is not valid Unicode (non-UTF-8 bytes on Unix,
/// unpaired surrogates on Windows) is written lossily with U+FFFD instead of
/// failing serialization. No `PathBuf` reaches serde directly.
pub fn serialize_path_lossy<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&path.to_string_lossy())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn non_utf8_paths_serialize_lossily() {
        use std::os::unix::ffi::OsStrExt;
        let location = ErrorLocation {
            file: PathBuf::from(std::ffi::OsStr::from_bytes(b"/cfg/bad\xff.yml")),
            line: Some(1),
            column: None,
        };
        let json = serde_json::to_value(&location).expect("serializes");
        assert_eq!(json["file"], "/cfg/bad\u{fffd}.yml");
        assert_eq!(json["line"], 1);
    }
}
