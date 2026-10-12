use crate::audio::{MAX_FILE_BYTES, decode_wav, normalize_and_validate, resample_to_target};
use crate::engine::Engine;
use crate::{Error, ErrorCode, JsonError, TranscriptionOptions};
use clap::error::ErrorKind;
use clap::{Parser, Subcommand};
use serde::Serialize;
use std::ffi::OsString;
use std::io::{self, Read, Write};
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

        /// Optional language hint (ISO-639-1 two-letter code). When given, this
        /// value is reported in the result; otherwise the engine-detected language
        /// is reported.
        #[arg(long, value_name = "xx")]
        language: Option<String>,

        /// Number of threads to use for inference (default: available parallelism, capped at 32).
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

/// Whisper's supported ISO-639-1 language codes.
const SUPPORTED_LANGUAGES: &[&str] = &[
    "af", "am", "ar", "as", "az", "ba", "be", "bg", "bn", "bo", "br", "bs", "ca", "cs", "cy", "da",
    "de", "el", "en", "es", "et", "eu", "fa", "fi", "fo", "fr", "gl", "gu", "ha", "haw", "he",
    "hi", "hr", "ht", "hu", "hy", "id", "is", "it", "ja", "jw", "ka", "km", "kn", "ko", "la", "lb",
    "ln", "lo", "lt", "lv", "mg", "mi", "mk", "ml", "mn", "mr", "ms", "mt", "my", "ne", "nl", "nn",
    "no", "oc", "pa", "pl", "ps", "pt", "ro", "ru", "sa", "sd", "si", "sk", "sl", "sn", "so", "sq",
    "sr", "su", "sv", "sw", "ta", "te", "tg", "th", "tk", "tl", "tr", "tt", "uk", "ur", "uz", "vi",
    "yi", "yo", "zh",
];

pub fn run<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let raw: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let wants_json = raw.iter().any(|a| a.as_encoded_bytes() == b"--json");

    let parsed = Args::try_parse_from(&raw);
    let args = match parsed {
        Ok(a) => a,
        Err(e) => {
            return handle_clap_error(e, wants_json);
        }
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
    let mut stdout = io::stdout();
    let mut stderr = io::stderr();

    match run_with_factory(args, make_engine) {
        Ok(output) => match print_success(&output, json, &mut stdout) {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => {
                let _ = stderr.write_all(b"failed to write output\n");
                ExitCode::from(1)
            }
        },
        Err(e) => match print_error(&e, json, &mut stdout, &mut stderr) {
            Ok(()) => exit_code(&e),
            Err(_) => {
                let _ = stderr.write_all(b"failed to write output\n");
                ExitCode::from(1)
            }
        },
    }
}

fn handle_clap_error(err: clap::Error, wants_json: bool) -> ExitCode {
    match err.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
            if err.print().is_err() {
                let _ = io::stderr().write_all(b"failed to write output\n");
                return ExitCode::from(1);
            }
            ExitCode::SUCCESS
        }
        _ if wants_json => {
            let mut stdout = io::stdout();
            let envelope = Envelope {
                schema_version: 1,
                ok: false,
                result: None,
                error: Some(JsonError {
                    code: ErrorCode::InvalidArguments.as_str().to_string(),
                    message: err.to_string().trim().to_string(),
                }),
            };
            // Clap errors may include newlines; emit exactly one JSON object followed by a newline.
            if writeln!(stdout, "{}", serde_json::to_string(&envelope).unwrap()).is_err() {
                let _ = io::stderr().write_all(b"failed to write output\n");
                return ExitCode::from(1);
            }
            ExitCode::from(2)
        }
        _ => {
            if err.print().is_err() {
                let _ = io::stderr().write_all(b"failed to write output\n");
                return ExitCode::from(1);
            }
            ExitCode::from(2)
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
            json: _,
        } => {
            let language = normalize_language(language)?;
            let threads = normalize_threads(threads)?;

            let file = open_regular_file(&audio)?;
            let metadata = file.metadata()?;
            if metadata.len() > MAX_FILE_BYTES {
                return Err(Error::input_too_large(metadata.len()));
            }

            let budgeted = BudgetReader::new(file, MAX_FILE_BYTES);
            let decoded =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| decode_wav(budgeted)))
                    .map_err(|_| Error::invalid_input("audio decoding panicked"))?
                    .map_err(map_budget_error)?;
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
            let transcript = engine.transcribe(
                &pcm,
                &TranscriptionOptions {
                    language: language.clone(),
                },
            )?;
            let transcription_ms = start.elapsed().as_millis() as u64;

            Ok(SuccessOutput {
                text: transcript.text,
                language: language.or(transcript.language),
                audio_seconds,
                model_load_ms,
                transcription_ms,
            })
        }
    }
}

fn open_regular_file(path: &Path) -> Result<std::fs::File, Error> {
    // Check the path before opening so FIFOs and other special files are rejected without
    // blocking. Re-check on the descriptor to cover races.
    let pre_metadata = std::fs::metadata(path)?;
    if !pre_metadata.file_type().is_file() {
        return Err(Error::invalid_input(format!(
            "audio path is not a regular file: {}",
            path.display()
        )));
    }

    let file = open_with_nonblock_on_unix(path)?;

    let post_metadata = file.metadata()?;
    if !post_metadata.file_type().is_file() {
        return Err(Error::invalid_input(format!(
            "audio path is not a regular file: {}",
            path.display()
        )));
    }

    Ok(file)
}

