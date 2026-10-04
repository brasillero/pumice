//! Pumice prototype CLI: `pumice listen` (logging listener) and
//! `pumice claude-probe` (stack validation).

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use pumice::listen::{self, ListenOptions};
use pumice::probe::{self, ProbeOptions};

const USAGE: &str = "\
pumice — local dictation formatting service (Phase 0 prototype)

usage:
  pumice listen --port <PORT> [--reply-prefix <TEXT>] [--log-file <PATH>]
  pumice claude-probe [--text <TEXT>] [--claude-bin <PATH>] [--timeout-secs <N>]
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("listen") => cmd_listen(&args[1..]),
        Some("claude-probe") => cmd_probe(&args[1..]),
        Some("-h" | "--help") => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("error: unknown subcommand '{other}'\n");
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
        None => {
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// Takes the value following the flag at index `i`.
fn flag_value(args: &[String], i: &mut usize, flag: &str) -> Result<String, String> {
    *i += 1;
    args.get(*i)
        .cloned()
        .ok_or_else(|| format!("missing value for {flag}"))
}

fn cmd_listen(args: &[String]) -> ExitCode {
    let mut port: Option<u16> = None;
    let mut reply_prefix = String::new();
    let mut log_file: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        match flag {
            "--port" => {
                let value = match flag_value(args, &mut i, flag) {
                    Ok(v) => v,
                    Err(e) => return usage_error(&e),
                };
                port = Some(match value.parse() {
                    Ok(p) => p,
                    Err(_) => return usage_error(&format!("invalid --port value: {value:?}")),
                });
            }
            "--reply-prefix" => match flag_value(args, &mut i, flag) {
                Ok(v) => reply_prefix = v,
                Err(e) => return usage_error(&e),
            },
            "--log-file" => match flag_value(args, &mut i, flag) {
                Ok(v) => log_file = Some(PathBuf::from(v)),
                Err(e) => return usage_error(&e),
            },
            "-h" | "--help" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => return usage_error(&format!("unknown argument '{other}'")),
        }
        i += 1;
    }

    let port = match port {
        Some(port) => port,
        None => return usage_error("--port is required"),
    };

    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("error: cannot listen on 127.0.0.1:{port}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let bound_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    println!("listening on http://127.0.0.1:{bound_port}");

    match listen::serve(
        listener,
        &ListenOptions {
            reply_prefix,
            log_file,
        },
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: listener stopped: {e}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_probe(args: &[String]) -> ExitCode {
    let mut text = probe::DEFAULT_SAMPLE_TEXT.to_string();
    let mut claude_bin = PathBuf::from(probe::DEFAULT_CLAUDE_BIN);
    let mut timeout = Duration::from_secs(probe::DEFAULT_TIMEOUT_SECS);

    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        match flag {
            "--text" => match flag_value(args, &mut i, flag) {
                Ok(v) => text = v,
                Err(e) => return usage_error(&e),
            },
            "--claude-bin" => match flag_value(args, &mut i, flag) {
                Ok(v) => claude_bin = PathBuf::from(v),
                Err(e) => return usage_error(&e),
            },
            "--timeout-secs" => {
                let value = match flag_value(args, &mut i, flag) {
                    Ok(v) => v,
                    Err(e) => return usage_error(&e),
                };
                timeout = match value.parse::<u64>() {
                    Ok(secs) => Duration::from_secs(secs),
                    Err(_) => {
                        return usage_error(&format!("invalid --timeout-secs value: {value:?}"));
                    }
                };
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => return usage_error(&format!("unknown argument '{other}'")),
        }
        i += 1;
    }

    let options = ProbeOptions {
        text,
        claude_bin,
        timeout,
    };
    match probe::run_probe(&options) {
        Ok(success) => {
            println!("formatted text:\n{}", success.text);
            println!("elapsed: {} ms", success.elapsed.as_millis());
            if let Some(is_error) = success.is_error {
                println!("is_error: {is_error}");
            }
            match success.exit_status.code() {
                Some(code) => println!("exit status: {code}"),
                None => println!("exit status: terminated by signal"),
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn usage_error(message: &str) -> ExitCode {
    eprintln!("error: {message}\n");
    eprint!("{USAGE}");
    ExitCode::from(2)
}
