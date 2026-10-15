//! Pumice relay command-line entry point.
//!
//! A bare invocation (or the explicit `serve` alias) runs the local
//! OpenAI-compatible proxy on IPv4 loopback; `check-config` validates and
//! summarizes the configuration; `doctor` reports which provider CLIs are
//! installed and can run one real call to check a provider's login.

use std::ffi::OsString;
use std::io::{self, IsTerminal};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use crate::api;
use crate::config::{self, ConfigSource, LoadedConfig};
use crate::doctor;
use crate::logging::DebugLog;
use crate::pipeline::Pipeline;
use crate::process::ProcessRunner;
use crate::providers;

fn usage(name: &str) -> String {
    format!(
        "\
{name} - connects dictation apps to the AI assistants you already use

usage:
  {name} [--config <path>] [--plain]       run the local service on 127.0.0.1 (default)
  {name} serve [--config <path>] [--plain] alias for the command above
  {name} check-config [--config <path>]    validate and summarize the configuration
  {name} doctor [--config <path>]          show which provider CLIs are installed
  {name} doctor --login-check --provider <id>
                                           run one real formatting call to check login
  {name} --help                            print this help
  {name} --version                         print the version

  --config <path> may also precede the command: {name} --config <path> check-config
  --plain prints one log line per request instead of the live view
"
    )
}

fn doctor_usage(name: &str) -> String {
    format!("usage: {name} doctor [--config <path>] [--login-check --provider <id>]")
}

pub fn run(name: &str) -> ExitCode {
    run_with(name, std::env::args_os().skip(1))
}

/// Runs the CLI as `name` with an explicit argument list (excluding the
/// program name). The `pumice` dispatcher uses this for its `relay`
/// subcommand so there is exactly one argument parser, whether the relay
/// runs standalone or in-process.
pub fn run_with(name: &str, args: impl IntoIterator<Item = OsString>) -> ExitCode {
    // `std::env::args` panics on an argument that is not valid Unicode.
    let Some(args) = args
        .into_iter()
        .map(|arg| arg.into_string().ok())
        .collect::<Option<Vec<String>>>()
    else {
        eprintln!("error: arguments must be valid Unicode\n");
        eprint!("{}", usage(name));
        return ExitCode::from(2);
    };
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["-h" | "--help"] => {
            print!("{}", usage(name));
            ExitCode::SUCCESS
        }
        ["-V" | "--version"] => {
            println!("{name} {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            // `--config <path>` may also precede the command
            // (`pumice relay --config FILE check-config`); it applies to
            // whichever command follows and may still appear only once.
            let (leading, rest) = peel_leading_config(&args);
            match rest {
                ["check-config", rest @ ..] => check_config(leading, rest, name),
                ["doctor", rest @ ..] => doctor(leading, rest, name),
                ["serve", rest @ ..] => serve(leading, rest, name, &format!("{name} serve")),
                // Bare invocation starts the service. `--config`, `--plain`
                // or both are the only accepted bare flags.
                [] | ["--config", ..] | ["--plain", ..] => serve(leading, rest, name, name),
                [first, ..] => {
                    eprintln!("error: unexpected argument '{first}'\n");
                    eprint!("{}", usage(name));
                    ExitCode::from(2)
                }
            }
        }
    }
}

/// Splits off a `--config <path>` placed before the subcommand. Anything
/// that is not exactly that prefix is returned untouched.
fn peel_leading_config<'a>(args: &'a [&'a str]) -> (Option<&'a Path>, &'a [&'a str]) {
    match args {
        ["--config", path, rest @ ..] => (Some(Path::new(path)), rest),
        _ => (None, args),
    }
}

