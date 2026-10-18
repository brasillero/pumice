//! The dictation route's transcription transport (p13.1-flow-seam/D4, D5):
//! the in-memory upload goes to the ready child's managed
//! `POST /pumice/v1/transcriptions` on its loopback port.
//!
//! The request is rebuilt, never forwarded: a fresh multipart form with
//! the flow's model, the request's [`TranscriptionOptions`] (none for a
//! dictation; `language`, `prompt` and `temperature` for an OpenAI
//! transcription, p15-serve-transcriptions/D6) and the uploaded bytes
//! unchanged, under a fixed file name (the client's is never passed on),
//! the literal `Host`
//! `127.0.0.1:<port>`, no `Origin`, no redirects and no proxy (hyper's
//! client connection has neither). The answer is read up to
//! [`MAX_RESULT_BYTES`] and checked strictly against the detailed result
//! schema; an error answer keeps only its stable code, mapped to the
//! [`SttFailure`] the flow already knows. The child's own messages are
//! never kept.
//!
//! A transcription abandoned after it was sent (deadline, cancellation)
//! leaves the child busy with work nobody waits for: the transport records
//! the generation, and the route recycles it (D5). Audio is never written
//! to disk.

use std::collections::VecDeque;
use std::net::Ipv4Addr;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body_util::{BodyExt, Limited};
use hyper::body::{Body, Frame, SizeHint};
use serde::Deserialize;
use tokio::time::Instant;

use crate::orchestrator::cancel::{self, CancelGuard};
use crate::orchestrator::flow::{AudioSource, Transcriber};
use crate::orchestrator::serve::supervisor::SupervisorHandle;
use crate::orchestrator::transcription::{SttFailure, TranscriptOutput};

/// The child's detailed result (and any error object) is at most this.
pub const MAX_RESULT_BYTES: usize = 1024 * 1024;
/// The fixed file name sent to the child: the format is detected from the
/// bytes, and the client's own name is never passed on.
const FILE_NAME: &str = "audio";
/// The child's managed route.
const PATH: &str = "/pumice/v1/transcriptions";

/// What one request asks of the transcription itself, already validated
/// by the route: forwarded as the child's own form fields, never to the AI.
/// The default (a dictation) sends none, so the child's defaults apply. No
/// `Debug`: `prompt` is the user's text.
#[derive(Clone, Default)]
pub struct TranscriptionOptions {
    /// A trimmed ISO-639-1 code; the child checks it against its list.
    pub language: Option<String>,
    /// Whisper's initial prompt.
    pub prompt: Option<String>,
    /// Between 0 and 1.
    pub temperature: Option<f32>,
}

/// Sends one operation's audio to the ready child, once.
pub struct WarmTranscriber {
    supervisor: SupervisorHandle,
    /// Fixed for the operation's life.
    options: TranscriptionOptions,
    /// The generation a request was sent to and then abandoned.
    abandoned: Mutex<Option<u64>>,
}

impl WarmTranscriber {
    /// A dictation's transcriber: no per-request options.
    pub fn new(supervisor: SupervisorHandle) -> WarmTranscriber {
        WarmTranscriber::with_options(supervisor, TranscriptionOptions::default())
    }

    pub fn with_options(
        supervisor: SupervisorHandle,
        options: TranscriptionOptions,
    ) -> WarmTranscriber {
        WarmTranscriber {
            supervisor,
            options,
            abandoned: Mutex::new(None),
        }
    }

