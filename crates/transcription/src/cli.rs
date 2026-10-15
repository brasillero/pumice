use crate::catalog;
use crate::config::{self, Config, ConfigSource, LoadedConfig};
use crate::download::{self, DownloadSpec, Downloader, FetchResult};
use crate::engine::{CancelToken, Engine};
use crate::models_dir;
use crate::service::{
    decode_audio_from_reader, normalize_language, normalize_threads, resolve_model_id,
    transcribe_pcm,
};
use crate::{Error, ErrorCode, JsonError, TranscriptionOptions};
use clap::error::ErrorKind;
use clap::{Args as ClapArgs, Parser, Subcommand};
use serde::Serialize;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    /// Serve the HTTP API.
    Serve(ServeArgs),
    /// Validate the configuration file and exit.
    CheckConfig(CheckConfigArgs),
    /// Inspect configured models and the built-in catalog.
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

#[derive(ClapArgs, Debug)]
pub struct ServeArgs {
    /// Model id to serve. Defaults to the configured `default_model`.
    #[arg(long, value_name = "ID")]
    model: Option<String>,

    /// Port to listen on. Overrides `server.port` in the configuration file.
    #[arg(long, value_name = "PORT")]
    port: Option<u16>,
}

#[derive(Subcommand, Debug)]
pub enum ModelsCommand {
    /// List configured models.
    List(ModelsListArgs),
    /// List the built-in downloadable catalog and what is installed.
    Catalog(ModelsCatalogArgs),
    /// Download a catalog model into the models directory.
    Download(ModelsDownloadArgs),
    /// Re-hash an installed catalog model against the catalog.
    Verify(ModelsVerifyArgs),
}

#[derive(ClapArgs, Debug)]
pub struct ModelsListArgs {
    /// Output JSON instead of a table.
    #[arg(long)]
    json: bool,
}

#[derive(ClapArgs, Debug)]
pub struct ModelsCatalogArgs {
    /// Output JSON instead of a table.
    #[arg(long)]
    json: bool,
}

#[derive(ClapArgs, Debug)]
pub struct ModelsDownloadArgs {
    /// Catalog model id (see `models catalog`).
    id: String,

    /// Replace an installed file that fails verification.
    #[arg(long)]
    force: bool,

    /// Output a JSON envelope instead of plain text. Progress is reported as
    /// JSON Lines on stderr.
    #[arg(long)]
    json: bool,
}

#[derive(ClapArgs, Debug)]
pub struct ModelsVerifyArgs {
    /// Catalog model id (see `models catalog`).
    id: String,

    /// Output a JSON envelope instead of plain text.
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
    /// Compute backend in use, e.g. `CPU [haswell]`; only present in builds
    /// that can know it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
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
    /// Built-in catalog id when the entry uses `catalog:` instead of `path`.
    #[serde(skip_serializing_if = "Option::is_none")]
    catalog: Option<String>,
    default: bool,
}

#[derive(Debug, Clone, Serialize)]
struct CatalogListEntry {
    id: &'static str,
    description: &'static str,
    file_name: &'static str,
    size: u64,
    sha256: &'static str,
    license: &'static str,
    /// The file is present with the expected size. Only `models verify`
    /// re-hashes it.
    installed: bool,
}

#[derive(Debug, Clone, Serialize)]
struct DownloadOutput {
    model: String,
    file: PathBuf,
    bytes: u64,
    sha256: String,
    already_installed: bool,
}

#[derive(Debug, Clone, Serialize)]
struct VerifyOutput {
    model: String,
    file: PathBuf,
    bytes: u64,
    sha256: String,
}

enum CommandOutput {
    Transcription(SuccessOutput),
    CheckConfig(Option<PathBuf>),
    ModelsList(Vec<ModelListEntry>),
    ModelsCatalog(Vec<CatalogListEntry>),
    Download(DownloadOutput),
    Verify(VerifyOutput),
    None,
}

