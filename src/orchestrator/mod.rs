//! The orchestrator: `pumice run` combines the modules into one flow
//! (p12-pumice-run) — transcription runs as the sibling `pumice-transcribe`
//! program, the formatter and the relay run in-process.

pub mod cancel;
pub mod cli;
pub mod config;
pub mod error;
pub mod events;
pub mod relay;
pub mod result;
pub mod run;
pub mod transcription;
