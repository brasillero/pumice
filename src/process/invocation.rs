//! Declarative description of one CLI call.
//!
//! Adapters describe what to run; the [`ProcessRunner`](super::ProcessRunner)
//! owns spawning, directories, deadlines and process cleanup.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;
use std::process::ExitStatus;

use crate::providers::ProviderError;

/// One argv element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Argument {
    /// Passed as-is. May be empty (for example `--tools ""`).
    Literal(OsString),
    /// Absolute path of `control_files[file]`, materialized by the runner.
    ControlPath { file: usize },
}

impl Argument {
    pub fn literal(value: impl Into<OsString>) -> Argument {
        Argument::Literal(value.into())
    }
}

/// A file the runner writes outside the CLI's working directory before the
/// call, such as a system prompt.
#[derive(Clone)]
pub struct ControlFile {
    /// Plain file name (no directory parts), e.g. `"system.txt"`.
    pub name: &'static str,
    pub contents: Vec<u8>,
}

impl fmt::Debug for ControlFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ControlFile")
            .field("name", &self.name)
            .field("contents_len", &self.contents.len())
            .finish()
    }
}

/// The program to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramSpec {
    /// A bare command name (looked up on PATH) or a path.
    pub binary: PathBuf,
    /// Package entrypoint used to launch a supported npm shim directly.
    /// Not used until Windows shim support lands (S5.1).
    pub npm_entrypoint: Option<&'static str>,
}

/// What a finished CLI call produced.
pub struct ProcessOutput {
    pub status: ExitStatus,
    /// Complete stdout (bounded by the runner's limit).
    pub stdout: Vec<u8>,
    /// The last bytes of stderr only.
    pub stderr_tail: Vec<u8>,
}

impl fmt::Debug for ProcessOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessOutput")
            .field("status", &self.status)
            .field("stdout_len", &self.stdout.len())
            .field("stderr_tail_len", &self.stderr_tail.len())
            .finish()
    }
}

/// Turns a finished call's output into the formatted text.
pub type OutputParser = fn(&ProcessOutput) -> Result<String, ProviderError>;

/// A complete, declarative CLI call.
pub struct CliInvocation {
    pub program: ProgramSpec,
    pub args: Vec<Argument>,
    pub stdin: Vec<u8>,
    /// Variables set in the child, on top of the inherited environment.
    pub env: BTreeMap<OsString, OsString>,
    /// Variables removed from the child's environment.
    pub remove_env: Vec<OsString>,
    pub control_files: Vec<ControlFile>,
    pub parser: OutputParser,
}

impl fmt::Debug for CliInvocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Arguments, stdin and environment values can hold prompts or
        // dictated text, so only their shape is shown.
        f.debug_struct("CliInvocation")
            .field("program", &self.program)
            .field("args_len", &self.args.len())
            .field("stdin_len", &self.stdin.len())
            .field("env_keys", &self.env.keys().collect::<Vec<_>>())
            .field("remove_env", &self.remove_env)
            .field("control_files", &self.control_files)
            .finish_non_exhaustive()
    }
}