impl From<SuccessOutput> for serde_json::Value {
    fn from(out: SuccessOutput) -> Self {
        let mut result = serde_json::json!({
            "text": out.text,
            "language": out.language,
            "audio_seconds": out.audio_seconds,
            "model_load_ms": out.model_load_ms,
            "transcription_ms": out.transcription_ms,
        });
        if let Some(backend) = out.backend {
            result["backend"] = serde_json::Value::String(backend);
        }
        result
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
        Ok(CommandOutput::ModelsCatalog(entries)) => {
            match print_models_catalog(&entries, json, &mut stdout) {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => {
                    let _ = stderr.write_all(b"failed to write output\n");
                    ExitCode::from(1)
                }
            }
        }
        Ok(CommandOutput::Download(output)) => match print_download(&output, json, &mut stdout) {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => {
                let _ = stderr.write_all(b"failed to write output\n");
                ExitCode::from(1)
            }
        },
        Ok(CommandOutput::Verify(output)) => match print_verify(&output, json, &mut stdout) {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => {
                let _ = stderr.write_all(b"failed to write output\n");
                ExitCode::from(1)
            }
        },
        Ok(CommandOutput::None) => ExitCode::SUCCESS,
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
        Command::File(file_args) => {
            run_file(file_args, &loaded.config, args.config.as_deref(), |_, _| {
                Ok(engine)
            })
        }
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
    let config_arg = args.config.as_deref();
    match args.command {
        Command::File(file_args) => {
            let output = run_file(file_args, &loaded.config, config_arg, make_engine)?;
            Ok(CommandOutput::Transcription(output))
        }
        Command::Serve(serve_args) => {
            run_serve(serve_args, &loaded.config, config_arg, make_engine)?;
            Ok(CommandOutput::None)
        }
        Command::CheckConfig(_) => Ok(CommandOutput::CheckConfig(match loaded.source {
            ConfigSource::File(path) => Some(path),
            ConfigSource::NoFile => None,
        })),
        Command::Models { command } => run_models_command(command, &loaded.config),
    }
}

fn run_models_command(command: ModelsCommand, config: &Config) -> Result<CommandOutput, Error> {
    match command {
        ModelsCommand::List(_) => Ok(CommandOutput::ModelsList(build_model_list(config))),
        ModelsCommand::Catalog(_) => {
            let dir = resolve_models_dir(config)?;
            Ok(CommandOutput::ModelsCatalog(build_catalog_list(&dir)))
        }
        ModelsCommand::Download(args) => {
            let entry = find_catalog_entry(&args.id)?;
            let dir = resolve_models_dir(config)?;
            let spec = DownloadSpec::from(entry);

            let mut stderr = io::stderr();
            let mut reporter =
                ProgressReporter::new(&mut stderr, args.json, entry.id, spec.size_bytes);
            let downloader = Downloader::new(
                download::RedirectPolicy::HttpsOnly,
                download::Timeouts::default(),
            );
            let result = downloader.fetch(&spec, &dir, args.force, &mut |event| match event {
                download::DownloadEvent::Progress { received, .. } => {
                    // Progress is best-effort; never fail the download over it.
                    let _ = reporter.tick(received);
                }
                download::DownloadEvent::State(state) => {
                    let _ = reporter.state(state);
                }
            })?;

            let (already_installed, path, bytes, sha256) = match result {
                FetchResult::AlreadyInstalled {
                    path,
                    bytes,
                    sha256,
                } => (true, path, bytes, sha256),
                FetchResult::Downloaded {
                    path,
                    bytes,
                    sha256,
                } => (false, path, bytes, sha256),
            };
            Ok(CommandOutput::Download(DownloadOutput {
                model: entry.id.to_string(),
                file: path,
                bytes,
                sha256,
                already_installed,
            }))
        }
        ModelsCommand::Verify(args) => {
            let entry = find_catalog_entry(&args.id)?;
            let dir = resolve_models_dir(config)?;
            let spec = DownloadSpec::from(entry);
            let path = dir.join(entry.file_name);
            let verified = download::verify_file(&path, &spec)?;
            Ok(CommandOutput::Verify(VerifyOutput {
                model: entry.id.to_string(),
                file: path,
                bytes: verified.bytes,
                sha256: verified.sha256,
            }))
        }
    }
}

fn find_catalog_entry(id: &str) -> Result<&'static catalog::CatalogEntry, Error> {
    catalog::find(id).ok_or_else(|| {
        Error::invalid_arguments(format!(
            "unknown catalog model \"{id}\"; run `pumice-transcribe models catalog` to list available models"
        ))
    })
}

/// Where downloaded catalog models go: the configured `models_dir`, else the
/// per-user data directory (see `models_dir::default_models_dir`).
fn resolve_models_dir(config: &Config) -> Result<PathBuf, Error> {
    config
        .models_dir
        .clone()
        .or_else(|| models_dir::default_models_dir(&|key| std::env::var_os(key)))
        .ok_or_else(|| {
            Error::invalid_arguments(
                "no models directory: set models_dir in transcribe.yml or define XDG_DATA_HOME or HOME",
            )
        })
}