/// Merges a leading `--config` with the command's own flag. Two configs are
/// a usage error, never "last one wins".
fn merge_config<'a>(
    leading: Option<&'a Path>,
    own: Option<&'a Path>,
    command: &str,
) -> Result<Option<&'a Path>, ExitCode> {
    match (leading, own) {
        (Some(_), Some(_)) => {
            eprintln!("error: --config given more than once\nusage: {command} [--config <path>]");
            Err(ExitCode::from(2))
        }
        (Some(path), None) | (None, Some(path)) => Ok(Some(path)),
        (None, None) => Ok(None),
    }
}

/// Parses a trailing `[--config <path>]` for `check-config`; the service
/// command takes [`serve_flags`].
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

/// Flags accepted by the service command and the bare form.
struct ServeFlags<'a> {
    config: Option<&'a Path>,
    plain: bool,
}

/// Parses `[--config <path>] [--plain]` in any order, each at most once.
fn serve_flags<'a>(args: &'a [&'a str], command: &str) -> Result<ServeFlags<'a>, ExitCode> {
    let usage = format!("usage: {command} [--config <path>] [--plain]");
    let mut config: Option<&'a Path> = None;
    let mut plain = false;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let repeated = match *arg {
            "--config" => config.is_some(),
            "--plain" => plain,
            _ => false,
        };
        if repeated {
            eprintln!("error: {arg} given more than once\n{usage}");
            return Err(ExitCode::from(2));
        }
        match *arg {
            "--config" => match rest.next() {
                Some(path) => config = Some(Path::new(path)),
                None => {
                    eprintln!("error: --config requires a path\n{usage}");
                    return Err(ExitCode::from(2));
                }
            },
            "--plain" => plain = true,
            other => {
                eprintln!("error: unexpected argument '{other}'\n{usage}");
                return Err(ExitCode::from(2));
            }
        }
    }
    Ok(ServeFlags { config, plain })
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

fn check_config(leading: Option<&Path>, args: &[&str], name: &str) -> ExitCode {
    let command = format!("{name} check-config");
    let own = match config_flag(args, &command) {
        Ok(own) => own,
        Err(code) => return code,
    };
    let explicit = match merge_config(leading, own, &command) {
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

fn serve(leading: Option<&Path>, args: &[&str], name: &str, command: &str) -> ExitCode {
    let flags = match serve_flags(args, command) {
        Ok(flags) => flags,
        Err(code) => return code,
    };
    let config = match merge_config(leading, flags.config, command) {
        Ok(config) => config,
        Err(code) => return code,
    };
    let loaded = match load_config(config) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    let runtime = match build_runtime() {
        Ok(runtime) => runtime,
        Err(code) => return code,
    };
    runtime.block_on(run_service(loaded, name, flags.plain))
}

/// `doctor [--config <path>] [--login-check --provider <id>]`.
fn doctor(leading: Option<&Path>, args: &[&str], name: &str) -> ExitCode {
    let mut explicit: Option<&Path> = None;
    let mut login_check = false;
    let mut provider: Option<&str> = None;
    let mut rest = args.iter();
    let usage = doctor_usage(name);
    while let Some(arg) = rest.next() {
        // A flag given twice is a usage error, never "last one wins".
        let repeated = match *arg {
            "--config" => explicit.is_some(),
            "--login-check" => login_check,
            "--provider" => provider.is_some(),
            _ => false,
        };
        if repeated {
            eprintln!("error: {arg} given more than once\n{usage}");
            return ExitCode::from(2);
        }
        match *arg {
            "--config" => match rest.next() {
                Some(path) => explicit = Some(Path::new(path)),
                None => {
                    eprintln!("error: --config requires a path\n{usage}");
                    return ExitCode::from(2);
                }
            },
            "--login-check" => login_check = true,
            "--provider" => match rest.next() {
                Some(id) => provider = Some(id),
                None => {
                    eprintln!("error: --provider requires an id\n{usage}");
                    return ExitCode::from(2);
                }
            },
            other => {
                eprintln!("error: unexpected argument '{other}'\n{usage}");
                return ExitCode::from(2);
            }
        }
    }
    if provider.is_some() && !login_check {
        eprintln!("error: --provider requires --login-check\n{usage}");
        return ExitCode::from(2);
    }
    if login_check && provider.is_none() {
        eprintln!("error: --login-check requires --provider <id>\n{usage}");
        return ExitCode::from(2);
    }
    let explicit = match merge_config(leading, explicit, &usage) {
        Ok(explicit) => explicit,
        Err(code) => return code,
    };
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

/// The quota-bearing path: one real call through the selected
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

async fn run_service(loaded: LoadedConfig, name: &str, plain: bool) -> ExitCode {
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
            config.max_parallel,
            &config_location(&loaded.source)
        )
    );
    println!("{name} listening on http://127.0.0.1:{}/v1", config.port);

    if plain
        || !std::io::stdin().is_terminal()
        || !std::io::stdout().is_terminal()
        || !std::io::stderr().is_terminal()
    {
        return match api::serve(
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
        };
    }

    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let view_url = format!("http://127.0.0.1:{}/v1", config.port);
    let view_info = crate::tui::Info {
        version: env!("CARGO_PKG_VERSION"),
        url: view_url,
        max_parallel: config.max_parallel,
        text_allowed: debug_log.is_enabled(),
    };
    let view_thread = std::thread::spawn(move || crate::tui::run(event_rx, view_info, stop_tx));

    let result = api::serve_monitored(
        listener,
        pipeline,
        Arc::new(api::NullLog),
        Arc::new(debug_log),
        Arc::new(crate::monitor::ChannelMonitor::new(event_tx)),
        async {
            let _ = stop_rx.await;
        },
    )
    .await;

    // A view that fails or panics drops `stop`, which also stops the
    // service; that unexpected stop is an error, not a clean exit.
    let view_failed = match view_thread.join() {
        Ok(Ok(())) => false,
        Ok(Err(error)) => {
            eprintln!("error: live view failed: {error}");
            true
        }
        Err(_) => {
            eprintln!("error: live view failed: the view thread panicked");
            true
        }
    };

    match result {
        Ok(()) if !view_failed => ExitCode::SUCCESS,
        Ok(()) => ExitCode::from(1),
        Err(error) => {
            eprintln!("error: the service stopped unexpectedly: {error}");
            ExitCode::from(1)
        }
    }
}

/// The startup route line: every enabled provider in list order, and what a
/// failure or a model-less request means — an HTTP error; the app keeps
/// its own text.
/// When nothing is enabled, one clear line says what that means.
fn route_line(
    pipeline: &Pipeline,
    total_timeout: std::time::Duration,
    max_parallel: usize,
    config_path: &str,
) -> String {
    let timeout = format!("total timeout {}s", total_timeout.as_secs());
    let parallel = format!("up to {max_parallel} requests at once");
    let route = pipeline.enabled_route();
    if route.is_empty() {
        return format!(
            "no providers enabled: every request gets an error and the app keeps its own text (add providers to {config_path}); {timeout}, {parallel}"
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
    format!(
        "providers: {enabled}; on failure or no model: an HTTP error, the app keeps its own text; {timeout}, {parallel}"
    )
}

fn config_location(source: &ConfigSource) -> String {
    match source {
        ConfigSource::File(path) => path.display().to_string(),
        ConfigSource::BuiltInDefaults => "the configuration file".to_owned(),
    }
}

/// Prints where the configuration came from and a summary of its settings.
fn print_summary(loaded: &LoadedConfig) {
    match &loaded.source {
        ConfigSource::File(path) => println!("config: {}", path.display()),
        ConfigSource::BuiltInDefaults => println!("config: built-in defaults"),
    }
    let config = &loaded.config;
    println!("port: {}", config.port);
    println!("total timeout: {}s", config.total_timeout.as_secs());
    println!("max parallel: {}", config.max_parallel);
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