    /// The generation left working on an abandoned transcription, which
    /// must be recycled (D5); `None` when nothing was abandoned.
    pub fn abandoned(&self) -> Option<u64> {
        *self
            .abandoned
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn abandon(&self, generation: u64) {
        *self
            .abandoned
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(generation);
    }
}

impl Transcriber for WarmTranscriber {
    async fn transcribe(
        &self,
        audio: &AudioSource,
        model: &str,
        deadline: Instant,
        cancel: &CancelGuard,
    ) -> Result<TranscriptOutput, SttFailure> {
        let AudioSource::Bytes(file) = audio else {
            return Err(SttFailure::Protocol(
                "the warm transcription child takes in-memory audio only",
            ));
        };
        if let Some(reason) = cancel::current(cancel) {
            return Err(SttFailure::Cancelled(reason));
        }
        if Instant::now() >= deadline {
            return Err(SttFailure::Timeout);
        }
        let Some(child) = self.supervisor.ready_child() else {
            return Err(SttFailure::Unavailable(
                "the transcription module is not ready",
            ));
        };

        let dispatched = AtomicBool::new(false);
        let mut cancelled = cancel.clone();
        // Leaving the select drops the call, which closes the connection;
        // the child may still be transcribing what it received.
        let outcome = tokio::select! {
            biased;
            reason = cancel::cancelled(&mut cancelled) => Err(SttFailure::Cancelled(reason)),
            () = tokio::time::sleep_until(deadline) => Err(SttFailure::Timeout),
            outcome = send(child.port, model, &self.options, file.clone(), &dispatched) => outcome,
        };
        let dispatched = dispatched.load(Ordering::Acquire);
        match outcome {
            Err(SttFailure::Cancelled(_) | SttFailure::Timeout) if dispatched => {
                self.abandon(child.generation);
            }
            // An explicit cancellation wins over the I/O failure it induced.
            Err(SttFailure::Unavailable(_)) if cancel::current(cancel).is_some() => {
                if dispatched {
                    self.abandon(child.generation);
                }
                let reason = cancel::current(cancel).expect("checked");
                return Err(SttFailure::Cancelled(reason));
            }
            _ => {}
        }
        outcome
    }
}

/// One request to the child; `dispatched` turns true once the request
/// may have reached it.
async fn send(
    port: u16,
    model: &str,
    options: &TranscriptionOptions,
    file: Bytes,
    dispatched: &AtomicBool,
) -> Result<TranscriptOutput, SttFailure> {
    const UNREACHABLE: SttFailure =
        SttFailure::Unavailable("the transcription module could not be reached");
    let stream = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        .await
        .map_err(|_| UNREACHABLE)?;
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream))
            .await
            .map_err(|_| UNREACHABLE)?;
    let _driver = DriverGuard(tokio::spawn(async move {
        let _ = connection.await;
    }));

    let form = Form::new(model, options, file);
    let request = hyper::Request::post(PATH)
        .header(hyper::header::HOST, format!("127.0.0.1:{port}"))
        .header(hyper::header::CONTENT_TYPE, form.content_type())
        .header(hyper::header::CONTENT_LENGTH, form.len())
        .header(hyper::header::CONNECTION, "close")
        .body(form)
        .map_err(|_| SttFailure::Protocol("the transcription request could not be built"))?;
    dispatched.store(true, Ordering::Release);
    let response = sender
        .send_request(request)
        .await
        .map_err(|_| SttFailure::Unavailable("the transcription module closed the connection"))?;
    let status = response.status();
    let body = Limited::new(response.into_body(), MAX_RESULT_BYTES)
        .collect()
        .await
        .map_err(|error| {
            if error.is::<http_body_util::LengthLimitError>() {
                SttFailure::Protocol("the transcription module returned too much output")
            } else {
                SttFailure::Unavailable("the transcription module closed the connection")
            }
        })?
        .to_bytes();
    if status == hyper::StatusCode::OK {
        parse_result(&body)
    } else {
        Err(map_error(status, &body))
    }
}

/// The detailed result, exactly as documented (docs/transcription.md,
/// "Managed mode"); no `Debug`: `text` is the transcript.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Detailed {
    schema_version: u32,
    text: String,
    #[serde(default)]
    language: Option<String>,
    audio_seconds: f64,
    inference_ms: u64,
}

