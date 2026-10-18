//! `pumice serve`'s OpenAI routes (p15-serve-transcriptions/D1, D5, D6):
//! `GET /v1/models`, `GET /v1/models/{id}` and
//! `POST /v1/audio/transcriptions`.
//!
//! A model is a flow published in `serve.transcription_flows`; its id is the
//! flow id. A transcription runs that flow through the dictation route's
//! machinery ([`dictation::admit`]): the same permit (busy 429 with
//! `Retry-After: 1`, unready 503), the same upload limits, deadlines,
//! cancellation, cleanup and privacy. `language`, `prompt` and
//! `temperature` go to the transcription child only, never to the AI.
//!
//! The answer's `text` is the final text. When the transcription succeeded
//! and anything after it failed (the formatter, the relay, a shutdown or
//! the cleanup), the answer is still a success, with the raw transcript as
//! `text` and the failure only in the Pumice details (D4, D9). Every other failure is an OpenAI error object with
//! an OpenAI status. Pumice details: a `pumice` object next to `text` in
//! `json`, the `Pumice-Postprocessing-Status` header in both formats.
//!
//! The model routes read in-memory snapshots only: no network, model or
//! provider work, and no wait for the permit.

use std::sync::Arc;

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::header::{self, HeaderValue};
use hyper::{Request, Response, StatusCode};
use serde_json::{Value, json};
use tokio::time::Instant;

use crate::orchestrator::cancel::CancelGuard;
use crate::orchestrator::config;
use crate::orchestrator::error::StageError;
use crate::orchestrator::flow::AudioSource;
use crate::orchestrator::result::{
    RunOutput, SCHEMA_VERSION, StageStatus, Timings, http_error, millis, stage,
};
use crate::orchestrator::serve::dictation::{
    self, Admitted, Dictations, Endpoint, MAX_BODY_BYTES, MAX_FILE_BYTES, MAX_RESPONSE_BYTES,
    MAX_SCALAR_BYTES, Once, request_error,
};
use crate::orchestrator::serve::http::{FlowInfo, Frontend, json_bytes_response};
use crate::orchestrator::serve::warm::TranscriptionOptions;

/// The transcription route.
pub const TRANSCRIPTIONS: &str = "/v1/audio/transcriptions";
/// The model list; one model is `MODELS` + `/` + its id.
pub const MODELS: &str = "/v1/models";
/// The post-processing status header, in both response formats.
pub const STATUS_HEADER: &str = "pumice-postprocessing-status";
const RESPONSE_FORMATS: [&str; 2] = ["json", "text"];

/// The route's handler; the guard and the method were checked already.
pub async fn handle(
    frontend: Arc<Frontend>,
    request: Request<Incoming>,
    arrival: Instant,
) -> Response<Full<Bytes>> {
    dictation::admit(frontend, request, arrival, Endpoint::Transcription).await
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseFormat {
    Json,
    Text,
}

/// How a transcription answers, known once its request was read.
pub struct Answering {
    format: ResponseFormat,
    /// The flow's formatter (`None`: none).
    preset: Option<String>,
    relay_route: String,
}

impl Answering {
    /// An off flow: `formatter: null` with the passthrough route (D5).
    fn off(&self) -> bool {
        self.preset.is_none() && config::is_passthrough(&self.relay_route)
    }
}

/// The `prepare` stage: the bounded upload, the form and the published
/// flow its `model` names.
pub(crate) async fn prepare(
    dictations: &Dictations,
    request: Request<Incoming>,
    arrival: Instant,
    cancel: &CancelGuard,
) -> Result<Admitted, StageError> {
    let (body, boundary) = dictation::read_upload(dictations, request, arrival, cancel).await?;
    let form = parse_form(body, boundary).await?;
    // Exact, case-sensitive; an unpublished flow is no model at all.
    let flow = dictations
        .transcription_flows
        .iter()
        .find(|id| **id == form.model)
        .and_then(|id| dictations.flows.iter().find(|flow| flow.id == *id))
        .filter(|flow| flow.enabled)
        .ok_or_else(model_not_found)?
        .clone();
    let answering = Answering {
        format: form.format,
        preset: flow.formatter.clone(),
        relay_route: flow.relay_route.clone(),
    };
    Ok(Admitted {
        flow: dictation::anchor(flow, arrival)?,
        audio: AudioSource::Bytes(form.file),
        options: form.options,
        answering: Some(answering),
    })
}

fn model_not_found() -> StageError {
    request_error(
        "model_not_found",
        "The requested model does not exist on this server.",
        Some("model"),
    )
}

/// A parsed request. No `Debug`: `file` is the user's audio and the
/// options carry their prompt.
struct Form {
    file: Bytes,
    model: String,
    options: TranscriptionOptions,
    format: ResponseFormat,
}

/// The accepted multipart parameters (D6). Unknown names are never echoed.
fn known_param(name: &str) -> Option<&'static str> {
    Some(match name {
        "file" => "file",
        "model" => "model",
        "language" => "language",
        "prompt" => "prompt",
        "temperature" => "temperature",
        "response_format" => "response_format",
        "stream" => "stream",
        _ => return None,
    })
}

