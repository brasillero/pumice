use crate::catalog;
use crate::compute::{BackendMode, Compute};
use crate::config::{self, Config, LoadedConfig, Threads};
use crate::download::{self, DownloadSpec, Downloader, FetchResult};
use crate::engine::{CancelToken, Engine, EngineRequest};
use crate::inventory::{self, Diagnostic, Inventory};
use crate::managed::{self, Lifecycle, StopReason};
use crate::models_dir;
use crate::service::{
    decode_audio_from_reader, normalize_language, normalize_threads, transcribe_pcm,
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
use tokio_util::sync::CancellationToken;

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
    /// Transcribe an audio file. With --model-file it also runs without a
    /// configuration file, from its flags and their defaults.
    File(FileArgs),
    /// Serve the HTTP API.
    Serve(ServeArgs),
    /// Validate the configuration file and the models folder, and exit.
    CheckConfig(CheckConfigArgs),
    /// Inspect the models folder and the built-in catalog.
    Models {
        #[command(subcommand)]
        command: ModelsCommand,
    },
    /// Show the compute backends this build ships and what registers in a
    /// backend mode, without loading a model.
    Backends(BackendsArgs),
}

#[derive(ClapArgs, Debug)]
#[command(group = clap::ArgGroup::new("model_source").args(["model", "model_file"]).required(false))]
pub struct FileArgs {
    /// Path to the audio file (WAV, MP3, FLAC, Ogg Vorbis).
    audio: PathBuf,

    /// Model id: a model in the models folder (its catalog id or file
    /// name without `.gguf`). Defaults to the configured `default_model`.
    #[arg(
        long,
        value_name = "ID",
        group = "model_source",
        allow_hyphen_values = true
    )]
    model: Option<String>,

    /// Direct path to a GGUF model file, named by the models folder's rule
    /// in the result. With it, `file` also runs without a configuration
    /// file: the other settings then come from the flags and their
    /// defaults. A configuration file that exists must be complete.
    #[arg(long, value_name = "PATH.gguf", group = "model_source")]
    model_file: Option<PathBuf>,

    /// Optional language hint (ISO-639-1 two-letter code). Overrides
    /// `language` in the configuration file; without one, the language is
    /// detected. When given, this value is reported in the result; otherwise
    /// the engine-detected language is reported.
    #[arg(long, value_name = "xx")]
    language: Option<String>,

    /// Number of threads to use for inference (1-32). Overrides `threads` in
    /// the configuration file; without one, the available parallelism,
    /// capped at 32.
    #[arg(long, value_name = "N")]
    threads: Option<usize>,

    /// Compute backend. Overrides `backend` in the configuration file;
    /// without one, cpu.
    #[arg(long, value_name = "MODE")]
    backend: Option<BackendMode>,

    /// Output a JSON envelope instead of plain text.
    #[arg(long)]
    json: bool,
}

#[derive(ClapArgs, Debug)]
pub struct BackendsArgs {
    /// Backend mode to register for. Overrides `backend` in the
    /// configuration file; required when there is none.
    #[arg(long, value_name = "MODE")]
    backend: Option<BackendMode>,

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
    /// Model id to serve, from the models folder. Defaults to the configured
    /// `default_model`.
    #[arg(long, value_name = "ID", allow_hyphen_values = true)]
    model: Option<String>,

    /// Port to listen on. Overrides `server.port` in the configuration file.
    #[arg(long, value_name = "PORT")]
    port: Option<u16>,

    /// Compute backend. Overrides `backend` in the configuration file.
    #[arg(long, value_name = "MODE")]
    backend: Option<BackendMode>,

    /// Run as a child owned by `pumice serve` (internal protocol): lifecycle
    /// JSON Lines on stdout, shutdown when stdin closes.
    #[arg(long, requires = "instance_id")]
    managed: bool,

    /// Launch id from the owner, echoed in lifecycle records and `/ready`.
    #[arg(long, value_name = "ID", requires = "managed")]
    instance_id: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum ModelsCommand {
    /// List the models in the models folder.
    List(ModelsListArgs),
    /// List the built-in downloadable catalog and what is installed.
    Catalog(ModelsCatalogArgs),
    /// Download a catalog model into the models folder.
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
    /// The model id: its id in the models folder, or for a one-off
    /// `--model-file` the id the folder's naming rule gives the file.
    pub model: String,
    pub text: String,
    pub language: Option<String>,
    pub audio_seconds: f64,
    pub model_load_ms: u64,
    pub transcription_ms: u64,
    /// The loaded model's compute snapshot; present whenever the engine
    /// reports one (every native engine build).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compute: Option<Compute>,
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
    file: PathBuf,
}

/// `models list`: the models folder's snapshot.
#[derive(Debug, Clone, Serialize)]
struct ModelsListOutput {
    models_dir: PathBuf,
    models: Vec<ModelListEntry>,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize)]
struct ModelListEntry {
    id: String,
    file: PathBuf,
    size: u64,
    /// The file matched a catalog entry by size and SHA-256.
    catalog: bool,
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
    /// A model file of the expected size is in the models folder, under any
    /// name. `models list` and `models verify` hash it.
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
    CheckConfig(PathBuf),
    ModelsList(ModelsListOutput),
    ModelsCatalog(Vec<CatalogListEntry>),
    Download(DownloadOutput),
    Verify(VerifyOutput),
    Backends(serde_json::Value),
    None,
}

