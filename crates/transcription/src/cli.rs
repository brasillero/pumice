use crate::audio::{MAX_FILE_BYTES, decode_audio, normalize_and_validate, resample_to_target};
use crate::config::{self, Config, ConfigSource, LoadedConfig, validate_language};
use crate::engine::Engine;
use crate::{Error, ErrorCode, JsonError, TranscriptionOptions};
use clap::error::ErrorKind;
use clap::{Args as ClapArgs, Parser, Subcommand};
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
    /// Path to the module configuration file.
    #[arg(long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Transcribe an audio file.
    File(FileArgs),
    /// Validate the configuration file and exit.
    CheckConfig(CheckConfigArgs),
    /// Inspect configured models.
    Models {
        #[command(subcommand)]
        command: ModelsCommand,
    },
}

#[derive(ClapArgs, Debug)]
#[command(group = clap::ArgGroup::new("model_source").args(["model", "model_file"]).required(false))]
pub struct FileArgs {
    /// Path to the audio file (WAV, MP3, FLAC, Ogg Vorbis).
    audio: PathBuf,

    /// Model id from the configuration file.
    #[arg(long, value_name = "ID", group = "model_source")]
    model: Option<String>,

    /// Direct path to a GGUF model file.
    #[arg(long, value_name = "PATH.gguf", group = "model_source")]
    model_file: Option<PathBuf>,

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
}

#[derive(ClapArgs, Debug)]
pub struct CheckConfigArgs {
    /// Output a JSON envelope instead of plain text.
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand, Debug)]
pub enum ModelsCommand {
    /// List configured models.
    List(ModelsListArgs),
}

#[derive(ClapArgs, Debug)]
pub struct ModelsListArgs {
    /// Output JSON instead of a table.
    #[arg(long)]
    json: bool,
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
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonError>,
}

#[derive(Debug, Clone, Serialize)]
struct CheckConfigResult {
    file: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
struct ModelListEntry {
    id: String,
    path: PathBuf,
    exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    default: bool,
}

enum CommandOutput {
    Transcription(SuccessOutput),
    CheckConfig(Option<PathBuf>),
    ModelsList(Vec<ModelListEntry>),
}

impl From<SuccessOutput> for serde_json::Value {
    fn from(out: SuccessOutput) -> Self {
        serde_json::json!({
            "text": out.text,
            "language": out.language,
            "audio_seconds": out.audio_seconds,
            "model_load_ms": out.model_load_ms,
            "transcription_ms": out.transcription_ms,
        })
    }
}

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

    let json = json_flag(&args);
    let mut stdout = io::stdout();
    let mut stderr = io::stderr();

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

    match run_command(args, make_engine) {
        Ok(CommandOutput::Transcription(output)) => match print_success(&output, json, &mut stdout)
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => {
                let _ = stderr.write_all(b"failed to write output\n");
                ExitCode::from(1)
            }
        },
        Ok(CommandOutput::CheckConfig(file)) => {
            match print_check_config(file.as_deref(), json, &mut stdout) {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => {
                    let _ = stderr.write_all(b"failed to write output\n");
                    ExitCode::from(1)
                }
            }
        }
        Ok(CommandOutput::ModelsList(entries)) => {
            match print_models_list(&entries, json, &mut stdout) {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => {
                    let _ = stderr.write_all(b"failed to write output\n");
                    ExitCode::from(1)
                }
            }
        }
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
                    location: None,
                }),
            };
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
    let loaded = load_config(&args)?;
    match args.command {
        Command::File(file_args) => run_file(file_args, &loaded.config, |_, _| Ok(engine)),
        _ => Err(Error::invalid_arguments(
            "run_with_engine only supports the file subcommand",
        )),
    }
}

fn run_command<F>(args: Args, make_engine: F) -> Result<CommandOutput, Error>
where
    F: FnOnce(&Path, i32) -> Result<Box<dyn Engine>, Error>,
{
    let loaded = load_config(&args)?;
    match args.command {
        Command::File(file_args) => {
            let output = run_file(file_args, &loaded.config, make_engine)?;
            Ok(CommandOutput::Transcription(output))
        }
        Command::CheckConfig(_) => Ok(CommandOutput::CheckConfig(match loaded.source {
            ConfigSource::File(path) => Some(path),
            ConfigSource::NoFile => None,
        })),
        Command::Models {
            command: ModelsCommand::List(_),
        } => Ok(CommandOutput::ModelsList(build_model_list(&loaded.config))),
    }
}