/// Exactly one non-empty uploaded `file` and the text fields of D6, each at
/// most once, in any order; the same limits as the transcription module's
/// own OpenAI route. No message quotes a field name, file name or value
/// the client sent.
async fn parse_form(body: Bytes, boundary: String) -> Result<Form, StageError> {
    let limits = multer::SizeLimit::new()
        .whole_stream(MAX_BODY_BYTES)
        .per_field(MAX_SCALAR_BYTES)
        .for_field("file", MAX_FILE_BYTES);
    let constraints = multer::Constraints::new().size_limit(limits);
    let mut multipart =
        multer::Multipart::with_constraints(Once(Some(body)), boundary, constraints);

    let mut seen: Vec<&'static str> = Vec::new();
    let mut file = None;
    let mut model = None;
    let mut options = TranscriptionOptions::default();
    let mut format = ResponseFormat::Json;
    while let Some(field) = multipart.next_field().await.map_err(multer_error)? {
        let upload = field.file_name().is_some();
        let Some(param) = field.name().and_then(known_param) else {
            return Err(request_error(
                "unsupported_parameter",
                "Unrecognized or unsupported request parameter. Supported: file, model, language, prompt, response_format, temperature, stream.",
                None,
            ));
        };
        if seen.contains(&param) {
            return Err(request_error(
                "duplicate_parameter",
                "A request parameter was sent more than once.",
                Some(param),
            ));
        }
        seen.push(param);

        if param == "file" {
            if !upload {
                return Err(request_error(
                    "file_not_upload",
                    "The file parameter must be sent as a file upload.",
                    Some("file"),
                ));
            }
            let data = field.bytes().await.map_err(multer_error)?;
            if data.is_empty() {
                return Err(request_error(
                    "empty_file",
                    "The uploaded file is empty.",
                    Some("file"),
                ));
            }
            file = Some(data);
            continue;
        }
        if upload {
            return Err(request_error(
                "text_param_as_file",
                "This parameter must be sent as a text field, not a file.",
                Some(param),
            ));
        }
        let data = field.bytes().await.map_err(multer_error)?;
        let Ok(value) = String::from_utf8(data.to_vec()) else {
            return Err(request_error(
                "invalid_utf8",
                "A text parameter is not valid UTF-8.",
                Some(param),
            ));
        };
        // The child passes text to C APIs that stop at NUL.
        if value.contains('\0') {
            return Err(request_error(
                "nul_in_text",
                "A text parameter contains a NUL character.",
                Some(param),
            ));
        }
        match param {
            "model" => model = Some(value),
            "language" => options.language = language(&value)?,
            // An empty prompt is no prompt.
            "prompt" => options.prompt = (!value.is_empty()).then_some(value),
            "temperature" => options.temperature = Some(temperature(&value)?),
            "response_format" => {
                format = match value.as_str() {
                    "json" => ResponseFormat::Json,
                    "text" => ResponseFormat::Text,
                    _ => {
                        return Err(request_error(
                            "invalid_response_format",
                            "Unsupported response_format. Supported: json, text.",
                            Some("response_format"),
                        ));
                    }
                }
            }
            _ => match value.as_str() {
                "false" => {}
                "true" => {
                    return Err(request_error(
                        "stream_unsupported",
                        "Streaming is not supported.",
                        Some("stream"),
                    ));
                }
                _ => {
                    return Err(request_error(
                        "invalid_stream",
                        "stream must be true or false.",
                        Some("stream"),
                    ));
                }
            },
        }
    }
    let Some(file) = file else {
        return Err(request_error(
            "missing_file",
            "You must provide a file parameter.",
            Some("file"),
        ));
    };
    let Some(model) = model else {
        return Err(request_error(
            "missing_model",
            "You must provide a model parameter.",
            Some("model"),
        ));
    };
    Ok(Form {
        file,
        model,
        options,
        format,
    })
}

