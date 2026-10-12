use pumice_transcription::cli;
use std::process::ExitCode;

fn main() -> ExitCode {
    cli::run(std::env::args_os())
}
