//! `pumice-format` binary entry point: a thin CLI on the library, so the
//! formatter runs standalone and in-process inside `pumice` with exactly the
//! same code (arch-modules/D13).

use std::process::ExitCode;

fn main() -> ExitCode {
    pumice_formatter::cli::run()
}
