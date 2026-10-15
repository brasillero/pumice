//! `pumice-format` binary entry point: a thin CLI on the library, so the
//! formatter runs standalone and in-process inside `pumice` with exactly the
//! same code (arch-modules/D13).

use std::process::ExitCode;

fn main() -> ExitCode {
    // The full CLI lands with the formatter commands; until then there is
    // nothing to run.
    eprintln!("error: pumice-format has no commands yet");
    ExitCode::from(2)
}
