//! The orchestrator: `pumice run` combines the modules into one flow
//! (p12-pumice-run) — transcription runs as the sibling `pumice-transcribe`
//! program, the formatter and the relay run in-process.

// Wired into the `run` subcommand by the follow-up commit of p12; until
// then the module is exercised by its unit tests only.
#[allow(dead_code)]
pub mod config;
#[allow(dead_code)]
pub mod error;
