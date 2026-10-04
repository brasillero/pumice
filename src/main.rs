//! Pumice command-line entry point.
//!
//! Phase 1 is in progress: this is a minimal scaffold. The `serve` command
//! (the local OpenAI-compatible service) is not implemented yet.

use std::process::ExitCode;

const USAGE: &str = "\
pumice - local dictation formatting service

usage:
  pumice --help       print this help
  pumice --version    print the version

The `serve` command (local OpenAI-compatible API) is coming in Phase 1 and is
not implemented yet.
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