/// A blank `language` asks for detection, like none at all; otherwise two
/// ASCII letters, lowercased. Whether Whisper knows the code is the
/// child's check (`invalid_language` either way).
fn language(value: &str) -> Result<Option<String>, StageError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.len() != 2 || !trimmed.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return Err(request_error(
            "invalid_language",
            "language must be a supported ISO-639-1 code.",
            Some("language"),
        ));
    }
    Ok(Some(trimmed.to_ascii_lowercase()))
}

/// A number from 0 to 1, as the transcription module accepts it.
fn temperature(value: &str) -> Result<f32, StageError> {
    match value.trim().parse::<f32>() {
        Ok(temperature) if temperature.is_finite() && (0.0..=1.0).contains(&temperature) => {
            Ok(temperature)
        }
        _ => Err(request_error(
            "invalid_temperature",
            "temperature must be a number between 0 and 1.",
            Some("temperature"),
        )),
    }
}

fn multer_error(error: multer::Error) -> StageError {
    match error {
        multer::Error::FieldSizeExceeded { field_name, .. } => {
            match field_name.as_deref().and_then(known_param) {
                Some("file") => request_error(
                    "file_too_large",
                    "The uploaded file exceeds the maximum size of 25000000 bytes.",
                    Some("file"),
                ),
                param => request_error(
                    "field_too_large",
                    "A text field exceeds the maximum size of 16384 bytes.",
                    param,
                ),
            }
        }
        multer::Error::StreamSizeExceeded { .. } => dictation::body_too_large(),
        _ => request_error(
            "malformed_multipart",
            "The multipart request body is malformed.",
            None,
        ),
    }
}

/// What post-processing did, in `pumice.post_processing.status` and the
/// status header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    /// An off flow: the raw transcript is the final text.
    Off,
    Applied,
    /// A silent (whitespace-only) transcript: no AI call.
    Skipped,
    /// A failure after the transcription (formatter, relay, shutdown or
    /// cleanup): the raw transcript is `text` (D4, D9).
    Failed,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Off => "off",
            Status::Applied => "applied",
            Status::Skipped => "skipped",
            Status::Failed => "failed",
        }
    }
}

/// The finished (or refused) operation as an OpenAI answer.
pub(crate) fn respond(output: RunOutput, answering: Option<Answering>) -> Response<Full<Bytes>> {
    // D4, D9: past a successful transcription (its transcript is kept only
    // then), any later failure is a success with the raw transcript, a
    // failed cleanup included (admission is stopped all the same).
    let post_processing_failed = output.error.is_some() && output.transcript.is_some();
    if let Some(error) = &output.error
        && !post_processing_failed
    {
        return error_response(error);
    }
    let (Some(answering), Some(transcript)) = (answering, output.transcript.as_ref()) else {
        return internal_error();
    };
    let (status, text) = match &output.result {
        _ if post_processing_failed => (Status::Failed, transcript.text.as_str()),
        Some(result) if answering.off() => (Status::Off, result.text.as_str()),
        Some(result) if output.stages.relay.status == StageStatus::Skipped => {
            (Status::Skipped, result.text.as_str())
        }
        Some(result) => (Status::Applied, result.text.as_str()),
        None => return internal_error(),
    };
    match success(&output, &answering, status, text, output.error.as_ref()) {
        Some(response) => response,
        // A final text too large to return fails the relay stage, and the
        // raw transcript (at most the child's 1 MiB result) goes instead.
        None => {
            let error = StageError::new(
                stage::RELAY,
                "relay_invalid_output",
                "the provider's reply is too large to return",
                1,
            )
            .with_cause("output_too_large");
            let mut output = output;
            output.stages.relay.status = StageStatus::Error;
            let Some(transcript) = output.transcript.as_ref() else {
                return internal_error();
            };
            success(
                &output,
                &answering,
                Status::Failed,
                &transcript.text,
                Some(&error),
            )
            .unwrap_or_else(internal_error)
        }
    }
}

