//! `pumice-relay` binary entry point.

use std::process::ExitCode;

fn main() -> ExitCode {
    // Before any child is spawned: a child orphaned by a hard kill of this
    // process must not hold its caller's pipes (Windows).
    pumice_relay::process::make_std_handles_non_inheritable();
    pumice_relay::cli::run("pumice-relay")
}