/// Error for a missing model file. A `catalog:` entry names the command that
/// fixes it — preserving an explicit `--config`, since a `models_dir` override
/// only takes effect through the same config file.
fn missing_model_error(path: &Path, catalog: Option<&str>, config_arg: Option<&Path>) -> Error {
    let message = match catalog {
        Some(id) => {
            let command = match config_arg {
                Some(config) => format!(
                    "pumice-transcribe --config {} models download {id}",
                    shell_quote(&config.to_string_lossy())
                ),
                None => format!("pumice-transcribe models download {id}"),
            };
            format!(
                "model file not found: {} (catalog model `{id}`); download it with: {command}",
                path.display()
            )
        }
        None => format!("model file not found: {}", path.display()),
    };
    Error::new(ErrorCode::ModelNotFound, message)
}

/// Quotes an argument for POSIX-style shells when it contains characters
/// outside the always-safe set, so the printed command is executable as
/// written even when the config path contains spaces or shell specials.
fn shell_quote(arg: &str) -> String {
    #[cfg(windows)]
    const SAFE: &[char] = &['_', '-', '.', ',', '/', '\\', ':', '@', '+', '=', '~'];
    #[cfg(not(windows))]
    const SAFE: &[char] = &['_', '-', '.', ',', '/', ':', '@', '%', '+', '=', '~'];
    if !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || SAFE.contains(&c))
    {
        arg.to_string()
    } else if cfg!(windows) {
        // cmd.exe and PowerShell both take double quotes; Windows paths cannot
        // contain `"`.
        format!("\"{arg}\"")
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

/// The catalog id of the configured entry `model_id` names, if any.
fn catalog_of<'a>(config: &'a Config, model_id: &str) -> Option<&'a str> {
    config
        .models
        .iter()
        .find(|m| m.id == model_id)
        .and_then(|m| m.catalog.as_deref())
}

fn run_serve<F>(
    serve_args: ServeArgs,
    config: &Config,
    config_arg: Option<&Path>,
    make_engine: F,
) -> Result<(), Error>
where
    F: FnOnce(&Path, i32) -> Result<Box<dyn Engine>, Error>,
{
    let threads = normalize_threads(config.threads)?;
    let (served_model_id, model_path) =
        resolve_model_id(config, serve_args.model.as_deref(), None)?;

    if !model_path.is_file() {
        return Err(missing_model_error(
            &model_path,
            catalog_of(config, &served_model_id),
            config_arg,
        ));
    }

    let port = serve_args.port.or(config.server_port).ok_or_else(|| {
        Error::invalid_arguments(
            "no port specified; use --port PORT or set server.port in transcribe.yml",
        )
    })?;

    let start = Instant::now();
    let engine = make_engine(&model_path, threads)?;
    let load_ms = start.elapsed().as_millis();
    match engine.backend_label() {
        Some(label) => {
            eprintln!("loaded model `{served_model_id}` in {load_ms} ms (backend: {label})")
        }
        None => eprintln!("loaded model `{served_model_id}` in {load_ms} ms"),
    }

    let rt = tokio::runtime::Runtime::new().map_err(|e| Error::internal(format!("{e}")))?;
    rt.block_on(async {
        let handle = crate::server::serve(
            port,
            Arc::from(engine),
            served_model_id,
            config.language.clone(),
            crate::server::shutdown_signal(),
        )
        .await
        .map_err(|e| Error::internal(format!("failed to bind: {e}")))?;
        let _ = handle.await;
        Ok(())
    })
}

fn load_config(args: &Args) -> Result<LoadedConfig, Error> {
    config::load(args.config.as_deref()).map_err(Error::invalid_config)
}

