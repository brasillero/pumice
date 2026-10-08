//! The opt-in debug log (S1.4): one JSON object per completion request,
//! appended to `debug_log.path` as JSONL.
//!
//! Disabled by default: unless `debug_log.enabled: true` in the YAML, the
//! sink is a no-op and no file is created, opened or written. When enabled,
//! every completion records the exchange verbatim — request body, raw text,
//! outcome and response — because that payload is exactly what an
//! investigation needs. Credential-bearing headers never reach the file:
//! only `user-agent` and `content-type` are recorded, and only at
//! [`DebugRecord::new`], the single point where headers enter the log.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use axum::http::HeaderMap;
use serde::Serialize;
use serde_json::Value;

use crate::config::DebugLogSettings;
use crate::pipeline::{AttemptResult, FormatOutcome, OutcomeKind};
use crate::time::{format_rfc3339, now_unix_secs};

/// The JSONL sink for debug records. One mutex-guarded file handle, opened
/// at startup; a runtime write failure is counted and the first is reported
/// once on stderr, never propagated to the request.
pub struct DebugLog {
    inner: Mutex<Option<Sink>>,
}

struct Sink {
    file: File,
    write_failures: u64,
    failure_reported: bool,
}

impl DebugLog {
    /// A disabled sink: every operation is a no-op and no file is touched.
    pub fn disabled() -> DebugLog {
        DebugLog {
            inner: Mutex::new(None),
        }
    }

    /// Opens the sink described by `settings`. A disabled configuration opens
    /// nothing; an enabled one whose file cannot be opened is a startup
    /// error, so a misconfiguration fails loudly before the first dictation.
    pub fn open(settings: &DebugLogSettings) -> Result<DebugLog, DebugLogOpenError> {
        if !settings.enabled {
            return Ok(DebugLog::disabled());
        }
        let file = open_append(&settings.path).map_err(|error| DebugLogOpenError {
            path: settings.path.clone(),
            error,
        })?;
        Ok(DebugLog {
            inner: Mutex::new(Some(Sink {
                file,
                write_failures: 0,
                failure_reported: false,
            })),
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.inner
            .lock()
            .map(|sink| sink.is_some())
            .unwrap_or(false)
    }

    /// Appends one JSON object as a single line. Never fails the caller: a
    /// poisoned mutex or a write error only loses the record.
    pub fn record(&self, record: &DebugRecord<'_>) {
        let Ok(mut guard) = self.inner.lock() else {
            return;
        };
        let Some(sink) = guard.as_mut() else {
            return;
        };
        // The record is a tree of strings and numbers: serialization cannot
        // fail, so the error is not worth a `Result` in the request path.
        let line = serde_json::to_string(record).expect("debug record serializes");
        if let Err(error) = writeln!(sink.file, "{line}") {
            sink.write_failures += 1;
            if !sink.failure_reported {
                sink.failure_reported = true;
                eprintln!("warning: debug log write failed: {error}");
            }
        }
    }
}

/// Opens `path` for appending, creating it (private on Unix) and its parent
/// directories as needed.
fn open_append(path: &Path) -> io::Result<File> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    // The file holds dictated text: readable only by its owner.
    options.mode(0o600);
    let file = options.open(path)?;
    // `mode` only applies when the file is created: an existing file keeps
    // whatever it had, so make it private now. On Windows the file inherits
    // the ACL of its directory (the per-user config directory by default).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

/// Why the debug log file could not be opened at startup.
#[derive(Debug)]
pub struct DebugLogOpenError {
    path: PathBuf,
    error: io::Error,
}

impl fmt::Display for DebugLogOpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot open {}: {}", self.path.display(), self.error)
    }
}

impl std::error::Error for DebugLogOpenError {}

/// One JSONL line of the debug log: the full completion exchange. Only ever
/// constructed when the sink is enabled.
#[derive(Serialize)]
pub struct DebugRecord<'a> {
    pub time: String,
    pub request_id: &'a str,
    /// The parsed request body as received, plus the two recorded headers.
    pub request: Value,
    pub raw_text: &'a str,
    pub outcome: OutcomeSummary,
    pub response_text: &'a str,
}

