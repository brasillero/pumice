//! Pumice command-line entry point.
//!
//! Phase 1 is in progress: `check-config` validates and summarizes the
//! configuration. The `serve` command (the local OpenAI-compatible service)
//! is not implemented yet.

use std::path::Path;
use std::process::ExitCode;

use pumice::config::{self, ConfigSource, LoadedConfig};

const USAGE: &str = "\
pumice - local dictation formatting service

usage:
  pumice --help                          print this help
  pumice --version                       print the version
  pumice check-config [--config <path>]  validate and summarize the configuration
  pumice serve [--config <path>]         run the local service (not implemented yet)
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
        ["serve", ..] => {
            eprintln!("error: `serve` is not implemented yet\n");
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
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

fn check_config(args: &[&str]) -> ExitCode {
    let explicit = match args {
        [] => None,
        ["--config", path] => Some(Path::new(path)),
        _ => {
            eprintln!("error: usage: pumice check-config [--config <path>]\n");
            return ExitCode::from(2);
        }
    };
    match config::load(explicit) {
        Ok(loaded) => {
            print_summary(&loaded);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
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
