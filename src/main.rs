//! `pumice` binary entry point.
//!
//! This is the legacy product executable. It delegates to the relay library
//! so that today's CLI behavior is preserved while the relay lives in its own
//! workspace crate.

use std::process::ExitCode;

fn main() -> ExitCode {
    pumice_relay::cli::run("pumice")
}