#[cfg(unix)]
fn open_with_nonblock_on_unix(path: &Path) -> Result<std::fs::File, Error> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(Error::from)
}

#[cfg(not(unix))]
fn open_with_nonblock_on_unix(path: &Path) -> Result<std::fs::File, Error> {
    std::fs::File::open(path).map_err(Error::from)
}

/// Reader that enforces a hard byte budget while reading.
struct BudgetReader<R> {
    inner: R,
    budget: u64,
    read: u64,
}

impl<R> BudgetReader<R> {
    fn new(inner: R, budget: u64) -> Self {
        Self {
            inner,
            budget,
            read: 0,
        }
    }
}

impl<R: Read> Read for BudgetReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.read >= self.budget {
            return Err(io::Error::other("byte budget exceeded"));
        }
        let max = ((self.budget - self.read) as usize).min(buf.len());
        let n = self.inner.read(&mut buf[..max])?;
        self.read += n as u64;
        Ok(n)
    }
}

fn map_budget_error(err: Error) -> Error {
    if err.code == ErrorCode::InvalidInput && err.message.contains("byte budget exceeded") {
        Error::input_too_large(MAX_FILE_BYTES)
    } else {
        err
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
        if output.text.ends_with('\n') {
            write!(stdout, "{}", output.text)
        } else {
            writeln!(stdout, "{}", output.text)
        }
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

fn normalize_language(language: Option<String>) -> Result<Option<String>, Error> {
    let Some(lang) = language else {
        return Ok(None);
    };
    let lang = lang.trim().to_lowercase();
    if lang.len() != 2 || !lang.bytes().all(|b| b.is_ascii_lowercase()) {
        return Err(Error::invalid_arguments(
            "--language must be a 2-letter ISO-639-1 code",
        ));
    }
    if !SUPPORTED_LANGUAGES.contains(&lang.as_str()) {
        return Err(Error::invalid_arguments(format!(
            "unsupported language code: {lang}"
        )));
    }
    Ok(Some(lang))
}

fn normalize_threads(threads: Option<usize>) -> Result<i32, Error> {
    const MAX_THREADS: i32 = 32;
    match threads {
        Some(0) => Err(Error::invalid_arguments("--threads must be at least 1")),
        Some(n) if n > MAX_THREADS as usize => {
            Err(Error::invalid_arguments("--threads must be at most 32"))
        }
        Some(n) => Ok(n as i32),
        None => Ok(std::thread::available_parallelism()
            .map(|p| (p.get() as i32).min(MAX_THREADS))
            .unwrap_or(1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("boom"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn print_success_plain_does_not_duplicate_trailing_newline() {
        let output = SuccessOutput {
            text: "hello\n".to_string(),
            language: None,
            audio_seconds: 1.0,
            model_load_ms: 0,
            transcription_ms: 0,
        };
        let mut buf = Vec::new();
        print_success(&output, false, &mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "hello\n");
    }

    #[test]
    fn print_success_plain_adds_newline_when_missing() {
        let output = SuccessOutput {
            text: "hello".to_string(),
            language: None,
            audio_seconds: 1.0,
            model_load_ms: 0,
            transcription_ms: 0,
        };
        let mut buf = Vec::new();
        print_success(&output, false, &mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "hello\n");
    }

    #[test]
    fn print_success_returns_error_on_failed_write() {
        let output = SuccessOutput {
            text: "hello".to_string(),
            language: None,
            audio_seconds: 1.0,
            model_load_ms: 0,
            transcription_ms: 0,
        };
        let mut writer = FailingWriter;
        assert!(print_success(&output, false, &mut writer).is_err());
    }

    #[test]
    fn print_error_returns_error_on_failed_write() {
        let error = Error::engine_failed();
        let mut stdout = FailingWriter;
        let mut stderr = Vec::new();
        assert!(print_error(&error, true, &mut stdout, &mut stderr).is_err());
    }

    #[test]
    fn normalize_language_accepts_supported_code() {
        assert_eq!(
            normalize_language(Some("pt".to_string())).unwrap(),
            Some("pt".to_string())
        );
    }

    #[test]
    fn normalize_language_uppercase_is_normalized() {
        assert_eq!(
            normalize_language(Some("PT".to_string())).unwrap(),
            Some("pt".to_string())
        );
    }

    #[test]
    fn normalize_language_rejects_non_iso_code() {
        assert_eq!(
            normalize_language(Some("portuguese".to_string()))
                .unwrap_err()
                .code,
            ErrorCode::InvalidArguments
        );
    }

    #[test]
    fn normalize_language_rejects_unknown_code() {
        assert_eq!(
            normalize_language(Some("zz".to_string())).unwrap_err().code,
            ErrorCode::InvalidArguments
        );
    }

    #[test]
    fn normalize_threads_caps_default() {
        let threads = normalize_threads(None).unwrap();
        assert!((1..=32).contains(&threads));
    }

    #[test]
    fn normalize_threads_rejects_too_large() {
        assert_eq!(
            normalize_threads(Some(33)).unwrap_err().code,
            ErrorCode::InvalidArguments
        );
    }
}