fn run_file<F>(
    file_args: FileArgs,
    config: &Config,
    config_arg: Option<&Path>,
    make_engine: F,
) -> Result<SuccessOutput, Error>
where
    F: FnOnce(&Path, i32) -> Result<Box<dyn Engine>, Error>,
{
    let language = normalize_language(file_args.language.or_else(|| config.language.clone()))?;
    let threads = normalize_threads(file_args.threads.or(config.threads))?;

    let (model_id, model_path) = resolve_model_id(
        config,
        file_args.model.as_deref(),
        file_args.model_file.as_deref(),
    )?;

    let file = open_regular_file(&file_args.audio)?;
    let metadata = file.metadata()?;
    if metadata.len() > crate::audio::MAX_FILE_BYTES {
        return Err(Error::input_too_large(metadata.len()));
    }

    let (pcm, audio_seconds) = decode_audio_from_reader(file)?;

    if !model_path.is_file() {
        return Err(missing_model_error(
            &model_path,
            catalog_of(config, &model_id),
            config_arg,
        ));
    }

    let start = Instant::now();
    let engine = make_engine(&model_path, threads)?;
    let model_load_ms = start.elapsed().as_millis() as u64;

    let start = Instant::now();
    let transcript = transcribe_pcm(
        engine.as_ref(),
        &pcm,
        &TranscriptionOptions {
            language: language.clone(),
            prompt: None,
            temperature: None,
        },
        &CancelToken::new(),
    )?;
    let transcription_ms = start.elapsed().as_millis() as u64;

    Ok(SuccessOutput {
        text: transcript.text,
        language: language.or(transcript.language),
        audio_seconds,
        model_load_ms,
        transcription_ms,
        backend: engine.backend_label(),
    })
}

fn build_model_list(config: &Config) -> Vec<ModelListEntry> {
    config
        .models
        .iter()
        .map(|entry| {
            let (exists, size) = match std::fs::metadata(&entry.path) {
                Ok(meta) if meta.is_file() => (true, Some(meta.len())),
                _ => (false, None),
            };
            ModelListEntry {
                id: entry.id.clone(),
                path: entry.path.clone(),
                exists,
                size,
                catalog: entry.catalog.clone(),
                default: config.default_model.as_ref() == Some(&entry.id),
            }
        })
        .collect()
}

fn build_catalog_list(models_dir: &Path) -> Vec<CatalogListEntry> {
    catalog::CATALOG
        .iter()
        .map(|entry| {
            let installed = std::fs::metadata(models_dir.join(entry.file_name))
                .map(|meta| meta.is_file() && meta.len() == entry.size_bytes)
                .unwrap_or(false);
            CatalogListEntry {
                id: entry.id,
                description: entry.description,
                file_name: entry.file_name,
                size: entry.size_bytes,
                sha256: entry.sha256,
                license: entry.license,
                installed,
            }
        })
        .collect()
}

/// Download progress and phase transitions designed for a future UI
/// (`arch-modules/D10`): with `json`, one JSON Line per tick and per phase
/// change on stderr (`progress`:
/// `{"schema_version":1,"event":"progress","model":ID,"bytes":N,"total":M}`;
/// `state`:
/// `{"schema_version":1,"event":"state","model":ID,"state":"connecting|downloading|verifying|installing"}`);
/// otherwise a single human-readable line on stderr that overwrites itself
/// (state events are JSON-only). At most ~4 progress events per second, plus
/// the first and the completing tick.
struct ProgressReporter<'a> {
    writer: &'a mut dyn Write,
    json: bool,
    model: &'a str,
    total: u64,
    last_sent: Option<Instant>,
}

impl<'a> ProgressReporter<'a> {
    const MIN_INTERVAL: Duration = Duration::from_millis(250);

    fn new(writer: &'a mut dyn Write, json: bool, model: &'a str, total: u64) -> Self {
        Self {
            writer,
            json,
            model,
            total,
            last_sent: None,
        }
    }

    /// Emits a phase transition as a JSONL `state` event (JSON mode only).
    fn state(&mut self, state: download::DownloadState) -> io::Result<()> {
        if !self.json {
            return Ok(());
        }
        let line = serde_json::json!({
            "schema_version": 1,
            "event": "state",
            "model": self.model,
            "state": state.as_str(),
        });
        writeln!(self.writer, "{line}")
    }

