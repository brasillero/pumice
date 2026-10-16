//! The `pumice run` result envelope (p12-pumice-run/D7): one
//! `schema_version: 1` JSON document on stdout under `--json`, with the raw
//! transcript and the final text as separate fields.
//!
//! [`RunOutput`] is what one flow execution produced, independent of how it
//! is presented: the CLI writes it as this envelope or as plain text, and
//! `pumice serve`'s dictation route writes the same envelope with an HTTP
//! status and an error object that also carries the OpenAI error fields
//! ([`RunOutput::into_http_envelope`], p13.1-flow-seam/D1, D6). Both read
//! the same [`StageError`]: the CLI its exit code, HTTP [`http_class`].
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
    /// The final teardown left a cleanup unconfirmed (a provider tree not
    /// reaped, a temporary directory not removed), also when `error` is
    /// another, primary failure. Never presented; a service acts on it.
    pub cleanup_failed: bool,
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

    /// The same document for an HTTP answer, with its status: `200` on
    /// success, otherwise the status [`http_class`] picks for the error.
    /// The error object gains the OpenAI fields (`type`, `param`) and loses
    /// the configuration `location`: an HTTP client never sees server
    /// paths, and a located message is replaced by a fixed one.
    pub fn into_http_envelope(self) -> (u16, RunEnvelope<HttpFlowError>) {
        let (status, error) = match self.error.as_ref().map(http_error) {
            Some((class, error)) => (class.status, Some(error)),
            None => (200, None),
        };
        let envelope = RunEnvelope {
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
            error,
        };
        (status, envelope)
    }
}

/// An error's HTTP class and its HTTP error object.
pub fn http_error(error: &StageError) -> (HttpClass, HttpFlowError) {
    let class = http_class(error);
    let object = HttpFlowError {
        message: http_message(error),
        kind: class.kind,
        param: error.param,
        code: error.code,
        stage: error.stage,
        retryable: false,
        cause_code: error.cause_code.as_deref().map(str::to_owned),
    };
    (class, object)
}

/// How an error is answered over HTTP (design §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HttpClass {
    pub status: u16,
    /// The OpenAI error `type`.
    pub kind: &'static str,
}

/// The HTTP status and OpenAI error type for a flow error, shared by every
/// HTTP presentation; the CLI uses the same error's exit code instead.
///
/// 400/403/404/405/408/413 are the client's request (`invalid_request_error`),
/// 429 a busy service or a provider limit (`rate_limit_error`), everything
/// else a `server_error`: 503 when a dependency is not available (starting,
/// restarting, stopping, the provider's CLI missing or logged out), 502
/// when the transcription child or the provider failed, 504 on a deadline,
/// 500 for the server's own failures (configuration, engine, cleanup).
pub fn http_class(error: &StageError) -> HttpClass {
    const REQUEST: &str = "invalid_request_error";
    const SERVER: &str = "server_error";
    let cause = error.cause_code.as_deref();
    let (status, kind) = match error.code {
        // The request itself.
        "not_multipart"
        | "malformed_multipart"
        | "unsupported_parameter"
        | "duplicate_parameter"
        | "missing_file"
        | "missing_flow"
        | "empty_file"
        | "file_not_upload"
        | "flow_not_text"
        | "invalid_utf8"
        | "nul_in_text"
        | "flow_disabled" => (400, REQUEST),
        "host_not_allowed" | "origin_not_allowed" | "cross_site_not_allowed" => (403, REQUEST),
        "flow_not_found" | "not_found" => (404, REQUEST),
        "method_not_allowed" => (405, REQUEST),
        "upload_timeout" => (408, REQUEST),
        "body_too_large" | "file_too_large" | "field_too_large" => (413, REQUEST),
        "transcription_invalid_input" => match cause {
            Some("input_too_large") => (413, REQUEST),
            _ => (400, REQUEST),
        },
        // Admission and availability.
        "rate_limit_exceeded" => (429, "rate_limit_error"),
        "relay_quota_exceeded" | "relay_rate_limited" => (429, "rate_limit_error"),
        "service_starting"
        | "transcription_restarting"
        | "transcription_failed"
        | "shutting_down"
        | "admission_stopped"
        | "transcription_busy"
        | "relay_not_installed"
        | "relay_not_logged_in" => (503, SERVER),
        "relay_failed" if cause == Some("authentication_rejected") => (503, SERVER),
        "cancelled" => match cause {
            // Nobody reads it: the client is gone. Never a fictional 499.
            Some("client_disconnected") => (400, REQUEST),
            _ => (503, SERVER),
        },
        // A dependency failed.
        "transcription_unavailable"
        | "transcription_protocol_error"
        | "relay_failed"
        | "relay_invalid_output" => (502, SERVER),
        // Deadlines.
        "flow_timeout" | "transcription_timeout" | "formatter_timeout" | "relay_timeout" => {
            (504, SERVER)
        }
        // The server's own failures: engine, configuration, cleanup, bugs.
        _ => (500, SERVER),
    };
    HttpClass { status, kind }
}

/// The error message an HTTP client sees: the error's own fixed text as a
/// sentence, or a fixed one when that text names a configuration location
/// (a server path).
fn http_message(error: &StageError) -> String {
    if error.location.is_some() {
        return "The server configuration is invalid; see the pumice serve output.".to_owned();
    }
    let mut message = String::with_capacity(error.message.len() + 1);
    let mut chars = error.message.chars();
    if let Some(first) = chars.next() {
        message.extend(first.to_uppercase());
        message.push_str(chars.as_str());
    }
    if !message.ends_with('.') {
        message.push('.');
    }
    message
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

/// The full result document; `E` is the error object (the CLI's
/// [`FlowError`] or HTTP's [`HttpFlowError`]).
#[derive(Serialize)]
pub struct RunEnvelope<E = FlowError> {
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
    pub error: Option<E>,
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

/// [`FlowError`] for an HTTP answer: the OpenAI error object (`message`,
/// `type`, `param`, `code`) plus the flow's `stage`, `retryable` (always
/// false) and `cause_code`. No `location`: HTTP clients never see server
/// paths.
#[derive(Debug, Serialize)]
pub struct HttpFlowError {
    pub message: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub param: Option<&'static str>,
    pub code: &'static str,
    pub stage: &'static str,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause_code: Option<String>,
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