impl From<SuccessOutput> for serde_json::Value {
    fn from(out: SuccessOutput) -> Self {
        let mut result = serde_json::json!({
            "model": out.model,
            "text": out.text,
            "language": out.language,
            "audio_seconds": out.audio_seconds,
            "model_load_ms": out.model_load_ms,
            "transcription_ms": out.transcription_ms,
        });
        if let Some(compute) = out.compute {
            result["compute"] = compute.to_value();
        }
        result
    }
}

pub fn run<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let make_engine = |request: EngineRequest<'_>| -> Result<Box<dyn Engine>, Error> {
        #[cfg(feature = "engine")]
        {
            Ok(Box::new(crate::engine::cpp::TranscribeCppEngine::new(
                request,
            )?))
        }
        #[cfg(not(feature = "engine"))]
        {
            let _ = request;
            Err(Error::engine_not_available())
        }
    };
    run_with_engine_factory(args, make_engine)
}

/// [`run`] with another engine loader, for tests that drive the whole binary
/// (arguments, output, exit status) with a fake engine.
pub fn run_with_engine_factory<I, T, F>(args: I, make_engine: F) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
    F: FnOnce(EngineRequest<'_>) -> Result<Box<dyn Engine>, Error>,
{
    let raw: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let wants_json = raw.iter().any(|a| a.as_encoded_bytes() == b"--json");
    let wants_managed = raw.iter().any(|a| a.as_encoded_bytes() == b"--managed");

    let parsed = Args::try_parse_from(&raw);
    let args = match parsed {
        Ok(a) => a,
        Err(e)
            if wants_managed
                && !matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) =>
        {
            // The owner reads stdout; clap's text (which can quote any
            // argument) only goes to stderr.
            let _ = e.print();
            managed::write_startup_error(
                &mut io::stdout(),
                ErrorCode::InvalidArguments,
                managed::INVALID_ARGUMENTS,
            );
            return ExitCode::from(2);
        }
        Err(e) => {
            return handle_clap_error(e, wants_json);
        }
    };

    if matches!(&args.command, Command::Serve(serve) if serve.managed) {
        return run_managed(args, make_engine);
    }

    let json = json_flag(&args);
    let mut stdout = io::stdout();
    let mut stderr = io::stderr();

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
            match print_check_config(&file, json, &mut stdout) {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => {
                    let _ = stderr.write_all(b"failed to write output\n");
                    ExitCode::from(1)
                }
            }
        }
        Ok(CommandOutput::ModelsList(output)) => {
            match print_models_list(&output, json, &mut stdout, &mut stderr) {
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
        Ok(CommandOutput::Backends(report)) => match print_backends(&report, json, &mut stdout) {
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
    let config_arg = args.config.as_deref();
    match args.command {
        Command::File(file_args) => {
            let loaded = file_config(&file_args, config_arg)?;
            run_file(file_args, loaded.as_ref(), config_arg, |_| Ok(engine))
        }
        _ => Err(Error::invalid_arguments(
            "run_with_engine only supports the file subcommand",
        )),
    }
}

fn run_command<F>(args: Args, make_engine: F) -> Result<CommandOutput, Error>
where
    F: FnOnce(EngineRequest<'_>) -> Result<Box<dyn Engine>, Error>,
{
    let config_arg = args.config.as_deref();
    match args.command {
        Command::File(file_args) => {
            let loaded = file_config(&file_args, config_arg)?;
            let output = run_file(file_args, loaded.as_ref(), config_arg, make_engine)?;
            Ok(CommandOutput::Transcription(output))
        }
        Command::Serve(serve_args) => {
            let loaded = load_config(config_arg)?;
            match run_serve(serve_args, &loaded, config_arg, make_engine, None)? {
                Served::Stopped => Ok(CommandOutput::None),
                Served::Retired => Err(Error::new(ErrorCode::EngineFailed, RETIRED)),
            }
        }
        Command::CheckConfig(_) => {
            let loaded = load_config(config_arg)?;
            let inventory = discover(&loaded.config)?;
            warn(&inventory.diagnostics);
            if loaded.config.default_model.is_some() {
                select_model(&loaded, &inventory, None, config_arg)?;
            }
            Ok(CommandOutput::CheckConfig(loaded.path))
        }
        Command::Models { command } => run_models_command(command, config_arg),
        Command::Backends(backends_args) => {
            // A report like `models catalog`: it also runs before a
            // configuration exists, with the mode given on the command line.
            let loaded = load_optional_config(config_arg)?;
            let mode = match (backends_args.backend, loaded) {
                (Some(mode), _) => mode,
                (None, Some(loaded)) => loaded.config.backend,
                (None, None) => {
                    return Err(Error::invalid_arguments(
                        "no backend mode: pass --backend MODE, or a configuration file with --config FILE or as the per-user transcribe.yml",
                    ));
                }
            };
            backends(mode).map(CommandOutput::Backends)
        }
    }
}

fn backends(mode: BackendMode) -> Result<serde_json::Value, Error> {
    #[cfg(feature = "engine")]
    {
        crate::engine::cpp::backends_report(mode)
    }
    #[cfg(not(feature = "engine"))]
    {
        let _ = mode;
        Err(Error::engine_not_available())
    }
}

/// How a `serve` run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Served {
    /// Stopped on request (signal, or the managed lifeline).
    Stopped,
    /// Retired itself after a native engine failure on a GPU.
    Retired,
}

/// Why a standalone `serve` on a GPU exited after an engine failure.
const RETIRED: &str = "the GPU engine failed during a transcription; the server stopped so it \
     can be restarted. Use backend: cpu (or --backend cpu) if this repeats.";

/// The retirement token for an engine on a GPU: a native failure there that
/// may leave the engine unusable retires the process instead of trusting the
/// device again (p14.2-backend-policy/D1).
fn retire_token(compute: Option<&Compute>) -> Option<CancellationToken> {
    compute
        .is_some_and(|compute| compute.backend.is_gpu())
        .then(CancellationToken::new)
}

async fn retired(retire: Option<CancellationToken>) {
    match retire {
        Some(retire) => retire.cancelled().await,
        None => std::future::pending().await,
    }
}

/// `models list` needs the configuration; `catalog`, `download` and
/// `verify` also run before one exists, in the per-user models folder, and
/// whatever else the folder holds never blocks them.
fn run_models_command(
    command: ModelsCommand,
    config_arg: Option<&Path>,
) -> Result<CommandOutput, Error> {
    match command {
        ModelsCommand::List(_) => {
            let loaded = load_config(config_arg)?;
            let inventory = discover(&loaded.config)?;
            Ok(CommandOutput::ModelsList(build_model_list(
                &loaded.config,
                inventory,
            )))
        }
        ModelsCommand::Catalog(_) => {
            let dir = models_dir_for(config_arg)?;
            Ok(CommandOutput::ModelsCatalog(build_catalog_list(&dir)))
        }
        ModelsCommand::Download(args) => {
            let entry = find_catalog_entry(&args.id)?;
            let dir = models_dir_for(config_arg)?;
            let spec = DownloadSpec::from(entry);

            // A copy under another name is the same model (D8): a second
            // copy would make its id a duplicate.
            if let Some(path) = inventory::catalog_copies(&dir, entry)?.into_iter().next() {
                return Ok(CommandOutput::Download(DownloadOutput {
                    model: entry.id.to_string(),
                    file: path,
                    bytes: entry.size_bytes,
                    sha256: entry.sha256.to_string(),
                    already_installed: true,
                }));
            }

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
            let dir = models_dir_for(config_arg)?;
            let spec = DownloadSpec::from(entry);
            // The catalog file name, else a copy under another name.
            let mut path = dir.join(entry.file_name);
            if std::fs::symlink_metadata(&path).is_err()
                && let Some(copy) = inventory::catalog_copies(&dir, entry)?.into_iter().next()
            {
                path = copy;
            }
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

/// [`resolve_models_dir`] for the configuration file, if there is one.
fn models_dir_for(config_arg: Option<&Path>) -> Result<PathBuf, Error> {
    let loaded = load_optional_config(config_arg)?;
    resolve_models_dir(loaded.as_ref().map(|loaded| &loaded.config))
}

/// The models folder: the configured `models_dir`, else (`models_dir:
/// null`, or no configuration file) the per-user data directory (see
/// `models_dir::default_models_dir`).
fn resolve_models_dir(config: Option<&Config>) -> Result<PathBuf, Error> {
    config
        .and_then(|config| config.models_dir.clone())
        .or_else(|| models_dir::default_models_dir(&|key| std::env::var_os(key)))
        .ok_or_else(|| {
            Error::invalid_arguments(
                "no models directory: set models_dir in transcribe.yml or define XDG_DATA_HOME or HOME",
            )
        })
}

/// The models in the configured models folder, read once.
fn discover(config: &Config) -> Result<Inventory, Error> {
    inventory::discover(&resolve_models_dir(Some(config))?)
}

/// Writes inventory diagnostics to stderr, as warnings.
fn warn(diagnostics: &[Diagnostic]) {
    for diagnostic in diagnostics {
        eprintln!(
            "warning: {}: {}",
            diagnostic.file.display(),
            diagnostic.message
        );
    }
}

/// The model a command runs.
struct Selected {
    id: String,
    path: PathBuf,
}

/// Selects `requested`, else the configured `default_model`, among the
/// models in the folder. A missing requested id is `model_not_found`; a
/// missing default is a configuration error at `default_model`.
fn select_model(
    loaded: &LoadedConfig,
    inventory: &Inventory,
    requested: Option<&str>,
    config_arg: Option<&Path>,
) -> Result<Selected, Error> {
    let (id, from_default) = match (requested, loaded.config.default_model.as_deref()) {
        (Some(id), _) => (id, false),
        (None, Some(id)) => (id, true),
        (None, None) => {
            return Err(Error::invalid_arguments(
                "no model selected; use --model ID, --model-file PATH, or set default_model in transcribe.yml",
            ));
        }
    };
    if let Some(model) = inventory.get(id) {
        return Ok(Selected {
            id: model.id.clone(),
            path: model.file.clone(),
        });
    }
    let mut message = format!(
        "{}model `{id}` is not in the models folder {} (models there: {})",
        if from_default { "default_model: " } else { "" },
        inventory.dir.display(),
        inventory.id_list()
    );
    if let Some(command) = download_command(id, config_arg) {
        message.push_str(&format!("; download it with: {command}"));
    }
    if from_default {
        Err(Error::invalid_config(loaded.default_model_error(message)))
    } else {
        Err(Error::new(ErrorCode::ModelNotFound, message))
    }
}

/// The command that downloads catalog model `id`, preserving an explicit
/// `--config`, since a `models_dir` override only takes effect through the
/// same config file. `None` when `id` is not a catalog id.
fn download_command(id: &str, config_arg: Option<&Path>) -> Option<String> {
    let entry = catalog::find(id)?;
    Some(match config_arg {
        Some(config) => format!(
            "pumice-transcribe --config {} models download {}",
            shell_quote(&config.to_string_lossy()),
            entry.id
        ),
        None => format!("pumice-transcribe models download {}", entry.id),
    })
}

/// A selected model file that disappeared after the folder was read.
fn missing_model_error(model: &Selected) -> Error {
    Error::new(
        ErrorCode::ModelNotFound,
        format!(
            "model file not found: {} (model `{}`); the models folder changed after it was read",
            model.path.display(),
            model.id
        ),
    )
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

/// `serve --managed`: every outcome is reported as lifecycle records on
/// stdout (see [`crate::managed`]); human messages go to stderr.
fn run_managed<F>(args: Args, make_engine: F) -> ExitCode
where
    F: FnOnce(EngineRequest<'_>) -> Result<Box<dyn Engine>, Error>,
{
    let Command::Serve(serve_args) = &args.command else {
        unreachable!("only serve has a managed mode");
    };
    let instance_id = serve_args.instance_id.as_deref().unwrap_or_default();
    let lifecycle = match managed::validate_instance_id(instance_id) {
        Ok(id) => Lifecycle::new(id, Box::new(io::stdout())),
        Err(message) => {
            eprintln!("{message}");
            managed::write_startup_error(&mut io::stdout(), ErrorCode::InvalidArguments, message);
            return ExitCode::from(2);
        }
    };
    // Before anything that can block, so an early EOF is never missed.
    if let Err(e) = lifecycle.start_lifeline() {
        let error = Error::internal(format!("failed to watch stdin: {e}"));
        eprintln!("{error}");
        lifecycle.announce_error(&error);
        return exit_code(&error);
    }

    let result = load_config(args.config.as_deref()).and_then(|loaded| {
        let Command::Serve(serve_args) = args.command else {
            unreachable!("only serve has a managed mode");
        };
        run_serve(
            serve_args,
            &loaded,
            args.config.as_deref(),
            make_engine,
            Some(&lifecycle),
        )
    });
    match result {
        Ok(Served::Stopped) => {
            lifecycle.announce_stopped();
            ExitCode::SUCCESS
        }
        Ok(Served::Retired) => {
            eprintln!("{RETIRED}");
            lifecycle.announce_stopped();
            ExitCode::from(1)
        }
        Err(error) => {
            eprintln!("{error}");
            lifecycle.announce_error(&error);
            exit_code(&error)
        }
    }
}

/// Serves one model, selected from the models folder as read at startup.
/// A managed child is always started with `--model`, so a restart reloads
/// the same id or fails.
fn run_serve<F>(
    serve_args: ServeArgs,
    loaded: &LoadedConfig,
    config_arg: Option<&Path>,
    make_engine: F,
    lifecycle: Option<&Arc<Lifecycle>>,
) -> Result<Served, Error>
where
    F: FnOnce(EngineRequest<'_>) -> Result<Box<dyn Engine>, Error>,
{
    let config = &loaded.config;
    let threads = normalize_threads(config.threads.count())?;
    let backend = serve_args.backend.unwrap_or(config.backend);
    let inventory = discover(config)?;
    warn(&inventory.diagnostics);
    let model = select_model(loaded, &inventory, serve_args.model.as_deref(), config_arg)?;
    if lifecycle.is_some() {
        managed::validate_model_id(&model.id)?;
    }

    if !model.path.is_file() {
        return Err(missing_model_error(&model));
    }
    let Selected {
        id: served_model_id,
        path: model_path,
    } = model;

    let port = serve_args.port.or(config.server_port).ok_or_else(|| {
        Error::invalid_arguments(
            "no port specified; use --port PORT or set server.port in transcribe.yml",
        )
    })?;

    let start = Instant::now();
    let engine = make_engine(EngineRequest {
        model_path: &model_path,
        threads,
        backend,
    })?;
    let load_ms = start.elapsed().as_millis();
    let compute = engine.compute();
    match &compute {
        // The snapshot as JSON: exactly what `file --json` and managed
        // `ready` report.
        Some(compute) => eprintln!(
            "loaded model `{served_model_id}` in {load_ms} ms; compute: {}",
            compute.to_value()
        ),
        None => eprintln!("loaded model `{served_model_id}` in {load_ms} ms"),
    }

    let rt = tokio::runtime::Runtime::new().map_err(|e| Error::internal(format!("{e}")))?;
    if let Some(lifecycle) = lifecycle {
        // Every managed `ready` carries a snapshot; a native engine always
        // has one.
        let compute = compute.ok_or_else(|| {
            Error::internal("the engine reported no compute device; managed mode needs one")
        })?;
        let model_load_ms = u64::try_from(load_ms).unwrap_or(u64::MAX);
        return rt.block_on(serve_managed(
            port,
            Arc::from(engine),
            served_model_id,
            config.language.clone(),
            model_load_ms,
            compute,
            lifecycle,
        ));
    }
    let retire = retire_token(compute.as_ref());
    rt.block_on(async {
        let bind_error = |e: io::Error| Error::internal(format!("failed to bind: {e}"));
        let listener = crate::server::bind(port).await.map_err(bind_error)?;
        let shutdown = {
            let retire = retire.clone();
            async move {
                tokio::select! {
                    () = crate::server::shutdown_signal() => {}
                    () = retired(retire) => {}
                }
            }
        };
        let handle = crate::server::serve_on(
            listener,
            Arc::from(engine),
            served_model_id,
            config.language.clone(),
            None,
            retire.clone(),
            shutdown,
        )
        .map_err(bind_error)?;
        let _ = handle.await;
        Ok(served(retire.as_ref()))
    })
}

fn served(retire: Option<&CancellationToken>) -> Served {
    if retire.is_some_and(CancellationToken::is_cancelled) {
        Served::Retired
    } else {
        Served::Stopped
    }
}

/// Serve in managed mode until the lifeline, a signal or a retirement stops
/// it. `ready` is announced only after the model is loaded and the listener
/// is bound, and never once a stop was requested.
async fn serve_managed(
    port: u16,
    engine: Arc<dyn Engine>,
    served_model_id: String,
    default_language: Option<String>,
    model_load_ms: u64,
    compute: Compute,
    lifecycle: &Arc<Lifecycle>,
) -> Result<Served, Error> {
    if lifecycle.is_stopping() {
        return Ok(Served::Stopped);
    }
    // Installed before `ready`, so an owner's signal is never missed.
    let signal = crate::server::registered_shutdown_signal()
        .map_err(|e| Error::internal(format!("failed to install signal handlers: {e}")))?;
    let bind_error = |e: io::Error| Error::internal(format!("failed to bind: {e}"));
    let listener = crate::server::bind(port).await.map_err(bind_error)?;
    let bound_port = listener.local_addr().map_err(bind_error)?.port();

    let retire = retire_token(Some(&compute));
    let lifeline = Arc::clone(lifecycle);
    let retirement = retire.clone();
    let shutdown = async move {
        tokio::select! {
            () = lifeline.cancelled() => {}
            () = signal => lifeline.request_stop(StopReason::Signal),
            () = retired(retirement) => lifeline.request_stop(StopReason::EngineFailed),
        }
    };
    let handle = crate::server::serve_on(
        listener,
        engine,
        served_model_id.clone(),
        default_language,
        Some(crate::server::Managed {
            instance_id: lifecycle.instance_id().to_string(),
            compute: compute.clone(),
        }),
        retire.clone(),
        shutdown,
    )
    .map_err(bind_error)?;
    lifecycle.announce_ready(bound_port, &served_model_id, model_load_ms, &compute);
    let _ = handle.await;
    Ok(served(retire.as_ref()))
}

/// The configuration file, which the command needs.
fn load_config(config_arg: Option<&Path>) -> Result<LoadedConfig, Error> {
    config::load(config_arg).map_err(Error::invalid_config)
}

/// The configuration file, for a command that also runs without one.
fn load_optional_config(config_arg: Option<&Path>) -> Result<Option<LoadedConfig>, Error> {
    config::load_optional(config_arg).map_err(Error::invalid_config)
}

/// The configuration for `file`. A one-off `file --model-file PATH` also
/// runs without a configuration file (`None`), from its flags and their
/// defaults ([`flag_defaults`]); D3 governs the YAML settings, so a file
/// that exists must still be complete. `--model ID` and `default_model`
/// need the file.
fn file_config(
    file_args: &FileArgs,
    config_arg: Option<&Path>,
) -> Result<Option<LoadedConfig>, Error> {
    if file_args.model_file.is_none() {
        return load_config(config_arg).map(Some);
    }
    load_optional_config(config_arg)
}

/// What the `file` flags fall back to without a configuration file:
/// language detection, `auto` threads (the available parallelism, capped at
/// 32) and the cpu backend.
fn flag_defaults() -> Config {
    Config {
        default_model: None,
        default_model_at: None,
        language: None,
        threads: Threads::Auto,
        backend: BackendMode::Cpu,
        server_port: None,
        models_dir: None,
    }
}

/// `file`: `loaded` is `None` only for a one-off `--model-file` without a
/// configuration file.
fn run_file<F>(
    file_args: FileArgs,
    loaded: Option<&LoadedConfig>,
    config_arg: Option<&Path>,
    make_engine: F,
) -> Result<SuccessOutput, Error>
where
    F: FnOnce(EngineRequest<'_>) -> Result<Box<dyn Engine>, Error>,
{
    let defaults = flag_defaults();
    let config = loaded.map_or(&defaults, |loaded| &loaded.config);
    let language = normalize_language(file_args.language.or_else(|| config.language.clone()))?;
    let threads = normalize_threads(file_args.threads.or(config.threads.count()))?;
    let backend = file_args.backend.unwrap_or(config.backend);

    // A model from the folder is selected before the audio is read; a
    // one-off file is named after it, where its existence is checked.
    let selected = match (&file_args.model_file, loaded) {
        (Some(_), _) => None,
        (None, Some(loaded)) => {
            let inventory = discover(config)?;
            warn(&inventory.diagnostics);
            Some(select_model(
                loaded,
                &inventory,
                file_args.model.as_deref(),
                config_arg,
            )?)
        }
        (None, None) => unreachable!("file_config loads the file without --model-file"),
    };

    let file = open_regular_file(&file_args.audio)?;
    let metadata = file.metadata()?;
    if metadata.len() > crate::audio::MAX_FILE_BYTES {
        return Err(Error::input_too_large(metadata.len()));
    }

    let (pcm, audio_seconds) = decode_audio_from_reader(file)?;

    let Selected {
        id: model_id,
        path: model_path,
    } = match (selected, &file_args.model_file) {
        (Some(selected), _) => {
            if !selected.path.is_file() {
                return Err(missing_model_error(&selected));
            }
            selected
        }
        (None, Some(path)) => {
            let (model, diagnostics) = inventory::name_file(path)?;
            warn(&diagnostics);
            Selected {
                id: model.id,
                path: path.clone(),
            }
        }
        (None, None) => unreachable!("a model is selected or given as a file"),
    };

    let start = Instant::now();
    let engine = make_engine(EngineRequest {
        model_path: &model_path,
        threads,
        backend,
    })?;
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
        model: model_id,
        text: transcript.text,
        language: language.or(transcript.language),
        audio_seconds,
        model_load_ms,
        transcription_ms,
        compute: engine.compute(),
    })
}

/// `models list`. A missing folder and a `default_model` missing from it
/// are diagnostics here, so the listing still shows what is there.
fn build_model_list(config: &Config, inventory: Inventory) -> ModelsListOutput {
    let Inventory {
        dir,
        models,
        mut diagnostics,
    } = inventory;
    if !dir.exists() {
        diagnostics.push(Diagnostic {
            file: dir.clone(),
            message: "the models folder does not exist yet".to_owned(),
        });
    }
    if let Some(default) = &config.default_model
        && !models.iter().any(|model| &model.id == default)
    {
        diagnostics.push(Diagnostic {
            file: dir.clone(),
            message: format!("default_model `{default}` is not in the models folder"),
        });
    }
    let models = models
        .into_iter()
        .map(|model| ModelListEntry {
            default: config.default_model.as_ref() == Some(&model.id),
            id: model.id,
            file: model.file,
            size: model.size,
            catalog: model.catalog,
        })
        .collect();
    ModelsListOutput {
        models_dir: dir,
        models,
        diagnostics,
    }
}

fn build_catalog_list(models_dir: &Path) -> Vec<CatalogListEntry> {
    catalog::CATALOG
        .iter()
        .map(|entry| CatalogListEntry {
            id: entry.id,
            description: entry.description,
            file_name: entry.file_name,
            size: entry.size_bytes,
            sha256: entry.sha256,
            license: entry.license,
            installed: inventory::has_file_of_size(models_dir, entry.size_bytes),
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

fn print_backends(
    report: &serde_json::Value,
    json: bool,
    stdout: &mut dyn Write,
) -> io::Result<()> {
    if json {
        let envelope = Envelope {
            schema_version: 1,
            ok: true,
            result: Some(report.clone()),
            error: None,
        };
        return writeln!(stdout, "{}", serde_json::to_string(&envelope).unwrap());
    }
    let text = |key: &str| match &report[key] {
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Null => "-".to_string(),
        other => other.to_string(),
    };
    for key in [
        "requested",
        "gpu_build",
        "auto_uses_gpu_module",
        "bundle",
        "cpu_variant",
        "gpu_module",
        "gpu_devices",
        "fallback_reason",
    ] {
        writeln!(stdout, "{key}: {}", text(key))?;
    }
    let allowed: Vec<&str> = report["allowed_backends"]
        .as_array()
        .map(|list| list.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    writeln!(stdout, "allowed_backends: {}", allowed.join(", "))?;
    writeln!(stdout, "devices:")?;
    for device in report["devices"].as_array().into_iter().flatten() {
        writeln!(
            stdout,
            "  {} ({}, {}): {}",
            device["name"].as_str().unwrap_or_default(),
            device["kind"].as_str().unwrap_or_default(),
            device["type"].as_str().unwrap_or_default(),
            device["description"].as_str().unwrap_or_default(),
        )?;
    }
    Ok(())
}

fn print_check_config(file: &Path, json: bool, stdout: &mut dyn Write) -> io::Result<()> {
    if json {
        let envelope = Envelope {
            schema_version: 1,
            ok: true,
            result: Some(serde_json::to_value(CheckConfigResult {
                file: file.to_path_buf(),
            })?),
            error: None,
        };
        writeln!(stdout, "{}", serde_json::to_string(&envelope).unwrap())
    } else {
        writeln!(stdout, "config ok: {}", file.display())
    }
}

fn print_models_list(
    output: &ModelsListOutput,
    json: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> io::Result<()> {
    if json {
        let envelope = Envelope {
            schema_version: 1,
            ok: true,
            result: Some(serde_json::to_value(output)?),
            error: None,
        };
        return writeln!(stdout, "{}", serde_json::to_string(&envelope).unwrap());
    }
    for diagnostic in &output.diagnostics {
        writeln!(
            stderr,
            "warning: {}: {}",
            diagnostic.file.display(),
            diagnostic.message
        )?;
    }
    writeln!(stdout, "models folder: {}", output.models_dir.display())?;
    let entries = &output.models;
    if entries.is_empty() {
        writeln!(stdout, "No models in the models folder.")?;
        return Ok(());
    }
    let id_width = entries.iter().map(|e| e.id.len()).max().unwrap_or(0).max(2);
    let size_width = entries
        .iter()
        .map(|e| e.size.to_string().len())
        .max()
        .unwrap_or(0)
        .max(4);
    writeln!(
        stdout,
        "{:<id_width$}  {:>size_width$}  {:<7}  {:<7}  FILE",
        "ID", "SIZE", "CATALOG", "DEFAULT",
    )?;
    for entry in entries {
        writeln!(
            stdout,
            "{:<id_width$}  {:>size_width$}  {:<7}  {:<7}  {}",
            entry.id,
            entry.size,
            if entry.catalog { "yes" } else { "no" },
            if entry.default { "yes" } else { "no" },
            entry.file.display(),
        )?;
    }
    Ok(())
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
        | ErrorCode::GpuUnavailable
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
        Command::Backends(args) => args.json,
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
            model: "tiny".to_string(),
            text: "hello".to_string(),
            language: None,
            audio_seconds: 1.0,
            model_load_ms: 0,
            transcription_ms: 0,
            compute: None,
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
        let command = download_command(
            "whisper-tiny-q8_0",
            Some(Path::new("/tmp/config with spaces.yml")),
        )
        .unwrap();
        assert_eq!(
            command,
            "pumice-transcribe --config '/tmp/config with spaces.yml' models download whisper-tiny-q8_0"
        );
        assert_eq!(
            download_command("whisper-tiny-q8_0", None).unwrap(),
            "pumice-transcribe models download whisper-tiny-q8_0"
        );
        assert_eq!(download_command("tiny", None), None);
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

    /// Runs `args` through the real command dispatch with a fake engine
    /// that records the backend it was asked for.
    /// Runs `args` against a complete configuration whose `backend` line is
    /// `backend`.
    fn requested_backend(backend: &str, args: &[&str]) -> (Result<(), Error>, Option<BackendMode>) {
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("transcribe.yml");
        std::fs::write(
            &config_path,
            format!(
                "schema_version: 1\ndefault_model: null\nlanguage: null\nthreads: auto\n{backend}server:\n  port: null\nmodels_dir: models\n"
            ),
        )
        .unwrap();
        std::fs::create_dir(temp.path().join("models")).unwrap();
        std::fs::write(temp.path().join("models").join("tiny.gguf"), b"not a model").unwrap();
        let audio = temp.path().join("audio.wav");
        std::fs::write(&audio, wav_bytes()).unwrap();
        let mut argv: Vec<OsString> = vec![
            "pumice-transcribe".into(),
            "--config".into(),
            config_path.into(),
        ];
        for arg in args {
            argv.push(if *arg == "AUDIO" {
                audio.clone().into_os_string()
            } else {
                (*arg).into()
            });
        }
        let parsed = Args::try_parse_from(&argv).unwrap();
        let seen = std::cell::Cell::new(None);
        let result = run_command(parsed, |request: EngineRequest<'_>| {
            seen.set(Some(request.backend));
            // Stop before serving: only the request matters here.
            Err(Error::model_load_failed())
        });
        (result.map(|_| ()), seen.get())
    }

    fn wav_bytes() -> Vec<u8> {
        let mut buf = io::Cursor::new(Vec::new());
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::new(&mut buf, spec).unwrap();
        for _ in 0..1600 {
            writer.write_sample(0i16).unwrap();
        }
        writer.finalize().unwrap();
        buf.into_inner()
    }

    #[test]
    fn the_backend_flag_overrides_the_config() {
        let file = ["file", "AUDIO", "--model", "tiny"];
        let serve = ["serve", "--model", "tiny", "--port", "0"];
        for command in [&file[..], &serve[..]] {
            let with = |flag: Option<&'static str>| -> Vec<&'static str> {
                let mut args = command.to_vec();
                if let Some(flag) = flag {
                    args.extend(["--backend", flag]);
                }
                args
            };
            assert_eq!(
                requested_backend("backend: cpu\n", &with(None)).1,
                Some(BackendMode::Cpu)
            );
            assert_eq!(
                requested_backend("backend: auto\n", &with(None)).1,
                Some(BackendMode::Auto)
            );
            assert_eq!(
                requested_backend("backend: auto\n", &with(Some("gpu"))).1,
                Some(BackendMode::Gpu)
            );
            assert_eq!(
                requested_backend("backend: gpu\n", &with(Some("cpu"))).1,
                Some(BackendMode::Cpu)
            );
            let (result, _) = requested_backend("backend: cpu\n", &with(Some("auto")));
            assert_eq!(result.unwrap_err().code, ErrorCode::ModelLoadFailed);
        }
    }

    #[test]
    fn model_ids_starting_with_hyphens_are_values_for_file_and_serve() {
        for id in ["-voice", "--json", "--help"] {
            for command in ["file", "serve"] {
                let mut argv = vec!["pumice-transcribe", command];
                if command == "file" {
                    argv.push("audio.wav");
                }
                argv.extend(["--model", id]);
                let args = Args::try_parse_from(argv).unwrap();
                match args.command {
                    Command::File(file) => {
                        assert_eq!(file.model.as_deref(), Some(id));
                        assert!(!file.json);
                    }
                    Command::Serve(serve) => assert_eq!(serve.model.as_deref(), Some(id)),
                    _ => unreachable!(),
                }
            }
        }
    }

    #[test]
    fn backend_flags_accept_only_the_three_modes() {
        for command in [
            &["file", "a.wav", "--backend"][..],
            &["serve", "--backend"],
            &["backends", "--backend"],
        ] {
            for good in ["cpu", "auto", "gpu"] {
                let mut argv = vec!["pumice-transcribe"];
                argv.extend(command);
                argv.push(good);
                assert!(Args::try_parse_from(&argv).is_ok(), "{argv:?}");
            }
            for bad in ["vulkan", "GPU", ""] {
                let mut argv = vec!["pumice-transcribe"];
                argv.extend(command);
                argv.push(bad);
                assert!(Args::try_parse_from(&argv).is_err(), "{argv:?}");
            }
        }
        // The P14.1 diagnostic is gone.
        assert!(Args::try_parse_from(["pumice-transcribe", "debug-backends"]).is_err());
    }

    #[test]
    fn retirement_needs_a_gpu_snapshot() {
        use crate::compute::ComputeBackend;
        let snapshot = |backend| Compute::new(BackendMode::Auto, backend, "D", "", None, None);
        assert!(retire_token(None).is_none());
        assert!(retire_token(Some(&snapshot(ComputeBackend::Cpu))).is_none());
        let token = retire_token(Some(&snapshot(ComputeBackend::Vulkan))).unwrap();
        assert_eq!(served(Some(&token)), Served::Stopped);
        token.cancel();
        assert_eq!(served(Some(&token)), Served::Retired);
        assert_eq!(served(None), Served::Stopped);
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

    fn model(id: &str, catalog: bool) -> inventory::Model {
        inventory::Model {
            id: id.to_string(),
            file: PathBuf::from(format!("/models/{id}.gguf")),
            size: 7,
            catalog,
        }
    }

    #[test]
    fn build_model_list_marks_the_default_and_reports_a_missing_one() {
        let mut config = flag_defaults();
        config.default_model = Some("whisper-tiny-q8_0".to_string());
        let folder = tempfile::tempdir().unwrap();
        let inventory = Inventory {
            dir: folder.path().to_path_buf(),
            models: vec![model("Custom", false), model("whisper-tiny-q8_0", true)],
            diagnostics: Vec::new(),
        };
        let output = build_model_list(&config, inventory.clone());
        assert_eq!(output.models_dir, folder.path());
        assert!(output.diagnostics.is_empty());
        let flags: Vec<(&str, bool, bool)> = output
            .models
            .iter()
            .map(|m| (m.id.as_str(), m.catalog, m.default))
            .collect();
        assert_eq!(
            flags,
            [("Custom", false, false), ("whisper-tiny-q8_0", true, true)]
        );
        let value = serde_json::to_value(&output).unwrap();
        assert_eq!(
            value["models"][1],
            serde_json::json!({"id": "whisper-tiny-q8_0", "file": "/models/whisper-tiny-q8_0.gguf",
                "size": 7, "catalog": true, "default": true})
        );

        config.default_model = Some("gone".to_string());
        let output = build_model_list(&config, inventory);
        assert!(output.models.iter().all(|m| !m.default));
        assert_eq!(output.diagnostics.len(), 1);
        assert!(
            output.diagnostics[0]
                .message
                .contains("default_model `gone`")
        );

        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        print_models_list(&output, false, &mut stdout, &mut stderr).unwrap();
        let stdout = String::from_utf8(stdout).unwrap();
        assert!(
            stdout.starts_with(&format!("models folder: {}\n", folder.path().display())),
            "{stdout}"
        );
        assert!(stdout.contains("whisper-tiny-q8_0"), "{stdout}");
        assert!(String::from_utf8(stderr).unwrap().contains("warning:"));
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
