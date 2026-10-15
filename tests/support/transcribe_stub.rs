//! Test-only stand-in for the `pumice-transcribe` executable. The dispatcher
//! tests copy this binary next to a copy of `pumice` under the name
//! `pumice-transcribe` and verify the dispatch contract (argument pass-through,
//! exit status propagation, install-relative lookup).
//!
//! Usage: pumice-test-transcribe [EXIT_CODE] [ARGS…]
//!
//! Prints every argument after EXIT_CODE on its own line to stdout and exits
//! with EXIT_CODE (default 0 when absent or unparsable).

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let code = args
        .next()
        .and_then(|arg| arg.into_string().ok())
        .and_then(|arg| arg.parse::<u8>().ok())
        .unwrap_or(0);
    for arg in args {
        println!("{}", arg.to_string_lossy());
    }
    ExitCode::from(code)
}