impl<'a> DebugRecord<'a> {
    pub fn new(
        request_id: &'a str,
        body: Value,
        headers: &HeaderMap,
        raw_text: &'a str,
        outcome: &'a FormatOutcome,
    ) -> DebugRecord<'a> {
        DebugRecord {
            time: format_rfc3339(now_unix_secs()),
            request_id,
            request: redacted_request(body, headers),
            raw_text,
            outcome: OutcomeSummary::of(outcome),
            response_text: &outcome.text,
        }
    }
}

/// Merges the body as received with the only two headers ever recorded.
/// `Authorization`, `Cookie` and any other credential-bearing header are
/// dropped here — the single point where headers enter the log — never to be
/// written at all. A body field named `headers` would be overwritten, so the
/// recorded headers move to `http_headers` in that case.
fn redacted_request(mut body: Value, headers: &HeaderMap) -> Value {
    let Some(object) = body.as_object_mut() else {
        return body;
    };
    let mut recorded = serde_json::Map::new();
    for name in ["user-agent", "content-type"] {
        if let Some(value) = headers.get(name).and_then(|value| value.to_str().ok()) {
            recorded.insert(name.to_owned(), Value::String(value.to_owned()));
        }
    }
    let key = if object.contains_key("headers") {
        "http_headers"
    } else {
        "headers"
    };
    object.insert(key.to_owned(), Value::Object(recorded));
    body
}

/// The safe outcome view recorded in the log: kind, reason, provider,
/// attempts and elapsed time, never text.
#[derive(Serialize)]
pub struct OutcomeSummary {
    pub kind: &'static str,
    /// The raw-fallback reason, if any.
    pub reason: Option<String>,
    pub provider: Option<&'static str>,
    pub attempts: u8,
    pub elapsed_ms: u128,
    /// Every provider started, with what the CLI said when it failed.
    pub trail: Vec<AttemptSummary>,
}

