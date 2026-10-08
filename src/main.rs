//! Pumice command-line entry point.
//!
//! A bare `pumice` (or the explicit `serve` alias) runs the local
//! OpenAI-compatible dictation-formatting service on IPv4 loopback;
//! `check-config` validates and summarizes the configuration; `doctor`
//! reports which provider CLIs are installed and can run one real formatting
//! call to check a provider's login.

use std::io;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use pumice::api;
use pumice::config::{self, ConfigSource, LoadedConfig};
use pumice::doctor;
use pumice::logging::DebugLog;
use pumice::pipeline::Pipeline;
use pumice::process::ProcessRunner;
use pumice::providers;

const USAGE: &str = "\
pumice - local dictation formatting service

usage:
  pumice [--config <path>]                 run the local service on 127.0.0.1 (default)
  pumice serve [--config <path>]           alias for the command above
  pumice check-config [--config <path>]    validate and summarize the configuration
  pumice doctor [--config <path>]          show which provider CLIs are installed
  pumice doctor --login-check --provider <id>
                                           run one real formatting call to check login
  pumice --help                            print this help
  pumice --version                         print the version
";

const DOCTOR_USAGE: &str = "usage: pumice doctor [--config <path>] [--login-check --provider <id>]";

fn main() -> ExitCode {
    // `std::env::args` panics on an argument that is not valid Unicode.
    let Some(args) = std::env::args_os()
        .skip(1)
        .map(|arg| arg.into_string().ok())
        .collect::<Option<Vec<String>>>()
    else {
        eprintln!("error: arguments must be valid Unicode\n");
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["-h" | "--help"] => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        ["-V" | "--version"] => {
            println!("pumice {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        ["check-config", rest @ ..] => check_config(rest),
        ["doctor", rest @ ..] => doctor(rest),
        ["serve", rest @ ..] => serve(rest, "pumice serve"),
        // Bare `pumice` starts the service; a leading `--config <path>` is
        // the explicit-configuration form of the same command.
        [] | ["--config", ..] => serve(&args, "pumice"),
        [first, ..] => {
            eprintln!("error: unexpected argument '{first}'\n");
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// Parses a trailing `[--config <path>]`; shared by `check-config` and the
/// service command (`serve` is the explicit alias of the bare form).
fn config_flag<'a>(args: &'a [&'a str], command: &str) -> Result<Option<&'a Path>, ExitCode> {
    match args {
        [] => Ok(None),
        ["--config", path] => Ok(Some(Path::new(path))),
        _ => {
            eprintln!("error: usage: {command} [--config <path>]\n");
            Err(ExitCode::from(2))
        }
    }
}

/// Config loading is shared between commands: same errors, same exit code 2.
fn load_config(explicit: Option<&Path>) -> Result<LoadedConfig, ExitCode> {
    config::load(explicit).map_err(|error| {
        eprintln!("{error}");
        ExitCode::from(2)
    })
}

fn build_runtime() -> Result<tokio::runtime::Runtime, ExitCode> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            eprintln!("error: cannot start the async runtime: {error}");
            ExitCode::from(1)
        })
}

fn check_config(args: &[&str]) -> ExitCode {
    let explicit = match config_flag(args, "pumice check-config") {
        Ok(explicit) => explicit,
        Err(code) => return code,
    };
    match load_config(explicit) {
        Ok(loaded) => {
            print_summary(&loaded);
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}

fn serve(args: &[&str], command: &str) -> ExitCode {
    let explicit = match config_flag(args, command) {
        Ok(explicit) => explicit,
        Err(code) => return code,
    };
    let loaded = match load_config(explicit) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    let runtime = match build_runtime() {
        Ok(runtime) => runtime,
        Err(code) => return code,
    };
    runtime.block_on(run_service(loaded))
}

/// `pumice doctor [--config <path>] [--login-check --provider <id>]`.
fn doctor(args: &[&str]) -> ExitCode {
    let mut explicit: Option<&Path> = None;
    let mut login_check = false;
    let mut provider: Option<&str> = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        // A flag given twice is a usage error, never "last one wins".
        let repeated = match *arg {
            "--config" => explicit.is_some(),
            "--login-check" => login_check,
            "--provider" => provider.is_some(),
            _ => false,
        };
        if repeated {
            eprintln!("error: {arg} given more than once\n{DOCTOR_USAGE}");
            return ExitCode::from(2);
        }
        match *arg {
            "--config" => match rest.next() {
                Some(path) => explicit = Some(Path::new(path)),
                None => {
                    eprintln!("error: --config requires a path\n{DOCTOR_USAGE}");
                    return ExitCode::from(2);
                }
            },
            "--login-check" => login_check = true,
            "--provider" => match rest.next() {
                Some(id) => provider = Some(id),
                None => {
                    eprintln!("error: --provider requires an id\n{DOCTOR_USAGE}");
                    return ExitCode::from(2);
                }
            },
            other => {
                eprintln!("error: unexpected argument '{other}'\n{DOCTOR_USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    if provider.is_some() && !login_check {
        eprintln!("error: --provider requires --login-check\n{DOCTOR_USAGE}");
        return ExitCode::from(2);
    }
    if login_check && provider.is_none() {
        eprintln!("error: --login-check requires --provider <id>\n{DOCTOR_USAGE}");
        return ExitCode::from(2);
    }
    match provider {
        Some(id) => doctor_login_check(explicit, id),
        None => doctor_report(explicit),
    }
}

/// The default, quota-free report: fresh detection over the whole registry.
fn doctor_report(explicit: Option<&Path>) -> ExitCode {
    let loaded = match load_config(explicit) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    let runtime = match build_runtime() {
        Ok(runtime) => runtime,
        Err(code) => return code,
    };
    runtime.block_on(async {
        let runner = ProcessRunner::new();
        let detection =
            providers::discovery::detect(&loaded.config, providers::PROVIDERS, &runner).await;
        print!("{}", doctor::render(&loaded.source, &detection));
        let (ready, total) = doctor::enabled_ready(&detection);
        if ready == total {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        }
    })
}

/// The quota-bearing path: one real formatting call through the selected
/// provider's normal adapter.
fn doctor_login_check(explicit: Option<&Path>, id: &str) -> ExitCode {
    let loaded = match load_config(explicit) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    let runtime = match build_runtime() {
        Ok(runtime) => runtime,
        Err(code) => return code,
    };
    runtime.block_on(async {
        let runner = ProcessRunner::new();
        let provider = match doctor::prepare_login_check(&loaded.config, id, &runner).await {
            Ok(provider) => provider,
            Err(error) => {
                let (line, code) = error.line_and_code(id);
                eprintln!("{line}");
                return ExitCode::from(code);
            }
        };
        eprintln!("{}", doctor::quota_notice(id));
        match doctor::run_login_call(&loaded.config, &provider).await {
            doctor::LoginCheck::Ok(elapsed) => {
                println!("login check: ok ({} ms)", elapsed.as_millis());
                ExitCode::SUCCESS
            }
            doctor::LoginCheck::Failed(error) => {
                eprintln!("login check: {}", doctor::error_category(error));
                ExitCode::from(1)
            }
        }
    })
}

async fn run_service(loaded: LoadedConfig) -> ExitCode {
    let config = loaded.config.clone();
    let runner = Arc::new(ProcessRunner::new());
    let built = match providers::build_from_config(&config, Arc::clone(&runner)) {
        Ok(built) => built,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    // Startup detection (S2.8 part 1): probe every registered provider once —
    // enabled or not, so a later `doctor` can show the whole registry — and
    // cache the result in the pipeline. Status lines name enabled providers
    // only and never carry probe output.
    let detection = providers::discovery::detect(&config, providers::PROVIDERS, &runner).await;
    for (descriptor, status) in providers::PROVIDERS.iter().zip(&detection) {
        debug_assert_eq!(descriptor.id, status.id);
        if status.enabled {
            eprintln!(
                "{}",
                providers::discovery::status_line(status, descriptor.install_hint)
            );
        }
    }
    let pipeline = Arc::new(Pipeline::with_detection(&config, built, detection));

    // The debug log holds dictated text, so it opens before serving and a
    // misconfigured path fails startup (exit 1) instead of silently losing
    // records.
    let debug_log = match DebugLog::open(&config.debug_log) {
        Ok(debug_log) => debug_log,
        Err(error) => {
            eprintln!("error: cannot open debug_log.path: {error}");
            return ExitCode::from(1);
        }
    };
    if debug_log.is_enabled() {
        eprintln!(
            "warning: debug log enabled: dictated text is written to {}",
            config.debug_log.path.display()
        );
    }

    // Bind before serving so an occupied port fails at startup with an
    // actionable message instead of after the event loop starts.
    let listener = match api::bind(config.port).await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
            eprintln!(
                "error: port {} on 127.0.0.1 is already in use. Change \"port\" in {}.",
                config.port,
                config_location(&loaded.source)
            );
            return ExitCode::from(1);
        }
        Err(error) => {
            eprintln!("error: cannot listen on 127.0.0.1:{}: {error}", config.port);
            return ExitCode::from(1);
        }
    };

    // The route goes to stderr with the other status lines, before the
    // listening line: a client may read that first stdout line and close the
    // pipe, and a later `println!` would then panic and stop the service.
    eprintln!(
        "{}",
        route_line(
            &pipeline,
            config.total_timeout,
            &config_location(&loaded.source)
        )
    );
    println!("pumice listening on http://127.0.0.1:{}/v1", config.port);
    match api::serve(
        listener,
        pipeline,
        Arc::new(api::StderrLog),
        Arc::new(debug_log),
    )
    .await
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: the service stopped unexpectedly: {error}");
            ExitCode::from(1)
        }
    }
}

/// The startup route line: every enabled provider in list order, and what a
/// failure or a model-less request means — the original text comes back.
/// When nothing is enabled, one clear line says what that means.
fn route_line(
    pipeline: &Pipeline,
    total_timeout: std::time::Duration,
    config_path: &str,
) -> String {
    let timeout = format!("total timeout {}s", total_timeout.as_secs());
    let route = pipeline.enabled_route();
    if route.is_empty() {
        return format!(
            "no providers enabled: every request returns the original text (add providers to {config_path}); {timeout}"
        );
    }
    let enabled = route
        .iter()
        .map(|(id, model)| {
            if model.is_empty() {
                id.to_string()
            } else {
                format!("{id} ({model})")
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("providers: {enabled}; on failure or no model: original text; {timeout}")
}

fn config_location(source: &ConfigSource) -> String {
    match source {
        ConfigSource::File(path) => path.display().to_string(),
        ConfigSource::BuiltInDefaults => "the configuration file".to_owned(),
    }
}

/// Prints what is safe to show: never prompts, environment values or option
/// values, which may be private.
fn print_summary(loaded: &LoadedConfig) {
    match &loaded.source {
        ConfigSource::File(path) => println!("config: {}", path.display()),
        ConfigSource::BuiltInDefaults => println!("config: built-in defaults"),
    }
    let config = &loaded.config;
    println!("port: {}", config.port);
    println!("total timeout: {}s", config.total_timeout.as_secs());
    if config.providers.is_empty() {
        println!("providers: (none configured)");
    } else {
        println!("providers (in order):");
        for entry in &config.providers {
            let settings = &entry.settings;
            let state = if settings.enabled {
                "enabled"
            } else {
                "disabled"
            };
            let mut line = format!("  {}: {state}", entry.id);
            if !settings.model.is_empty() {
                line.push_str(&format!(", model {}", settings.model));
            }
            if settings.enabled {
                line.push_str(&format!(", timeout {}s", settings.timeout.as_secs()));
                if let Some(binary) = &settings.binary {
                    line.push_str(&format!(", binary {}", binary.display()));
                }
            }
            println!("{line}");
        }
    }
}
