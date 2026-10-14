//! `pumice-relay` binary entry point.

use std::process::ExitCode;

fn main() -> ExitCode {
    pumice_relay::cli::run("pumice-relay")
}