/// One provider attempt in the debug log, including the CLI's stderr and
/// stdout excerpt on failure (which may contain dictated text).
#[derive(Serialize)]
pub struct AttemptSummary {
    pub provider: &'static str,
    pub model: String,
    pub result: String,
    pub elapsed_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtype: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl OutcomeSummary {
    fn of(outcome: &FormatOutcome) -> OutcomeSummary {
        let (kind, reason) = match &outcome.kind {
            OutcomeKind::Formatted => ("formatted", None),
            OutcomeKind::Passthrough => ("passthrough", None),
            OutcomeKind::Inspect => ("inspect", None),
            OutcomeKind::Empty => ("empty", None),
            OutcomeKind::Raw(reason) => ("raw", Some(format!("{reason:?}"))),
        };
        OutcomeSummary {
            kind,
            reason,
            provider: outcome.provider,
            attempts: outcome.attempts,
            elapsed_ms: outcome.elapsed.as_millis(),
            trail: outcome
                .trail
                .iter()
                .map(|attempt| {
                    let diagnostic = attempt.diagnostic.as_ref();
                    AttemptSummary {
                        provider: attempt.provider,
                        model: attempt.model.clone(),
                        result: match attempt.result {
                            AttemptResult::Formatted => "formatted".to_owned(),
                            AttemptResult::Failed(error) => format!("{error:?}"),
                            AttemptResult::CleanupRejected(error) => {
                                format!("cleanup_rejected: {error}")
                            }
                        },
                        elapsed_ms: attempt.elapsed.as_millis(),
                        exit_code: diagnostic.and_then(|d| d.exit_code),
                        api_status: diagnostic.and_then(|d| d.api_status),
                        subtype: diagnostic.and_then(|d| d.subtype.clone()),
                        detail: diagnostic
                            .map(|d| d.detail.clone())
                            .filter(|detail| !detail.is_empty()),
                    }
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn enabled_settings(path: &Path) -> DebugLogSettings {
        DebugLogSettings {
            enabled: true,
            path: path.to_path_buf(),
        }
    }

    #[test]
    fn disabled_opens_nothing_and_records_nothing() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("debug.jsonl");
        let log = DebugLog::open(&DebugLogSettings {
            enabled: false,
            path: path.clone(),
        })
        .expect("disabled sink opens");
        assert!(!log.is_enabled());
        assert!(!path.exists(), "a disabled sink must not create the file");
    }

    #[test]
    fn open_creates_parent_directories_and_appends() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("nested").join("debug.jsonl");
        let log = DebugLog::open(&enabled_settings(&path)).expect("sink opens");
        assert!(log.is_enabled());
        assert!(path.is_file(), "the file and its parents are created");

        let outcome = crate::pipeline::FormatOutcome {
            text: "formatted text".to_owned(),
            kind: OutcomeKind::Formatted,
            provider: Some("claude"),
            attempts: 1,
            trail: Vec::new(),
            elapsed: std::time::Duration::from_millis(7),
        };
        let headers = HeaderMap::new();
        let body = serde_json::json!({"messages": [{"role": "user", "content": "oi"}]});
        let record = DebugRecord::new("chatcmpl-pumice-1", body, &headers, "oi", &outcome);
        log.record(&record);

        let reopened = DebugLog::open(&enabled_settings(&path)).expect("sink reopens");
        reopened.record(&record);
        let contents = std::fs::read_to_string(&path).expect("log readable");
        assert_eq!(
            contents.lines().count(),
            2,
            "reopening appends instead of truncating"
        );
    }

    #[test]
    fn record_keeps_only_user_agent_and_content_type() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("debug.jsonl");
        let log = DebugLog::open(&enabled_settings(&path)).expect("sink opens");

        let mut headers = HeaderMap::new();
        headers.insert("user-agent", "pumice-test".parse().unwrap());
        headers.insert("content-type", "application/json".parse().unwrap());
        headers.insert("authorization", "Bearer secret-token".parse().unwrap());
        headers.insert("cookie", "session=abc".parse().unwrap());
        let body = serde_json::json!({"messages": [{"role": "user", "content": "dictation"}]});
        let outcome = crate::pipeline::FormatOutcome {
            text: "dictation".to_owned(),
            kind: OutcomeKind::Formatted,
            provider: Some("claude"),
            attempts: 1,
            trail: Vec::new(),
            elapsed: std::time::Duration::from_millis(3),
        };
        log.record(&DebugRecord::new(
            "chatcmpl-pumice-2",
            body,
            &headers,
            "dictation",
            &outcome,
        ));

        let contents = std::fs::read_to_string(&path).expect("log readable");
        assert!(
            !contents.contains("secret-token"),
            "authorization leaked: {contents}"
        );
        assert!(
            !contents.contains("session=abc"),
            "cookie leaked: {contents}"
        );
        let line: Value = serde_json::from_str(contents.lines().next().unwrap()).unwrap();
        assert_eq!(line["request"]["headers"]["user-agent"], "pumice-test");
        assert_eq!(
            line["request"]["headers"]["content-type"],
            "application/json"
        );
        assert_eq!(line["request"]["headers"]["authorization"], Value::Null);
        assert_eq!(line["raw_text"], "dictation");
        assert_eq!(line["response_text"], "dictation");
        assert_eq!(line["outcome"]["kind"], "formatted");
        assert_eq!(line["outcome"]["provider"], "claude");
        assert_eq!(line["outcome"]["attempts"], 1);
        assert_eq!(line["outcome"]["reason"], Value::Null);
        assert_eq!(line["outcome"]["elapsed_ms"], 3);
    }

    #[test]
    fn body_field_named_headers_is_not_overwritten() {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", "agent".parse().unwrap());
        let body = serde_json::json!({"headers": {"note": "client field"}, "messages": []});
        let merged = redacted_request(body, &headers);
        assert_eq!(merged["headers"]["note"], "client field");
        assert_eq!(merged["http_headers"]["user-agent"], "agent");
    }

    #[cfg(unix)]
    #[test]
    fn created_file_is_owner_readable_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("debug.jsonl");
        let _log = DebugLog::open(&enabled_settings(&path)).expect("sink opens");
        let mode = std::fs::metadata(&path)
            .expect("log metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "the log holds dictated text: mode {mode:o}");
    }

    #[cfg(unix)]
    #[test]
    fn existing_file_is_made_owner_readable_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("debug.jsonl");
        std::fs::write(&path, "").expect("pre-create");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        let _log = DebugLog::open(&enabled_settings(&path)).expect("sink opens");
        let mode = std::fs::metadata(&path)
            .expect("log metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "an existing log is made private: mode {mode:o}"
        );
    }
}
