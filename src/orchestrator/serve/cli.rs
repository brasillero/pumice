//! The `pumice serve` command line (p13.1-flow-seam/D3, D7).

use std::ffi::OsString;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use crate::orchestrator::config;
use crate::orchestrator::serve::output::{Outputs, PanicHook};
use crate::orchestrator::serve::{self, ServeParams};

const USAGE: &str = "\
usage: pumice serve [--config <FILE>] [--port <PORT>] [--json]

Runs the local dictation stack on 127.0.0.1: one loopback frontend and one
warm transcription child (the pumice-transcribe program installed next to
pumice) loaded with serve.transcription_model. Needs a `serve:` section
with `enabled: true` in the orchestrator configuration.

Routes: GET /health, GET /ready, GET /pumice/v1/status, GET /pumice/v1/flows,
POST /pumice/v1/dictations; OpenAI GET /v1/models, GET /v1/models/{id} and
POST /v1/audio/transcriptions for the flows in serve.transcription_flows.

  --config <FILE>  the orchestrator configuration file
                   (default: the per-user orchestrator.yml)
  --port <PORT>    the frontend port, overriding serve.port (default 17570;
                   7567 is refused; 0 picks a free port and announces it)
  --json           lifecycle records as JSON lines on stdout, metadata-only
                   events on stderr
  --help           print this help

Exit codes: 2 invalid arguments or configuration; 1 runtime failure;
130 Ctrl-C; 143 SIGTERM.
";

/// The async runtime's own shutdown after the service returned.
const RUNTIME_SHUTDOWN: Duration = Duration::from_millis(250);

pub fn run(args: &[OsString]) -> ExitCode {
    // Decided before parsing so even argument errors use the JSON records.
    let json = args.iter().any(|arg| arg.as_encoded_bytes() == b"--json");
    let params = match parse(args) {
        Parsed::Help => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Parsed::Error(message) => {
            let outputs = Outputs::start(json);
            outputs
                .reporter()
                .error("invalid_arguments", None, &message);
            outputs.finish();
            if !json {
                eprint!("\n{USAGE}");
            }
            return ExitCode::from(2);
        }
        Parsed::Serve(params) => params,
    };
    let outputs = Outputs::start(json);
    let reporter = outputs.reporter();
    // Before anything can panic: metadata only, never the payload (D9).
    let panic_hook = PanicHook::install(reporter.clone());
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            reporter.error("internal", None, "cannot start the async runtime");
            drop(panic_hook);
            drop(reporter);
            outputs.finish();
            return ExitCode::from(1);
        }
    };
    // Task panics are handled inside; this catches one in the service's
    // own future. Its tasks are then dropped by the runtime's shutdown,
    // which kills the child's tree.
    let exit = std::panic::catch_unwind(AssertUnwindSafe(|| {
        runtime.block_on(serve::execute(&params, &reporter))
    }))
    .unwrap_or_else(|_| {
        reporter.error("internal", None, "pumice serve failed internally");
        1
    });
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN);
    drop(panic_hook);
    drop(reporter);
    outputs.finish();
    ExitCode::from(exit)
}

enum Parsed {
    Help,
    Error(String),
    Serve(ServeParams),
}

/// Error messages name flags, never values.
fn parse(args: &[OsString]) -> Parsed {
    if args
        .iter()
        .any(|arg| matches!(arg.as_encoded_bytes(), b"--help" | b"-h"))
    {
        return Parsed::Help;
    }
    let mut params = ServeParams::default();
    let mut json_seen = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_encoded_bytes() {
            b"--json" => {
                if json_seen {
                    return Parsed::Error("--json given more than once".into());
                }
                json_seen = true;
            }
            b"--config" => {
                if params.config.is_some() {
                    return Parsed::Error("--config given more than once".into());
                }
                let Some(value) = iter.next() else {
                    return Parsed::Error("--config requires a path".into());
                };
                params.config = Some(PathBuf::from(value));
            }
            b"--port" => {
                if params.port.is_some() {
                    return Parsed::Error("--port given more than once".into());
                }
                let Some(value) = iter.next() else {
                    return Parsed::Error("--port requires a port number".into());
                };
                let Some(port) = value.to_str().and_then(|text| text.parse::<u16>().ok()) else {
                    return Parsed::Error("--port must be a number from 0 to 65535".into());
                };
                match config::check_serve_port(port) {
                    Ok(port) => params.port = Some(port),
                    Err(message) => return Parsed::Error(format!("--port: {message}")),
                }
            }
            _ => return Parsed::Error("unknown argument; see `pumice serve --help`".into()),
        }
    }
    Parsed::Serve(params)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_strs(args: &[&str]) -> Parsed {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        parse(&args)
    }

    fn error(args: &[&str]) -> String {
        match parse_strs(args) {
            Parsed::Error(message) => message,
            _ => panic!("expected an error for {args:?}"),
        }
    }

    #[test]
    fn parses_config_port_and_json() {
        let Parsed::Serve(params) = parse_strs(&["--config", "o.yml", "--port", "0", "--json"])
        else {
            panic!("expected params");
        };
        assert_eq!(
            params,
            ServeParams {
                config: Some(PathBuf::from("o.yml")),
                port: Some(0)
            }
        );
        let Parsed::Serve(params) = parse_strs(&[]) else {
            panic!("expected params");
        };
        assert_eq!(params, ServeParams::default());
        assert!(matches!(
            parse_strs(&["--port", "x", "--help"]),
            Parsed::Help
        ));
    }

    #[test]
    fn bad_arguments_are_errors_without_echoing_values() {
        assert!(error(&["--port", "7567"]).contains("port 7567 is reserved"));
        assert_eq!(
            error(&["--port", "70000"]),
            "--port must be a number from 0 to 65535"
        );
        assert_eq!(error(&["--port"]), "--port requires a port number");
        assert_eq!(
            error(&["--port", "1", "--port", "2"]),
            "--port given more than once"
        );
        assert_eq!(error(&["--config"]), "--config requires a path");
        assert_eq!(error(&["--json", "--json"]), "--json given more than once");
        let message = error(&["S3NT1NEL"]);
        assert!(!message.contains("S3NT1NEL"), "{message}");
    }
}
