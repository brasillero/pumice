//! Pumice command-line entry point.
//!
//! `serve` runs the local OpenAI-compatible dictation-formatting service on
//! IPv4 loopback; `check-config` validates and summarizes the configuration.

use std::io;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use pumice::api;
use pumice::config::{self, ConfigSource, LoadedConfig};
use pumice::pipeline::Pipeline;
use pumice::process::ProcessRunner;
use pumice::providers;

const USAGE: &str = "\
pumice - local dictation formatting service

usage:
  pumice --help                          print this help
  pumice --version                       print the version
  pumice check-config [--config <path>]  validate and summarize the configuration
  pumice serve [--config <path>]         run the local service on 127.0.0.1
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
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
        ["serve", rest @ ..] => serve(rest),
        [] => {
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
        [first, ..] => {
            eprintln!("error: unexpected argument '{first}'\n");
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// `--config` handling is shared with `check-config`: same errors, same
/// exit code 2.
fn load_config(args: &[&str], command: &str) -> Result<LoadedConfig, ExitCode> {
    let explicit = match args {
        [] => None,
        ["--config", path] => Some(Path::new(path)),
        _ => {
            eprintln!("error: usage: pumice {command} [--config <path>]\n");
            return Err(ExitCode::from(2));
        }
    };
    config::load(explicit).map_err(|error| {
        eprintln!("{error}");
        ExitCode::from(2)
    })
}

fn check_config(args: &[&str]) -> ExitCode {
    match load_config(args, "check-config") {
        Ok(loaded) => {
            print_summary(&loaded);
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}

fn serve(args: &[&str]) -> ExitCode {
    let loaded = match load_config(args, "serve") {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("error: cannot start the async runtime: {error}");
            return ExitCode::from(1);
        }
    };
    runtime.block_on(run_service(loaded))
}

async fn run_service(loaded: LoadedConfig) -> ExitCode {
    let config = loaded.config.clone();
    let runner = Arc::new(ProcessRunner::new());
    let providers = match providers::build_from_config(&config, runner) {
        Ok(providers) => providers,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let pipeline = Arc::new(Pipeline::new(&config, providers));

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

    println!("pumice listening on http://127.0.0.1:{}/v1", config.port);
    match api::serve(listener, pipeline, Arc::new(api::StderrLog)).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: the service stopped unexpectedly: {error}");
            ExitCode::from(1)
        }
    }
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
    println!("default provider: {}", config.default_provider);
    println!("total timeout: {}s", config.total_timeout.as_secs());
    let enabled: Vec<&str> = config
        .providers
        .iter()
        .filter(|(_, settings)| settings.enabled)
        .map(|(id, _)| id.as_str())
        .collect();
    println!(
        "enabled providers: {}",
        if enabled.is_empty() {
            "(none)".to_owned()
        } else {
            enabled.join(", ")
        }
    );
    println!(
        "fallback order: {}",
        if config.fallback_order.is_empty() {
            "(none)".to_owned()
        } else {
            config.fallback_order.join(", ")
        }
    );
    for (id, settings) in &config.providers {
        if !settings.enabled {
            continue;
        }
        match &settings.binary {
            Some(binary) => println!(
                "  {id}: model {}, timeout {}s, binary {}",
                settings.model,
                settings.timeout.as_secs(),
                binary.display()
            ),
            None => println!(
                "  {id}: model {}, timeout {}s",
                settings.model,
                settings.timeout.as_secs()
            ),
        }
    }
}