    fn tick(&mut self, bytes: u64) -> io::Result<()> {
        let complete = bytes >= self.total;
        if !complete
            && self
                .last_sent
                .is_some_and(|last| last.elapsed() < Self::MIN_INTERVAL)
        {
            return Ok(());
        }
        self.last_sent = Some(Instant::now());
        if self.json {
            let line = serde_json::json!({
                "schema_version": 1,
                "event": "progress",
                "model": self.model,
                "bytes": bytes,
                "total": self.total,
            });
            writeln!(self.writer, "{line}")
        } else {
            let percent = bytes
                .saturating_mul(100)
                .checked_div(self.total)
                .unwrap_or(100);
            write!(
                self.writer,
                "\r{}: {} / {} bytes ({}%)",
                self.model, bytes, self.total, percent
            )?;
            if complete {
                writeln!(self.writer)?;
            }
            self.writer.flush()
        }
    }
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
            "{:<id_width$}  {:<path_width$}  {:<8}  {:>12}  {:<7}",
            "ID", "PATH", "EXISTS", "SIZE", "DEFAULT",
        )?;
        for entry in entries {
            let size = entry
                .size
                .map(|s| s.to_string())
                .unwrap_or_else(|| "-".to_string());
            writeln!(
                stdout,
                "{:<id_width$}  {:<path_width$}  {:<8}  {:>12}  {:<7}",
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

fn print_models_catalog(
    entries: &[CatalogListEntry],
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
        let id_width = entries.iter().map(|e| e.id.len()).max().unwrap_or(0).max(2);
        let size_width = entries
            .iter()
            .map(|e| e.size.to_string().len())
            .max()
            .unwrap_or(0)
            .max(4);
        writeln!(
            stdout,
            "{:<id_width$}  {:<size_width$}  {:<9}  LICENSE",
            "ID", "SIZE", "INSTALLED",
        )?;
        for entry in entries {
            writeln!(
                stdout,
                "{:<id_width$}  {:<size_width$}  {:<9}  {}",
                entry.id,
                entry.size,
                if entry.installed { "yes" } else { "no" },
                entry.license,
                id_width = id_width,
                size_width = size_width,
            )?;
            writeln!(stdout, "    {}", entry.description)?;
        }
        Ok(())
    }
}

fn print_download(output: &DownloadOutput, json: bool, stdout: &mut dyn Write) -> io::Result<()> {
    if json {
        let envelope = Envelope {
            schema_version: 1,
            ok: true,
            result: Some(serde_json::to_value(output)?),
            error: None,
        };
        writeln!(stdout, "{}", serde_json::to_string(&envelope).unwrap())
    } else if output.already_installed {
        writeln!(stdout, "already installed: {}", output.file.display())
    } else {
        writeln!(
            stdout,
            "installed {} ({} bytes): {}",
            output.model,
            output.bytes,
            output.file.display()
        )
    }
}

fn print_verify(output: &VerifyOutput, json: bool, stdout: &mut dyn Write) -> io::Result<()> {
    if json {
        let envelope = Envelope {
            schema_version: 1,
            ok: true,
            result: Some(serde_json::to_value(output)?),
            error: None,
        };
        writeln!(stdout, "{}", serde_json::to_string(&envelope).unwrap())
    } else {
        writeln!(
            stdout,
            "verified {}: {} ({} bytes)",
            output.model,
            output.file.display(),
            output.bytes
        )
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
        | ErrorCode::ChecksumMismatch
        | ErrorCode::DownloadFailed
        | ErrorCode::Cancelled
        | ErrorCode::Internal => 1,
    };
    ExitCode::from(code)
}

fn json_flag(args: &Args) -> bool {
    match &args.command {
        Command::File(args) => args.json,
        Command::Serve(_) => false,
        Command::CheckConfig(args) => args.json,
        Command::Models { command } => match command {
            ModelsCommand::List(args) => args.json,
            ModelsCommand::Catalog(args) => args.json,
            ModelsCommand::Download(args) => args.json,
            ModelsCommand::Verify(args) => args.json,
        },
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

    fn success_output() -> SuccessOutput {
        SuccessOutput {
            text: "hello".to_string(),
            language: None,
            audio_seconds: 1.0,
            model_load_ms: 0,
            transcription_ms: 0,
            backend: None,
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn shell_quote_leaves_simple_paths_alone() {
        assert_eq!(
            shell_quote("/home/user/transcribe.yml"),
            "/home/user/transcribe.yml"
        );
        assert_eq!(shell_quote("rel/dir/config.yml"), "rel/dir/config.yml");
    }

    #[cfg(not(windows))]
    #[test]
    fn shell_quote_quotes_paths_with_spaces_or_specials() {
        assert_eq!(
            shell_quote("/tmp/config with spaces.yml"),
            "'/tmp/config with spaces.yml'"
        );
        assert_eq!(shell_quote("/tmp/it's.yml"), "'/tmp/it'\\''s.yml'");
        let command = missing_model_error(
            Path::new("/models/model.gguf"),
            Some("whisper-tiny-q8_0"),
            Some(Path::new("/tmp/config with spaces.yml")),
        )
        .message;
        assert!(
            command.contains("--config '/tmp/config with spaces.yml' models download"),
            "unexpected message: {command}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn shell_quote_uses_double_quotes_on_windows() {
        assert_eq!(
            shell_quote(r"C:\Users\me\transcribe.yml"),
            r"C:\Users\me\transcribe.yml"
        );
        assert_eq!(
            shell_quote(r"C:\Users\my name\transcribe.yml"),
            r#""C:\Users\my name\transcribe.yml""#
        );
    }

    #[test]
    fn print_success_plain_does_not_duplicate_trailing_newline() {
        let mut output = success_output();
        output.text = "hello\n".to_string();
        let mut buf = Vec::new();
        print_success(&output, false, &mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "hello\n");
    }

    #[test]
    fn print_success_plain_adds_newline_when_missing() {
        let mut buf = Vec::new();
        print_success(&success_output(), false, &mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "hello\n");
    }

    #[test]
    fn print_success_returns_error_on_failed_write() {
        let mut writer = FailingWriter;
        assert!(print_success(&success_output(), false, &mut writer).is_err());
    }

    #[test]
    fn print_error_returns_error_on_failed_write() {
        let error = Error::engine_failed();
        let mut stdout = FailingWriter;
        let mut stderr = Vec::new();
        assert!(print_error(&error, true, &mut stdout, &mut stderr).is_err());
    }

    #[test]
    fn download_exit_codes_are_stable() {
        assert_eq!(exit_code(&Error::checksum_mismatch("x")), ExitCode::from(1));
        assert_eq!(exit_code(&Error::download_failed("x")), ExitCode::from(1));
    }

    #[test]
    fn build_catalog_list_marks_installed_by_size() {
        let tmp = tempfile::tempdir().unwrap();
        let tiny = catalog::find("whisper-tiny-q8_0").unwrap();
        let file = std::fs::File::create(tmp.path().join(tiny.file_name)).unwrap();
        file.set_len(tiny.size_bytes).unwrap();
        let entries = build_catalog_list(tmp.path());
        assert!(!entries.is_empty());
        for entry in &entries {
            assert_eq!(
                entry.installed,
                entry.id == tiny.id,
                "only the present, right-sized file counts as installed"
            );
        }
    }

    #[test]
    fn build_model_list_includes_catalog_source() {
        let config = Config {
            models: vec![crate::config::ModelEntry {
                id: "tiny".to_string(),
                path: PathBuf::from("/models/whisper-tiny-Q8_0.gguf"),
                catalog: Some("whisper-tiny-q8_0".to_string()),
            }],
            ..Config::default()
        };
        let entries = build_model_list(&config);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].catalog.as_deref(), Some("whisper-tiny-q8_0"));
        assert!(!entries[0].exists);
    }

    #[test]
    fn progress_json_lines_have_the_documented_shape() {
        let mut buf = Vec::new();
        {
            let mut reporter = ProgressReporter::new(&mut buf, true, "whisper-tiny-q8_0", 1_000);
            reporter.tick(100).unwrap();
            reporter.tick(200).unwrap(); // throttled: too soon after the first
            reporter.tick(1_000).unwrap(); // completion always reported
        }
        let text = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "the middle tick must be throttled away");
        for line in &lines {
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(value["schema_version"], 1);
            assert_eq!(value["event"], "progress");
            assert_eq!(value["model"], "whisper-tiny-q8_0");
            assert_eq!(value["total"], 1_000);
            assert!(value["bytes"].as_u64().unwrap() <= 1_000);
        }
        let last: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(last["bytes"], 1_000);
    }

    #[test]
    fn progress_state_lines_have_the_documented_shape() {
        let mut buf = Vec::new();
        {
            let mut reporter = ProgressReporter::new(&mut buf, true, "whisper-tiny-q8_0", 1_000);
            reporter.state(download::DownloadState::Connecting).unwrap();
            reporter.state(download::DownloadState::Installing).unwrap();
        }
        let text = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        let expected = ["connecting", "installing"];
        for (line, state) in lines.iter().zip(expected) {
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(value["schema_version"], 1);
            assert_eq!(value["event"], "state");
            assert_eq!(value["model"], "whisper-tiny-q8_0");
            assert_eq!(value["state"], state);
        }
    }

    #[test]
    fn progress_human_line_shows_percent_and_newline() {
        let mut buf = Vec::new();
        {
            let mut reporter = ProgressReporter::new(&mut buf, false, "model", 4);
            reporter.tick(1).unwrap();
            reporter.tick(4).unwrap();
        }
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("(25%)"));
        assert!(text.contains("(100%)"));
        assert!(text.ends_with('\n'));
    }
}