fn parse_result(body: &[u8]) -> Result<TranscriptOutput, SttFailure> {
    let Ok(result) = serde_json::from_slice::<Detailed>(body) else {
        return Err(SttFailure::Protocol(
            "the transcription module's result has an unexpected shape",
        ));
    };
    if result.schema_version != 1 {
        return Err(SttFailure::Protocol(
            "the transcription module answered with an unsupported result version",
        ));
    }
    if !result.audio_seconds.is_finite() || result.audio_seconds < 0.0 {
        return Err(SttFailure::Protocol(
            "the transcription module reported an invalid audio duration",
        ));
    }
    Ok(TranscriptOutput {
        text: result.text,
        language: result.language,
        audio_seconds: result.audio_seconds,
        // A warm child loaded its model once, at startup (in status).
        model_load_ms: None,
        transcription_ms: Some(result.inference_ms),
    })
}

#[derive(Deserialize)]
struct ErrorBody {
    error: ErrorObject,
}

/// Only the stable code is read; the message is never kept.
#[derive(Deserialize)]
struct ErrorObject {
    code: Option<String>,
}

/// A child error answer, by its status and stable code. Codes that can
/// only mean this transport built a bad request are protocol errors.
fn map_error(status: hyper::StatusCode, body: &[u8]) -> SttFailure {
    let code = serde_json::from_slice::<ErrorBody>(body)
        .ok()
        .and_then(|body| body.error.code);
    let module = |code: &str| SttFailure::Module {
        code: code.to_owned(),
        location: None,
    };
    match (status.as_u16(), code.as_deref()) {
        (400, Some("invalid_audio")) => module("invalid_input"),
        (400, Some(code @ ("unsupported_format" | "audio_too_long"))) => module(code),
        // Only a forwarded `language` can be refused this way: the route
        // checks its shape, the child its list.
        (400, Some("invalid_language")) => module("invalid_language"),
        (413, Some("file_too_large" | "body_too_large")) => module("input_too_large"),
        (500, Some("transcription_failed")) => module("engine_failed"),
        (504, Some("deadline_exceeded")) => SttFailure::Timeout,
        (429, Some("rate_limit_exceeded")) => SttFailure::Busy,
        (503, Some("shutting_down")) => {
            SttFailure::Unavailable("the transcription module is shutting down")
        }
        _ => SttFailure::Protocol("the transcription module answered with an unexpected error"),
    }
}

/// The internal multipart form: `model`, the options that are set, then
/// `file` with the uploaded bytes unchanged. Sent as three chunks, so the
/// audio is never copied.
pub struct Form {
    boundary: String,
    chunks: VecDeque<Bytes>,
    len: u64,
}

impl Form {
    pub fn new(model: &str, options: &TranscriptionOptions, file: Bytes) -> Form {
        let temperature = options.temperature.map(|value| value.to_string());
        let fields = [
            ("model", Some(model)),
            ("language", options.language.as_deref()),
            ("prompt", options.prompt.as_deref()),
            ("temperature", temperature.as_deref()),
        ];
        // The boundary occurs in no value: a prompt is the client's text.
        let mut parts: Vec<&[u8]> = vec![&file];
        parts.extend(
            fields
                .iter()
                .filter_map(|(_, value)| value.map(str::as_bytes)),
        );
        let boundary = boundary_for(&parts);
        let mut head = String::new();
        for (name, value) in fields {
            if let Some(value) = value {
                head.push_str(&format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
                ));
            }
        }
        head.push_str(&format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{FILE_NAME}\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n"
        ));
        let tail = format!("\r\n--{boundary}--\r\n");
        let chunks = VecDeque::from([Bytes::from(head), file, Bytes::from(tail)]);
        let len = chunks.iter().map(|chunk| chunk.len() as u64).sum();
        Form {
            boundary,
            chunks,
            len,
        }
    }

    pub fn content_type(&self) -> String {
        format!("multipart/form-data; boundary={}", self.boundary)
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Body for Form {
    type Data = Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        let next = self.chunks.pop_front();
        if let Some(chunk) = &next {
            self.len -= chunk.len() as u64;
        }
        Poll::Ready(next.map(|chunk| Ok(Frame::data(chunk))))
    }

    fn is_end_stream(&self) -> bool {
        self.chunks.is_empty()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.len)
    }
}

