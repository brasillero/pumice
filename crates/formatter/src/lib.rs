//! Pumice formatter library.
//!
//! The formatter injects an explicitly selected formatter's rules, as one
//! `system` message, into an OpenAI-shaped chat request before it reaches the
//! relay (`arch-modules/D3`). It never calls a provider or rewrites the model:
//! besides inserting into the `messages` array it only normalizes top-level
//! key names, and every value is re-emitted byte for byte; the relay stays
//! pass-through.
//! The library powers both the standalone `pumice-format` binary and the
//! in-process `pumice format …` subcommand (D13).

pub mod builtins;
pub mod cli;
pub mod config;
pub mod definition;
pub mod error;
pub mod transform;

pub use builtins::{BUILTIN_CLEAN, builtin_clean};
pub use config::{
    Config, ConfigError, FormatterEntry, LoadedConfig, load, load_optional, load_optional_with_env,
    load_with_env,
};
pub use definition::{FormatterDefinition, load_definition_file};
pub use error::{Error, ErrorCode, JsonError};
pub use transform::{ConflictPolicy, apply};