/// The 200 answer, or `None` when it would exceed the 16 MiB bound.
fn success(
    output: &RunOutput,
    answering: &Answering,
    status: Status,
    text: &str,
    failure: Option<&StageError>,
) -> Option<Response<Full<Bytes>>> {
    let mut response = match answering.format {
        ResponseFormat::Text => {
            if text.len() > MAX_RESPONSE_BYTES {
                return None;
            }
            let mut response = json_bytes_response(StatusCode::OK, text.as_bytes().to_vec());
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            );
            response
        }
        ResponseFormat::Json => {
            let body = json!({
                "text": text,
                "pumice": details(output, answering, status, failure),
            });
            let bytes = serde_json::to_vec(&body).ok()?;
            if bytes.len() > MAX_RESPONSE_BYTES {
                return None;
            }
            json_bytes_response(StatusCode::OK, bytes)
        }
    };
    response
        .headers_mut()
        .insert(STATUS_HEADER, HeaderValue::from_static(status.as_str()));
    Some(response)
}

/// The `pumice` object: everything standard clients ignore.
fn details(
    output: &RunOutput,
    answering: &Answering,
    status: Status,
    failure: Option<&StageError>,
) -> Value {
    let mut post_processing = json!({
        "status": status.as_str(),
        "preset": answering.preset,
        "relay_route": answering.relay_route,
    });
    if let Some(error) = failure {
        // The safe HTTP message: never a configuration path.
        let (_, object) = http_error(error);
        let mut details = json!({
            "stage": error.stage,
            "code": error.code,
            "message": object.message,
        });
        if let Some(cause) = &object.cause_code {
            details["cause_code"] = json!(cause);
        }
        post_processing["error"] = details;
    }
    json!({
        "schema_version": SCHEMA_VERSION,
        "operation_id": output.operation_id,
        "flow": output.flow,
        "transcript": output.transcript,
        "post_processing": post_processing,
        "stages": output.stages,
        "timings_ms": Timings {
            total: millis(output.total),
        },
    })
}

/// A failure as the OpenAI error object, with its status; `Retry-After: 1`
/// when the service is busy.
fn error_response(error: &StageError) -> Response<Full<Bytes>> {
    let (class, object) = http_error(error);
    let body = json!({
        "error": {
            "message": object.message,
            "type": object.kind,
            "param": object.param,
            "code": object.code,
        }
    });
    let Ok(bytes) = serde_json::to_vec(&body) else {
        return internal_error();
    };
    let status = StatusCode::from_u16(class.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut response = json_bytes_response(status, bytes);
    if error.code == "rate_limit_exceeded" {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    }
    response
}

fn internal_error() -> Response<Full<Bytes>> {
    json_bytes_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        br#"{"error":{"message":"The request failed.","type":"server_error","param":null,"code":"internal"}}"#
            .to_vec(),
    )
}

/// `GET /v1/models`: the published flows, in `serve.transcription_flows`
/// order.
pub(crate) fn models(frontend: &Frontend) -> Value {
    models_body(
        frontend.flow_infos(),
        frontend.published(),
        frontend.unavailable(),
    )
}

/// The model list for these flows and this state; startup checks its
/// largest form against the metadata bound (`http::oversized_snapshot`).
pub(crate) fn models_body(
    flows: &[FlowInfo],
    published: &[String],
    unavailable: Option<&str>,
) -> Value {
    let data: Vec<Value> = published
        .iter()
        .filter_map(|id| flows.iter().find(|flow| &flow.id == id))
        .map(|flow| model_object(flow, unavailable))
        .collect();
    json!({"object": "list", "data": data})
}

/// `GET /v1/models/{id}`: one published flow as an OpenAI model object,
/// with its Pumice details; `None` for anything else.
pub(crate) fn model(frontend: &Frontend, id: &str) -> Option<Value> {
    if !frontend.published().iter().any(|published| published == id) {
        return None;
    }
    let flow = frontend.flow_infos().iter().find(|flow| flow.id == id)?;
    Some(model_object(flow, frontend.unavailable()))
}

