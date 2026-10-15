//! Pumice formatter library.
//!
//! The formatter injects an explicitly selected formatter's rules, as one
//! `system` message, into an OpenAI-shaped chat request before it reaches the
//! relay (`arch-modules/D3`). It never calls a provider, rewrites the model or
//! touches anything but the `messages` array; the relay stays pass-through.
//! The library powers both the standalone `pumice-format` binary and the
//! in-process `pumice format …` subcommand (D13).

pub mod error;

pub use error::{Error, ErrorCode, JsonError};