/// A boundary that occurs in none of the parts (a delimiter is
/// `--boundary`; one inside the data would end the part early).
fn boundary_for(parts: &[&[u8]]) -> String {
    let mut attempt = 0u32;
    loop {
        let boundary = format!(
            "pumice-{}-{attempt}",
            crate::orchestrator::events::operation_id()
        );
        let delimiter = format!("--{boundary}");
        if !parts
            .iter()
            .any(|part| contains(part, delimiter.as_bytes()))
        {
            return boundary;
        }
        attempt += 1;
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window[0] == needle[0] && window == needle)
}

/// Stops the connection driver when the call ends or is dropped.
struct DriverGuard(tokio::task::JoinHandle<()>);

impl Drop for DriverGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_keeps_the_bytes_and_drops_the_file_name() {
        let audio = Bytes::from_static(b"RIFF\x00\x01--not-a-boundary\r\n\xff");
        let form = Form::new("turbo", &TranscriptionOptions::default(), audio.clone());
        let boundary = form.boundary.clone();
        let total = form.len();
        let joined: Vec<u8> = form.chunks.iter().flat_map(|c| c.to_vec()).collect();
        assert_eq!(joined.len() as u64, total);
        let text = String::from_utf8_lossy(&joined);
        assert!(text.starts_with(&format!("--{boundary}\r\n")));
        assert!(text.contains("name=\"model\"\r\n\r\nturbo\r\n"));
        assert!(text.contains("filename=\"audio\""));
        assert!(text.ends_with(&format!("\r\n--{boundary}--\r\n")));
        let start = joined
            .windows(4)
            .position(|w| w == b"RIFF")
            .expect("the audio is in the form");
        assert_eq!(&joined[start..start + audio.len()], &audio[..]);
        for absent in ["language", "prompt", "temperature"] {
            assert!(!text.contains(absent), "a dictation sends no {absent}");
        }
    }

    #[test]
    fn options_are_sent_as_their_own_fields() {
        let audio = Bytes::from_static(b"RIFF audio");
        let options = TranscriptionOptions {
            language: Some("pt".to_owned()),
            prompt: Some("Olá, \"Pumice\"\r\nnext line".to_owned()),
            temperature: Some(0.25),
        };
        let form = Form::new("turbo", &options, audio);
        let boundary = form.boundary.clone();
        let joined: Vec<u8> = form.chunks.iter().flat_map(|c| c.to_vec()).collect();
        let text = String::from_utf8(joined).unwrap();
        let field = |name: &str, value: &str| {
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
        };
        let expected = [
            field("model", "turbo"),
            field("language", "pt"),
            field("prompt", "Olá, \"Pumice\"\r\nnext line"),
            field("temperature", "0.25"),
        ]
        .concat();
        assert!(text.starts_with(&expected), "{text}");
        assert!(text.contains("filename=\"audio\""));

        // Only the options that are set.
        let options = TranscriptionOptions {
            temperature: Some(0.0),
            ..TranscriptionOptions::default()
        };
        let form = Form::new("turbo", &options, Bytes::from_static(b"x"));
        let joined: Vec<u8> = form.chunks.iter().flat_map(|c| c.to_vec()).collect();
        let text = String::from_utf8(joined).unwrap();
        assert!(text.contains("name=\"temperature\"\r\n\r\n0\r\n"), "{text}");
        assert!(!text.contains("language") && !text.contains("prompt"));
    }

    #[test]
    fn the_boundary_never_occurs_in_the_file() {
        // A file that happens to contain a boundary-looking line still gets
        // one that does not occur in it.
        let probe = boundary_for(&[b""]);
        let file = format!("x--{probe}y");
        let boundary = boundary_for(&[file.as_bytes()]);
        assert!(!contains(
            file.as_bytes(),
            format!("--{boundary}").as_bytes()
        ));
        // Any part counts, a prompt included.
        let probe = boundary_for(&[b""]);
        let prompt = format!("--{probe}");
        let boundary = boundary_for(&[b"audio", prompt.as_bytes()]);
        assert!(!contains(
            prompt.as_bytes(),
            format!("--{boundary}").as_bytes()
        ));
        assert!(contains(b"abc--def", b"--d"));
        assert!(!contains(b"ab", b"abc"));
    }

    #[test]
    fn detailed_results_are_checked_strictly() {
        let ok = parse_result(
            br#"{"schema_version":1,"text":" hi","language":"pt","audio_seconds":2.5,"inference_ms":40}"#,
        )
        .unwrap_or_else(|_| panic!("valid"));
        assert_eq!(ok.text, " hi");
        assert_eq!(ok.language.as_deref(), Some("pt"));
        assert_eq!(ok.transcription_ms, Some(40));
        assert_eq!(ok.model_load_ms, None);
        // Without a language.
        assert!(
            parse_result(br#"{"schema_version":1,"text":"","audio_seconds":0.0,"inference_ms":0}"#)
                .is_ok()
        );
        for bad in [
            &br#"{"schema_version":2,"text":"x","audio_seconds":1,"inference_ms":1}"#[..],
            br#"{"schema_version":1,"text":"x","audio_seconds":-1,"inference_ms":1}"#,
            br#"{"schema_version":1,"text":"x","audio_seconds":1}"#,
            br#"{"schema_version":1,"text":"x","audio_seconds":1,"inference_ms":1,"extra":1}"#,
            br#"{"text":"x"}"#,
            b"not json",
        ] {
            assert!(
                matches!(parse_result(bad), Err(SttFailure::Protocol(_))),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn child_errors_map_by_status_and_code_only() {
        let body = |code: &str| {
            format!(
                r#"{{"error":{{"message":"S3NT1NEL","type":"x","param":null,"code":"{code}"}}}}"#
            )
        };
        let module = |status: u16, code: &str| match map_error(
            hyper::StatusCode::from_u16(status).unwrap(),
            body(code).as_bytes(),
        ) {
            SttFailure::Module { code, location } => {
                assert!(location.is_none());
                Some(code)
            }
            _ => None,
        };
        assert_eq!(
            module(400, "invalid_audio").as_deref(),
            Some("invalid_input")
        );
        assert_eq!(
            module(400, "unsupported_format").as_deref(),
            Some("unsupported_format")
        );
        assert_eq!(
            module(400, "audio_too_long").as_deref(),
            Some("audio_too_long")
        );
        assert_eq!(
            module(413, "file_too_large").as_deref(),
            Some("input_too_large")
        );
        assert_eq!(
            module(400, "invalid_language").as_deref(),
            Some("invalid_language")
        );
        assert_eq!(
            module(500, "transcription_failed").as_deref(),
            Some("engine_failed")
        );
        let other = |status: u16, code: &str| {
            map_error(
                hyper::StatusCode::from_u16(status).unwrap(),
                body(code).as_bytes(),
            )
        };
        assert!(matches!(
            other(504, "deadline_exceeded"),
            SttFailure::Timeout
        ));
        assert!(matches!(
            other(429, "rate_limit_exceeded"),
            SttFailure::Busy
        ));
        assert!(matches!(
            other(503, "shutting_down"),
            SttFailure::Unavailable(_)
        ));
        // Codes only a broken request could cause, a mismatched status or no
        // JSON at all: protocol errors.
        for (status, code) in [
            (400, "missing_file"),
            (400, "malformed_multipart"),
            (404, "model_not_found"),
            (403, "host_not_allowed"),
            (500, "invalid_audio"),
            (418, "teapot"),
        ] {
            assert!(
                matches!(other(status, code), SttFailure::Protocol(_)),
                "{status} {code}"
            );
        }
        assert!(matches!(
            map_error(hyper::StatusCode::BAD_GATEWAY, b"<html>"),
            SttFailure::Protocol(_)
        ));
    }
}
