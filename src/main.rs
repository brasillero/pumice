//! `pumice` binary entry point: the orchestrator that dispatches to the
//! modules. The relay runs in-process from its library; transcription runs
//! as the `pumice-transcribe` program installed next to this executable.

use std::process::ExitCode;

fn main() -> ExitCode {
    // Before any child is spawned: a child orphaned by a hard kill of this
    // process must not hold its caller's pipes (Windows).
    pumice_relay::process::make_std_handles_non_inheritable();
    pumice::dispatch::run()
}
