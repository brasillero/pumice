//! The `pumice run` command line: parse arguments, run the flow on a fresh
//! async runtime, and report exactly one result.
//!
//! Output contract (p12-pumice-run/D7): with `--json`, stdout carries exactly
//! one `schema_version: 1` envelope — success or failure alike — and stderr
//! carries bounded metadata-only JSONL events. Without `--json`, stdout is
//! the final text alone on success and empty on failure, and stderr explains
//! the failure. Exit codes: 0 success, 2 invalid arguments or configuration,
//! 1 runtime failure, 130 Ctrl-C, 143 SIGTERM.

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use crate::orchestrator::events::{self, EventWriter};
use crate::orchestrator::result::{
    MODULE, RunEnvelope, SCHEMA_VERSION, StageReports, Timings, stage,
};
use crate::orchestrator::run::{self, RunOutput, RunParams};

const USAGE: &str = "\
usage: pumice run <AUDIO> --flow <ID> [--config <FILE>] [--json]

Runs one audio file through the flow <ID> from the orchestrator
configuration: transcription (the pumice-transcribe program installed next
to pumice), the selected formatter and the relay, in one shot.

  --flow <ID>      the flow to run (required)
  --config <FILE>  the orchestrator configuration file
                   (default: the per-user orchestrator.yml)
  --json           print the result as one JSON document on stdout
  --help           print this help

Exit codes: 0 success; 2 invalid arguments or configuration; 1 runtime
failure; 130 Ctrl-C; 143 SIGTERM.
";

pub fn run(args: &[OsString]) -> ExitCode {
    // Decided before parsing so even argument errors can produce the JSON
    // envelope (same approach as pumice-transcribe).
    let json = args.iter().any(|arg| arg.as_encoded_bytes() == b"--json");
    match parse(args) {
        ParseResult::Help => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        ParseResult::Error(message) => {
            argument_failure(json, message);
            ExitCode::from(2)
        }
        ParseResult::Run(params) => run_flow(params, json),
    }
}

enum ParseResult {
    Help,
    Error(String),
    Run(RunParams),
}

/// Parses `run`'s arguments. Error messages name flags, never values: a
/// mistyped value is the user's input and is not echoed back.
fn parse(args: &[OsString]) -> ParseResult {
    if args
        .iter()
        .any(|arg| matches!(arg.as_encoded_bytes(), b"--help" | b"-h"))
    {
        return ParseResult::Help;
    }
    let mut audio: Option<OsString> = None;
    let mut flow: Option<String> = None;
    let mut config: Option<PathBuf> = None;
    let mut json_seen = false;
    let mut positionals_only = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let bytes = arg.as_encoded_bytes();
        if positionals_only {
            match audio {
                None => audio = Some(arg.clone()),
                Some(_) => return ParseResult::Error("exactly one audio file is accepted".into()),
            }
            continue;
        }
        match bytes {
            b"--" => positionals_only = true,
            b"--json" => {
                if json_seen {
                    return ParseResult::Error("--json given more than once".into());
                }
                json_seen = true;
            }
            b"--flow" => {
                if flow.is_some() {
                    return ParseResult::Error("--flow given more than once".into());
                }
                let Some(value) = iter.next() else {
                    return ParseResult::Error("--flow requires a flow id".into());
                };
                match value.clone().into_string() {
                    Ok(value) if value.trim().is_empty() => {
                        return ParseResult::Error("--flow must not be empty".into());
                    }
                    Ok(value) => flow = Some(value),
                    Err(_) => {
                        return ParseResult::Error("the flow id must be valid UTF-8".into());
                    }
                }
            }
            b"--config" => {
                if config.is_some() {
                    return ParseResult::Error("--config given more than once".into());
                }
                let Some(value) = iter.next() else {
                    return ParseResult::Error("--config requires a path".into());
                };
                config = Some(PathBuf::from(value));
            }
            _ if bytes.first() == Some(&b'-') && bytes.len() > 1 => {
                return ParseResult::Error("unknown option; see `pumice run --help`".into());
            }
            _ => match audio {
                None => audio = Some(arg.clone()),
                Some(_) => {
                    return ParseResult::Error("exactly one audio file is accepted".into());
                }
            },
        }
    }
    let Some(audio) = audio else {
        return ParseResult::Error("no audio file given".into());
    };
    let Some(flow) = flow else {
        return ParseResult::Error("--flow is required".into());
    };
    ParseResult::Run(RunParams {
        audio: PathBuf::from(audio),
        flow,
        config,
    })
}

fn run_flow(params: RunParams, json: bool) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            return internal_failure(json, format!("cannot start the async runtime: {error}"));
        }
    };
    let operation_id = events::operation_id();
    let mut writer = json.then(|| EventWriter::start(operation_id.clone()));
    let output = runtime.block_on(run::execute(&params, operation_id, writer.as_mut()));
    drop(runtime);
    if let Some(writer) = writer {
        writer.finish();
    }
    report(output, json)
}

