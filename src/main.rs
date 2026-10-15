//! `pumice` binary entry point: the orchestrator that dispatches to the
//! modules. The relay runs in-process from its library; transcription runs
//! as the `pumice-transcribe` program installed next to this executable.

mod dispatch;
mod orchestrator;

use std::process::ExitCode;

fn main() -> ExitCode {
    dispatch::run()
}