fn load_config(args: &Args) -> Result<LoadedConfig, Error> {
    config::load(args.config.as_deref()).map_err(Error::invalid_config)
}

fn run_file<F>(file_args: FileArgs, config: &Config, make_engine: F) -> Result<SuccessOutput, Error>
where
    F: FnOnce(&Path, i32) -> Result<Box<dyn Engine>, Error>,
{
    let language = effective_language(file_args.language, config.language.clone())?;
    let threads = effective_threads(file_args.threads, config.threads)?;

    let model_path = resolve_model_path(config, file_args.model, file_args.model_file)?;

    let file = open_regular_file(&file_args.audio)?;
    let metadata = file.metadata()?;
    if metadata.len() > MAX_FILE_BYTES {
        return Err(Error::input_too_large(metadata.len()));
    }

    let budgeted = BudgetReader::new(file, MAX_FILE_BYTES);
    let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| decode_audio(budgeted)))
        .map_err(|_| Error::invalid_input("audio decoding panicked"))?
        .map_err(map_budget_error)?;
    let resampled = resample_to_target(&decoded.samples, decoded.sample_rate)?;
    let pcm = normalize_and_validate(resampled)?;
    let audio_seconds = pcm.len() as f64 / 16_000.0;

    if !model_path.is_file() {
        return Err(Error::model_not_found(&model_path));
    }

    let start = Instant::now();
    let engine = make_engine(&model_path, threads)?;
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

fn resolve_model_path(
    config: &Config,
    model_id: Option<String>,
    model_file: Option<PathBuf>,
) -> Result<PathBuf, Error> {
    match (model_id, model_file) {
        (Some(_), Some(_)) => Err(Error::invalid_arguments(
            "--model and --model-file are mutually exclusive",
        )),
        (None, Some(path)) => Ok(path),
        (Some(id), None) => {
            let entry = config
                .models
                .iter()
                .find(|m| m.id == id)
                .ok_or_else(|| Error::invalid_arguments(unknown_model_message(&id, config)))?;
            Ok(entry.path.clone())
        }
        (None, None) => match &config.default_model {
            Some(id) => {
                let entry = config
                    .models
                    .iter()
                    .find(|m| m.id == *id)
                    .expect("default_model was validated against models");
                Ok(entry.path.clone())
            }
            None => Err(Error::invalid_arguments(
                "no model selected; use --model ID, --model-file PATH, or set default_model in transcribe.yml",
            )),
        },
    }
}

fn unknown_model_message(id: &str, config: &Config) -> String {
    let ids: Vec<&str> = config.models.iter().map(|m| m.id.as_str()).collect();
    let list = if ids.is_empty() {
        "(none configured)".to_string()
    } else {
        ids.join(", ")
    };
    format!("unknown model id `{id}`; configured ids: {list}")
}

fn effective_language(
    cli: Option<String>,
    from_config: Option<String>,
) -> Result<Option<String>, Error> {
    normalize_language(cli.or(from_config))
}

fn effective_threads(cli: Option<usize>, from_config: Option<usize>) -> Result<i32, Error> {
    normalize_threads(cli.or(from_config))
}

fn build_model_list(config: &Config) -> Vec<ModelListEntry> {
    config
        .models
        .iter()
        .map(|entry| {
            let (exists, size) = match std::fs::metadata(&entry.path) {
                Ok(meta) => (meta.is_file(), Some(meta.len())),
                Err(_) => (false, None),
            };
            ModelListEntry {
                id: entry.id.clone(),
                path: entry.path.clone(),
                exists,
                size,
                default: config.default_model.as_ref() == Some(&entry.id),
            }
        })
        .collect()
}