fn model_object(flow: &FlowInfo, unavailable: Option<&str>) -> Value {
    let mut details = json!({
        "schema_version": SCHEMA_VERSION,
        "flow": flow.id,
        "transcription_model": flow.transcription_model,
        "post_processing": {
            "preset": flow.formatter,
            "relay_route": flow.relay_route,
        },
        "response_formats": RESPONSE_FORMATS,
        "ready": unavailable.is_none(),
    });
    if let Some(code) = unavailable {
        details["unavailable"] = json!(code);
    }
    json!({
        "id": flow.id,
        "object": "model",
        "created": 0,
        "owned_by": "pumice",
        "pumice": details,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::result::{FinalText, StageReport, StageReports, Transcript};
    use http_body_util::BodyExt;
    use std::time::Duration;

    type Part<'a> = (&'a str, Option<&'a str>, &'a [u8]);

    fn form(parts: &[Part]) -> (Bytes, String) {
        let boundary = "XyZ".to_owned();
        let mut body = Vec::new();
        for (name, file_name, data) in parts {
            body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            let disposition = match file_name {
                Some(file_name) => format!(
                    "Content-Disposition: form-data; name=\"{name}\"; filename=\"{file_name}\"\r\n\r\n"
                ),
                None => format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n"),
            };
            body.extend_from_slice(disposition.as_bytes());
            body.extend_from_slice(data);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        (Bytes::from(body), boundary)
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }

    fn parse(parts: &[Part]) -> Result<Form, StageError> {
        let (body, boundary) = form(parts);
        block_on(parse_form(body, boundary))
    }

    fn code(parts: &[Part]) -> (&'static str, Option<&'static str>) {
        match parse(parts) {
            Ok(_) => ("ok", None),
            Err(error) => (error.code, error.param),
        }
    }

    const FILE: Part = ("file", Some("x.wav"), b"RIFF");
    const MODEL: Part = ("model", None, b"clean");

    #[test]
    fn every_accepted_field_is_read() {
        let form = parse(&[
            ("stream", None, b"false"),
            ("response_format", None, b"text"),
            ("temperature", None, b" 0.5 "),
            ("prompt", None, "Olá, Pumice".as_bytes()),
            ("language", None, b" PT "),
            MODEL,
            FILE,
        ])
        .unwrap_or_else(|_| panic!("valid"));
        assert_eq!(&form.file[..], b"RIFF");
        assert_eq!(form.model, "clean");
        assert_eq!(form.format, ResponseFormat::Text);
        assert_eq!(form.options.language.as_deref(), Some("pt"));
        assert_eq!(form.options.prompt.as_deref(), Some("Olá, Pumice"));
        assert_eq!(form.options.temperature, Some(0.5));

        // Only file and model: json, no options.
        let form = parse(&[FILE, MODEL]).unwrap_or_else(|_| panic!("valid"));
        assert_eq!(form.format, ResponseFormat::Json);
        assert!(form.options.language.is_none());
        assert!(form.options.prompt.is_none());
        assert!(form.options.temperature.is_none());

        // A blank language or prompt is none; the bounds of temperature.
        let form = parse(&[
            FILE,
            MODEL,
            ("language", None, b" "),
            ("prompt", None, b""),
            ("temperature", None, b"1"),
            ("response_format", None, b"json"),
        ])
        .unwrap_or_else(|_| panic!("valid"));
        assert!(form.options.language.is_none());
        assert!(form.options.prompt.is_none());
        assert_eq!(form.options.temperature, Some(1.0));
        assert_eq!(
            code(&[FILE, MODEL, ("temperature", None, b"0")]),
            ("ok", None)
        );
    }

    #[test]
    fn every_refused_field_gets_a_fixed_code() {
        for (parts, expected) in [
            (vec![MODEL], ("missing_file", Some("file"))),
            (vec![FILE], ("missing_model", Some("model"))),
            (
                vec![FILE, MODEL, FILE],
                ("duplicate_parameter", Some("file")),
            ),
            (
                vec![
                    FILE,
                    MODEL,
                    ("language", None, b"en"),
                    ("language", None, b"en"),
                ],
                ("duplicate_parameter", Some("language")),
            ),
            (
                vec![FILE, MODEL, ("flow", None, b"clean")],
                ("unsupported_parameter", None),
            ),
            (
                vec![FILE, MODEL, ("timestamp_granularities[]", None, b"word")],
                ("unsupported_parameter", None),
            ),
            (
                vec![FILE, MODEL, ("include[]", None, b"logprobs")],
                ("unsupported_parameter", None),
            ),
            (
                vec![FILE, MODEL, ("chunking_strategy", None, b"auto")],
                ("unsupported_parameter", None),
            ),
            (
                vec![("file", Some("x.wav"), b""), MODEL],
                ("empty_file", Some("file")),
            ),
            (
                vec![("file", None, b"RIFF"), MODEL],
                ("file_not_upload", Some("file")),
            ),
            (
                vec![FILE, ("model", Some("m.txt"), b"clean")],
                ("text_param_as_file", Some("model")),
            ),
            (
                vec![FILE, MODEL, ("prompt", None, b"\xff")],
                ("invalid_utf8", Some("prompt")),
            ),
            (
                vec![FILE, MODEL, ("prompt", None, b"a\0b")],
                ("nul_in_text", Some("prompt")),
            ),
            (
                vec![FILE, MODEL, ("language", None, b"english")],
                ("invalid_language", Some("language")),
            ),
            (
                vec![FILE, MODEL, ("language", None, b"p1")],
                ("invalid_language", Some("language")),
            ),
            (
                vec![FILE, MODEL, ("temperature", None, b"1.5")],
                ("invalid_temperature", Some("temperature")),
            ),
            (
                vec![FILE, MODEL, ("temperature", None, b"-0.1")],
                ("invalid_temperature", Some("temperature")),
            ),
            (
                vec![FILE, MODEL, ("temperature", None, b"NaN")],
                ("invalid_temperature", Some("temperature")),
            ),
            (
                vec![FILE, MODEL, ("temperature", None, b"")],
                ("invalid_temperature", Some("temperature")),
            ),
            (
                vec![FILE, MODEL, ("response_format", None, b"verbose_json")],
                ("invalid_response_format", Some("response_format")),
            ),
            (
                vec![FILE, MODEL, ("response_format", None, b"srt")],
                ("invalid_response_format", Some("response_format")),
            ),
            (
                vec![FILE, MODEL, ("stream", None, b"true")],
                ("stream_unsupported", Some("stream")),
            ),
            (
                vec![FILE, MODEL, ("stream", None, b"yes")],
                ("invalid_stream", Some("stream")),
            ),
        ] {
            assert_eq!(code(&parts), expected, "{expected:?}");
        }
        let long = vec![b'a'; 16 * 1024 + 1];
        assert_eq!(
            code(&[FILE, MODEL, ("prompt", None, &long)]),
            ("field_too_large", Some("prompt"))
        );
        let fits = vec![b'a'; 16 * 1024];
        assert_eq!(code(&[FILE, MODEL, ("prompt", None, &fits)]), ("ok", None));
        let result = block_on(parse_form(Bytes::from_static(b"garbage"), "XyZ".to_owned()));
        assert_eq!(
            result.err().map(|error| error.code),
            Some("malformed_multipart")
        );
    }

    #[test]
    fn messages_never_quote_the_request() {
        let sentinel = "S3NT1NEL";
        for parts in [
            vec![(sentinel, None, &b"x"[..])],
            vec![FILE, MODEL, ("language", None, sentinel.as_bytes())],
            vec![FILE, MODEL, ("temperature", None, sentinel.as_bytes())],
            vec![FILE, MODEL, ("response_format", None, sentinel.as_bytes())],
            vec![FILE, ("model", Some(sentinel), b"x")],
        ] {
            let error = parse(&parts).err().expect("refused");
            assert!(!error.message.contains(sentinel), "{}", error.message);
        }
    }

    fn answering(format: ResponseFormat, preset: Option<&str>, route: &str) -> Answering {
        Answering {
            format,
            preset: preset.map(str::to_owned),
            relay_route: route.to_owned(),
        }
    }

    /// A finished operation: the transcript, then `result` or `error`.
    fn output(result: Option<&str>, error: Option<StageError>, relay: StageStatus) -> RunOutput {
        RunOutput {
            operation_id: "op-1".to_owned(),
            flow: Some("clean".to_owned()),
            transcript: Some(Transcript {
                text: " raw words".to_owned(),
                language: Some("en".to_owned()),
                audio_seconds: 2.0,
            }),
            result: result.map(|text| FinalText {
                text: text.to_owned(),
            }),
            stages: StageReports {
                relay: StageReport::measured(relay, Duration::from_millis(3)),
                ..StageReports::default()
            },
            total: Duration::from_millis(42),
            error,
            cleanup_failed: false,
        }
    }

    fn read(response: Response<Full<Bytes>>) -> (StatusCode, hyper::HeaderMap, Bytes) {
        let status = response.status();
        let headers = response.headers().clone();
        let body = block_on(response.into_body().collect()).unwrap().to_bytes();
        (status, headers, body)
    }

    #[test]
    fn a_success_answers_the_final_text_with_the_details() {
        let clean = || answering(ResponseFormat::Json, Some("builtin:clean"), "claude");
        let (status, headers, body) = read(respond(
            output(Some("Raw words."), None, StageStatus::Ok),
            Some(clean()),
        ));
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[STATUS_HEADER], "applied");
        assert_eq!(headers[header::CACHE_CONTROL], "no-store");
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["text"], "Raw words.");
        let pumice = &body["pumice"];
        assert_eq!(pumice["schema_version"], 1);
        assert_eq!(pumice["operation_id"], "op-1");
        assert_eq!(pumice["flow"], "clean");
        assert_eq!(pumice["transcript"]["text"], " raw words");
        assert_eq!(pumice["transcript"]["language"], "en");
        assert_eq!(
            pumice["post_processing"],
            json!({"status": "applied", "preset": "builtin:clean", "relay_route": "claude"})
        );
        assert_eq!(pumice["stages"]["relay"]["status"], "ok");
        assert_eq!(pumice["timings_ms"]["total"], 42);

        // A silent transcript skipped the AI.
        let (_, headers, body) = read(respond(
            output(Some("  "), None, StageStatus::Skipped),
            Some(clean()),
        ));
        assert_eq!(headers[STATUS_HEADER], "skipped");
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["pumice"]["post_processing"]["status"], "skipped");

        // An off flow, as plain text: the exact text, nothing else.
        let (status, headers, body) = read(respond(
            output(Some(" raw words"), None, StageStatus::Ok),
            Some(answering(ResponseFormat::Text, None, "passthrough")),
        ));
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[STATUS_HEADER], "off");
        assert_eq!(headers[header::CONTENT_TYPE], "text/plain; charset=utf-8");
        assert_eq!(&body[..], b" raw words");
    }

    #[test]
    fn a_post_processing_failure_answers_the_raw_transcript() {
        for (error, cause) in [
            (
                StageError::new(stage::RELAY, "relay_failed", "the provider failed", 1),
                None,
            ),
            (
                StageError::new(stage::RELAY, "relay_timeout", "late", 1),
                None,
            ),
            (
                StageError::new(stage::RELAY, "relay_invalid_output", "empty", 1)
                    .with_cause("empty_output"),
                Some("empty_output"),
            ),
            (
                StageError::new(stage::FORMATTER, "formatter_instruction_conflict", "x", 1),
                None,
            ),
        ] {
            let code = error.code;
            let (status, headers, body) = read(respond(
                output(None, Some(error), StageStatus::Error),
                Some(answering(
                    ResponseFormat::Json,
                    Some("builtin:clean"),
                    "claude",
                )),
            ));
            assert_eq!(status, StatusCode::OK, "{code}");
            assert_eq!(headers[STATUS_HEADER], "failed");
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["text"], " raw words", "{code}");
            let failure = &body["pumice"]["post_processing"];
            assert_eq!(failure["status"], "failed");
            assert_eq!(failure["error"]["code"], code);
            assert!(failure["error"]["message"].is_string());
            assert_eq!(failure["error"]["cause_code"].as_str(), cause);
        }
        // A located message never reaches the client.
        let mut error = StageError::new(stage::FORMATTER, "formatter_not_found", "x", 2);
        error.locate(crate::orchestrator::result::ErrorLocation {
            file: "/srv/S3NT1NEL/orchestrator.yml".into(),
            line: Some(3),
            column: Some(1),
        });
        let (status, _, body) = read(respond(
            output(None, Some(error), StageStatus::Skipped),
            Some(answering(ResponseFormat::Json, Some("x"), "claude")),
        ));
        assert_eq!(status, StatusCode::OK);
        let text = String::from_utf8_lossy(&body);
        assert!(!text.contains("S3NT1NEL"), "{text}");
        assert!(text.contains("formatter_not_found"), "{text}");

        // As plain text: the raw transcript, exactly.
        let (_, headers, body) = read(respond(
            output(
                None,
                Some(StageError::new(stage::RELAY, "relay_failed", "x", 1)),
                StageStatus::Error,
            ),
            Some(answering(ResponseFormat::Text, Some("x"), "claude")),
        ));
        assert_eq!(headers[STATUS_HEADER], "failed");
        assert_eq!(&body[..], b" raw words");
    }

    #[test]
    fn a_cleanup_failure_after_the_transcription_answers_the_raw_transcript() {
        // D9: the transcript stays valid; the failure is reported, not lost.
        let failed = || {
            let mut output = output(
                None,
                Some(StageError::new(stage::CLEANUP, "cleanup_failed", "x", 1)),
                StageStatus::Ok,
            );
            output.cleanup_failed = true;
            output
        };
        let (status, headers, body) = read(respond(
            failed(),
            Some(answering(
                ResponseFormat::Json,
                Some("builtin:clean"),
                "claude",
            )),
        ));
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[STATUS_HEADER], "failed");
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["text"], " raw words");
        let failure = &body["pumice"]["post_processing"];
        assert_eq!(failure["status"], "failed");
        assert_eq!(failure["error"]["stage"], "cleanup");
        assert_eq!(failure["error"]["code"], "cleanup_failed");
        assert!(failure["error"]["message"].is_string());

        let (status, headers, body) = read(respond(
            failed(),
            Some(answering(ResponseFormat::Text, None, "passthrough")),
        ));
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[STATUS_HEADER], "failed");
        assert_eq!(headers[header::CONTENT_TYPE], "text/plain; charset=utf-8");
        assert_eq!(&body[..], b" raw words");
    }

    #[test]
    fn other_failures_are_openai_errors() {
        let mut no_transcript = output(
            None,
            Some(StageError::new(
                stage::TRANSCRIPTION,
                "transcription_timeout",
                "late",
                1,
            )),
            StageStatus::Skipped,
        );
        no_transcript.transcript = None;
        let (status, headers, body) = read(respond(
            no_transcript,
            Some(answering(ResponseFormat::Text, None, "passthrough")),
        ));
        assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
        assert!(headers.get(STATUS_HEADER).is_none());
        assert_eq!(headers[header::CONTENT_TYPE], "application/json");
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            body,
            json!({"error": {
                "message": "Late.",
                "type": "server_error",
                "param": null,
                "code": "transcription_timeout",
            }})
        );

        // Busy: 429 with Retry-After; a refused model: 404 on `model`.
        let mut refused = output(None, Some(dictation_busy()), StageStatus::Skipped);
        refused.transcript = None;
        let (status, headers, body) = read(respond(refused, None));
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(headers[header::RETRY_AFTER], "1");
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"]["type"], "rate_limit_error");
        assert_eq!(body["error"]["code"], "rate_limit_exceeded");
        let mut refused = output(None, Some(model_not_found()), StageStatus::Skipped);
        refused.transcript = None;
        let (status, _, body) = read(respond(refused, None));
        assert_eq!(status, StatusCode::NOT_FOUND);
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"]["param"], "model");
        assert_eq!(body["error"]["code"], "model_not_found");
        assert_eq!(body["error"]["type"], "invalid_request_error");
    }

    fn dictation_busy() -> StageError {
        StageError::new(stage::PREPARE, "rate_limit_exceeded", "busy", 1)
    }

    #[test]
    fn a_final_text_too_large_falls_back_to_the_raw_transcript() {
        let huge = "x".repeat(MAX_RESPONSE_BYTES + 1);
        for format in [ResponseFormat::Json, ResponseFormat::Text] {
            let (status, headers, body) = read(respond(
                output(Some(&huge), None, StageStatus::Ok),
                Some(answering(format, Some("builtin:clean"), "claude")),
            ));
            assert_eq!(status, StatusCode::OK);
            assert_eq!(headers[STATUS_HEADER], "failed");
            if format == ResponseFormat::Text {
                assert_eq!(&body[..], b" raw words");
            } else {
                let body: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(body["text"], " raw words");
                let error = &body["pumice"]["post_processing"]["error"];
                assert_eq!(error["code"], "relay_invalid_output");
                assert_eq!(error["cause_code"], "output_too_large");
                assert_eq!(body["pumice"]["stages"]["relay"]["status"], "error");
            }
        }
    }
}