/// Writes the result and returns the exit code. A failed stdout write cannot
/// be reported with another envelope on the broken stream, and is not
/// reported on stderr either: under `--json` the event thread may still hold
/// a blocked stderr, so any synchronous write there could hang the exit, and
/// a plain line would break the JSONL-only stderr. It exits 1 silently.
fn report(output: RunOutput, json: bool) -> ExitCode {
    let exit = output.exit_code();
    if json {
        let line =
            serde_json::to_string(&output.into_envelope()).expect("the envelope always serializes");
        if writeln!(std::io::stdout().lock(), "{line}").is_err() {
            return ExitCode::from(1);
        }
        return ExitCode::from(exit);
    }
    match (output.result, output.error) {
        (Some(text), None) => {
            let mut stdout = std::io::stdout().lock();
            let written = if text.text.ends_with('\n') {
                write!(stdout, "{}", text.text)
            } else {
                writeln!(stdout, "{}", text.text)
            };
            if written.is_err() {
                return ExitCode::from(1);
            }
            ExitCode::SUCCESS
        }
        (_, Some(error)) => {
            eprintln!("error: {}: {}", error.code, error.message);
            if output.transcript.is_some() {
                eprintln!(
                    "note: the raw transcript was preserved; rerun with --json to retrieve it"
                );
            }
            ExitCode::from(exit)
        }
        (None, None) => internal_failure(
            json,
            "the run produced neither a result nor an error".to_owned(),
        ),
    }
}

/// A failure envelope for argument errors (no flow was ever named).
fn argument_failure(json: bool, message: String) {
    if json {
        let envelope = RunEnvelope {
            schema_version: SCHEMA_VERSION,
            operation_id: events::operation_id(),
            module: MODULE,
            ok: false,
            flow: None,
            transcript: None,
            result: None,
            stages: StageReports::default(),
            timings_ms: Timings { total: 0 },
            error: Some(crate::orchestrator::result::FlowError {
                stage: stage::PREPARE.to_owned(),
                code: "invalid_arguments".to_owned(),
                message,
                retryable: false,
                cause_code: None,
                location: None,
            }),
        };
        let line = serde_json::to_string(&envelope).expect("the envelope always serializes");
        let _ = writeln!(std::io::stdout().lock(), "{line}");
    } else {
        eprintln!("error: {message}\n{USAGE}");
    }
}

fn internal_failure(json: bool, message: String) -> ExitCode {
    if json {
        let envelope = RunEnvelope {
            schema_version: SCHEMA_VERSION,
            operation_id: events::operation_id(),
            module: MODULE,
            ok: false,
            flow: None,
            transcript: None,
            result: None,
            stages: StageReports::default(),
            timings_ms: Timings { total: 0 },
            error: Some(crate::orchestrator::result::FlowError {
                stage: stage::PREPARE.to_owned(),
                code: "internal".to_owned(),
                message,
                retryable: false,
                cause_code: None,
                location: None,
            }),
        };
        let line = serde_json::to_string(&envelope).expect("the envelope always serializes");
        let _ = writeln!(std::io::stdout().lock(), "{line}");
    } else {
        eprintln!("error: internal: {message}");
    }
    ExitCode::from(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(args: &[&str]) -> RunParams {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        match parse(&args) {
            ParseResult::Run(params) => params,
            ParseResult::Help => panic!("unexpected help"),
            ParseResult::Error(message) => panic!("unexpected error: {message}"),
        }
    }

    fn parse_err(args: &[&str]) -> String {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        match parse(&args) {
            ParseResult::Error(message) => message,
            ParseResult::Help => panic!("unexpected help"),
            ParseResult::Run(_) => panic!("unexpected success"),
        }
    }

    #[test]
    fn parses_audio_flow_config_and_json() {
        let params = parse_ok(&[
            "audio.wav",
            "--flow",
            "clean",
            "--config",
            "x.yml",
            "--json",
        ]);
        assert_eq!(params.audio, PathBuf::from("audio.wav"));
        assert_eq!(params.flow, "clean");
        assert_eq!(params.config, Some(PathBuf::from("x.yml")));

        let params = parse_ok(&["--flow", "clean", "--json", "audio.wav"]);
        assert_eq!(params.flow, "clean");
    }

    #[test]
    fn help_is_help() {
        let args = vec![OsString::from("--help")];
        assert!(matches!(parse(&args), ParseResult::Help));
    }

    #[test]
    fn missing_and_repeated_arguments_are_errors() {
        assert_eq!(parse_err(&[]), "no audio file given");
        assert_eq!(parse_err(&["a.wav"]), "--flow is required");
        assert_eq!(
            parse_err(&["a.wav", "b.wav", "--flow", "x"]),
            "exactly one audio file is accepted"
        );
        assert_eq!(
            parse_err(&["a.wav", "--flow", "x", "--flow", "y"]),
            "--flow given more than once"
        );
        assert_eq!(
            parse_err(&["a.wav", "--flow", "x", "--json", "--json"]),
            "--json given more than once"
        );
        assert_eq!(parse_err(&["a.wav", "--flow"]), "--flow requires a flow id");
        assert_eq!(
            parse_err(&["a.wav", "--flow", "x", "--bogus"]),
            "unknown option; see `pumice run --help`"
        );
        // The `--` terminator allows a dash-leading audio path.
        let params = parse_ok(&["--flow", "x", "--", "-odd name.wav"]);
        assert_eq!(params.audio, PathBuf::from("-odd name.wav"));
    }
}
