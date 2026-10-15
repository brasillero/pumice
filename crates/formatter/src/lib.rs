//! Pumice formatter library.
//!
//! The formatter injects an explicitly selected formatter's rules, as one
//! `system` message, into an OpenAI-shaped chat request before it reaches the
//! relay (`arch-modules/D3`). It never calls a provider, rewrites the model or
//! touches anything but the `messages` array; the relay stays pass-through.
//! The library powers both the standalone `pumice-format` binary and the
//! in-process `pumice format …` subcommand (D13).

pub mod builtins;
pub mod config;
pub mod definition;
pub mod error;
pub mod transform;

pub use builtins::{BUILTIN_CLEAN, builtin_clean};
pub use config::{
    Config, ConfigError, ConfigSource, FormatterEntry, LoadedConfig, load, load_with_env,
};
pub use definition::{FormatterDefinition, load_definition_file};
pub use error::{Error, ErrorCode, JsonError};
pub use transform::{ConflictPolicy, apply};
