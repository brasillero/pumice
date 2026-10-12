use crate::audio::{MAX_FILE_BYTES, decode_wav, normalize_and_validate, resample_to_target};
use crate::engine::Engine;
use crate::{Error, ErrorCode, JsonError, TranscriptionOptions};
use clap::{Parser, Subcommand};
use serde::Serialize;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

#[derive(Parser, Debug)]
#[command(
    name = "pumice-transcribe",
    about = "Local speech-to-text transcription",
    version
)]
pub struct Args {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Transcribe an audio file.
    File {
        /// Path to the audio file (WAV).
        audio: PathBuf,

        /// Path to the GGUF model file.
        #[arg(long, value_name = "PATH.gguf")]
        model: PathBuf,

        /// Optional language hint (ISO-639-1 two-letter code).
        #[arg(long, value_name = "xx")]
        language: Option<String>,

        /// Number of threads to use for inference (default: available parallelism).
        #[arg(long, value_name = "N")]
        threads: Option<usize>,

        /// Output a JSON envelope instead of plain text.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct SuccessOutput {
    pub text: String,
    pub language: Option<String>,
    pub audio_seconds: f64,
    pub model_load_ms: u64,
    pub transcription_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
struct Envelope {
    schema_version: i32,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<ResultJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonError>,
}

#[derive(Debug, Clone, Serialize)]
struct ResultJson {
    text: String,
    language: Option<String>,
    audio_seconds: f64,
    model_load_ms: u64,
    transcription_ms: u64,
}

impl From<SuccessOutput> for ResultJson {
    fn from(out: SuccessOutput) -> Self {
        Self {
            text: out.text,
            language: out.language,
            audio_seconds: out.audio_seconds,
            model_load_ms: out.model_load_ms,
            transcription_ms: out.transcription_ms,
        }
    }
}

pub fn run<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args = match Args::try_parse_from(args) {
        Ok(a) => a,
        Err(e) => e.exit(),
    };

    let make_engine = |path: &Path, threads: i32| -> Result<Box<dyn Engine>, Error> {
        #[cfg(feature = "engine")]
        {
            Ok(Box::new(crate::engine::cpp::TranscribeCppEngine::new(
                path, threads,
            )?))
        }
        #[cfg(not(feature = "engine"))]
        {
            let _ = (path, threads);
            Err(Error::engine_not_available())
        }
    };

    let json = json_flag(&args);
    match run_with_factory(args, make_engine) {
        Ok(output) => {
            let _ = print_success(&output, json, &mut io::stdout());
            ExitCode::SUCCESS
        }
        Err(e) => {
            let _ = print_error(&e, json, &mut io::stdout(), &mut io::stderr());
            exit_code(&e)
        }
    }
}

pub fn run_with_engine<I, T>(args: I, engine: Box<dyn Engine>) -> Result<SuccessOutput, Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args = Args::try_parse_from(args).map_err(|e| Error::invalid_arguments(format!("{e}")))?;
    run_with_factory(args, |_: &Path, _: i32| Ok(engine))
}

pub fn run_with_factory<F>(args: Args, make_engine: F) -> Result<SuccessOutput, Error>
where
    F: FnOnce(&Path, i32) -> Result<Box<dyn Engine>, Error>,
{
    match args.command {
        Command::File {
            audio,
            model,
            language,
            threads,
            json,
        } => {
            let _ = json; // JSON flag affects printing, not computation.
            let language = normalize_language(language);
            let threads = normalize_threads(threads)?;

            let file = std::fs::File::open(&audio)?;
            let metadata = file.metadata()?;
            if metadata.len() > MAX_FILE_BYTES {
                return Err(Error::input_too_large(metadata.len()));
            }

            let decoded = decode_wav(file)?;
            let resampled = resample_to_target(&decoded.samples, decoded.sample_rate)?;
            let pcm = normalize_and_validate(resampled)?;
            let audio_seconds = pcm.len() as f64 / 16_000.0;

            if !model.is_file() {
                return Err(Error::model_not_found(&model));
            }

            let start = Instant::now();
            let engine = make_engine(&model, threads)?;
            let model_load_ms = start.elapsed().as_millis() as u64;

            let start = Instant::now();
            let transcript = engine.transcribe(&pcm, &TranscriptionOptions { language })?;
            let transcription_ms = start.elapsed().as_millis() as u64;

            Ok(SuccessOutput {
                text: transcript.text,
                language: transcript.language,
                audio_seconds,
                model_load_ms,
                transcription_ms,
            })
        }
    }
}

pub fn print_success(output: &SuccessOutput, json: bool, stdout: &mut dyn Write) -> io::Result<()> {
    if json {
        let envelope = Envelope {
            schema_version: 1,
            ok: true,
            result: Some(output.clone().into()),
            error: None,
        };
        writeln!(stdout, "{}", serde_json::to_string(&envelope).unwrap())
    } else {
        writeln!(stdout, "{}", output.text)
    }
}

pub fn print_error(
    error: &Error,
    json: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> io::Result<()> {
    if json {
        let envelope = Envelope {
            schema_version: 1,
            ok: false,
            result: None,
            error: Some(error.to_json_error()),
        };
        writeln!(stdout, "{}", serde_json::to_string(&envelope).unwrap())
    } else {
        writeln!(stderr, "{error}")
    }
}

pub fn exit_code(error: &Error) -> ExitCode {
    let code = match error.code {
        ErrorCode::InvalidArguments
        | ErrorCode::InvalidInput
        | ErrorCode::InputTooLarge
        | ErrorCode::AudioTooLong
        | ErrorCode::UnsupportedFormat
        | ErrorCode::ModelNotFound => 2,
        ErrorCode::EngineNotAvailable
        | ErrorCode::EngineFailed
        | ErrorCode::ModelLoadFailed
        | ErrorCode::Internal => 1,
    };
    ExitCode::from(code)
}

fn json_flag(args: &Args) -> bool {
    matches!(args.command, Command::File { json: true, .. })
}

fn normalize_language(language: Option<String>) -> Option<String> {
    let trimmed = language
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());
    trimmed.filter(|s| s.len() == 2 && s.bytes().all(|b| b.is_ascii_lowercase()))
}

fn normalize_threads(threads: Option<usize>) -> Result<i32, Error> {
    match threads {
        Some(0) => Err(Error::invalid_arguments("--threads must be at least 1")),
        Some(n) if n > 256 => Err(Error::invalid_arguments("--threads must be at most 256")),
        Some(n) => Ok(n as i32),
        None => Ok(std::thread::available_parallelism()
            .map(|p| p.get() as i32)
            .unwrap_or(1)),
    }
}