fn open_regular_file(path: &Path) -> Result<std::fs::File, Error> {
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

struct BudgetReader<R> {
    inner: R,
    budget: u64,
    read: u64,
    overshot: bool,
}

impl<R> BudgetReader<R> {
    fn new(inner: R, budget: u64) -> Self {
        Self {
            inner,
            budget,
            read: 0,
            overshot: false,
        }
    }
}

impl<R: Read> Read for BudgetReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.overshot {
            return Err(io::Error::other("byte budget exceeded"));
        }
        if self.read >= self.budget {
            let mut probe = [0u8; 1];
            let n = self.inner.read(&mut probe)?;
            if n == 0 {
                return Ok(0);
            }
            self.overshot = true;
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

fn print_check_config(file: Option<&Path>, json: bool, stdout: &mut dyn Write) -> io::Result<()> {
    if json {
        let envelope = Envelope {
            schema_version: 1,
            ok: true,
            result: Some(serde_json::to_value(CheckConfigResult {
                file: file.map(Path::to_path_buf),
            })?),
            error: None,
        };
        writeln!(stdout, "{}", serde_json::to_string(&envelope).unwrap())
    } else {
        match file {
            Some(path) => writeln!(stdout, "config ok: {}", path.display()),
            None => writeln!(stdout, "config ok: no per-user file found"),
        }
    }
}

fn print_models_list(
    entries: &[ModelListEntry],
    json: bool,
    stdout: &mut dyn Write,
) -> io::Result<()> {
    if json {
        let envelope = Envelope {
            schema_version: 1,
            ok: true,
            result: Some(serde_json::to_value(entries)?),
            error: None,
        };
        writeln!(stdout, "{}", serde_json::to_string(&envelope).unwrap())
    } else {
        if entries.is_empty() {
            writeln!(stdout, "No models configured.")?;
            return Ok(());
        }
        let id_width = entries.iter().map(|e| e.id.len()).max().unwrap_or(0).max(2);
        let path_width = entries
            .iter()
            .map(|e| e.path.to_string_lossy().len())
            .max()
            .unwrap_or(0)
            .max(4);
        writeln!(
            stdout,
            "{:<id_width$}  {:<path_width$}  {:<6}  {:>12}  {:<7}",
            "ID", "PATH", "EXISTS", "SIZE", "DEFAULT",
        )?;
        for entry in entries {
            let size = entry
                .size
                .map(|s| s.to_string())
                .unwrap_or_else(|| "-".to_string());
            writeln!(
                stdout,
                "{:<id_width$}  {:<path_width$}  {:<6}  {:>12}  {:<7}",
                entry.id,
                entry.path.display(),
                if entry.exists { "yes" } else { "no" },
                size,
                if entry.default { "yes" } else { "no" },
                id_width = id_width,
                path_width = path_width,
            )?;
        }
        Ok(())
    }
}

pub fn exit_code(error: &Error) -> ExitCode {
    let code = match error.code {
        ErrorCode::InvalidArguments
        | ErrorCode::InvalidInput
        | ErrorCode::InputTooLarge
        | ErrorCode::AudioTooLong
        | ErrorCode::UnsupportedFormat
        | ErrorCode::InvalidConfig
        | ErrorCode::ModelNotFound => 2,
        ErrorCode::EngineNotAvailable
        | ErrorCode::EngineFailed
        | ErrorCode::ModelLoadFailed
        | ErrorCode::Internal => 1,
    };
    ExitCode::from(code)
}

fn json_flag(args: &Args) -> bool {
    match &args.command {
        Command::File(args) => args.json,
        Command::CheckConfig(args) => args.json,
        Command::Models {
            command: ModelsCommand::List(args),
        } => args.json,
    }
}

fn normalize_language(language: Option<String>) -> Result<Option<String>, Error> {
    let Some(lang) = language else {
        return Ok(None);
    };
    validate_language(&lang)
        .map(Some)
        .map_err(Error::invalid_arguments)
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
    fn budget_reader_one_under_limit_reads_all() {
        let data = vec![0u8; MAX_FILE_BYTES as usize - 1];
        let mut reader = BudgetReader::new(std::io::Cursor::new(&data), MAX_FILE_BYTES);
        let mut out = Vec::new();
        reader.read_to_end(&mut out).unwrap();
        assert_eq!(out.len(), data.len());
    }

    #[test]
    fn budget_reader_exact_limit_reads_all() {
        let data = vec![0u8; MAX_FILE_BYTES as usize];
        let mut reader = BudgetReader::new(std::io::Cursor::new(&data), MAX_FILE_BYTES);
        let mut out = Vec::new();
        reader.read_to_end(&mut out).unwrap();
        assert_eq!(out.len(), data.len());
    }

    #[test]
    fn budget_reader_one_over_limit_rejects() {
        let data = vec![0u8; MAX_FILE_BYTES as usize + 1];
        let mut reader = BudgetReader::new(std::io::Cursor::new(&data), MAX_FILE_BYTES);
        let mut out = Vec::new();
        let err = reader.read_to_end(&mut out).unwrap_err();
        assert!(err.to_string().contains("byte budget exceeded"));
    }

    #[test]
    fn normalize_threads_rejects_too_large() {
        assert_eq!(
            normalize_threads(Some(33)).unwrap_err().code,
            ErrorCode::InvalidArguments
        );
    }
}
