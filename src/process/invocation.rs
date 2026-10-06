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
    /// One `key="<absolute path>"` argument (for example Codex's
    /// `-c model_instructions_file="…"`), with the path of
    /// `control_files[file]` TOML-encoded inside the value. Materialized by
    /// the runner; paths never go through a shell.
    ConfigControlPath { key: &'static str, file: usize },
}

impl Argument {
    pub fn literal(value: impl Into<OsString>) -> Argument {
        Argument::Literal(value.into())
    }
}

/// Encodes `value` as a TOML basic string, surrounding quotes included, so it
/// can be embedded in a `-c key="<value>"` argument. Backslashes and quotes
/// are escaped (Windows paths contain backslashes); control characters use
/// the short or `\uXXXX` escapes; other characters, including non-ASCII, stay
/// literal.
pub fn toml_basic_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c <= '\u{1f}') || c == '\u{7f}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Inverse of [`toml_basic_string`] for the escapes it produces.
    fn decode(encoded: &str) -> String {
        let inner = encoded
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .expect("quoted");
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next().expect("escape") {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                'b' => out.push('\u{8}'),
                't' => out.push('\t'),
                'n' => out.push('\n'),
                'f' => out.push('\u{c}'),
                'r' => out.push('\r'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).unwrap()).unwrap());
                }
                other => panic!("unexpected escape \\{other}"),
            }
        }
        out
    }

    #[test]
    fn toml_string_escapes_windows_paths() {
        // A Windows temp path: backslashes double, nothing else changes.
        let path = r"C:\Users\João\AppData\Local\Temp\pumice-ab12\control\system.txt";
        assert_eq!(
            toml_basic_string(path),
            r#""C:\\Users\\João\\AppData\\Local\\Temp\\pumice-ab12\\control\\system.txt""#
        );
    }

    #[test]
    fn toml_string_escapes_quotes_and_control_characters() {
        assert_eq!(toml_basic_string("say \"hi\""), r#""say \"hi\"""#);
        assert_eq!(
            toml_basic_string("tab\tnul\u{0}bell\u{7}del\u{7f}"),
            "\"tab\\tnul\\u0000bell\\u0007del\\u007f\""
        );
    }

    #[test]
    fn toml_string_keeps_unicode_literal() {
        let value = "Olá, coração 🎤 — ação";
        assert_eq!(toml_basic_string(value), format!("\"{value}\""));
    }

    #[test]
    fn toml_string_round_trips() {
        for value in [
            r"C:\a\b.txt",
            "plain",
            "with \"quotes\" and \\ backslashes",
            "tab\tnewline\nfeed\u{c}return\r",
            "ünïcodé 🎤",
            "",
        ] {
            assert_eq!(decode(&toml_basic_string(value)), value);
        }
    }
}

/// A file the runner writes outside the CLI's working directory before the
/// call, such as a system prompt.
#[derive(Clone)]
pub struct ControlFile {
    /// File name or relative subpath, e.g. `"system.txt"` or
    /// `"agents/pumice.json"`. Must be relative and contain only normal path
    /// components (`..` is rejected).
    pub name: String,
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
    /// Package entrypoint inside the npm global `node_modules`, declared by
    /// the adapter so a standard Windows `.cmd` shim can be translated into
    /// a direct launch of its runtime. `None` refuses such shims.
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
    /// Files written inside the CLI's working directory before spawn.
    ///
    /// This lets an adapter supply workspace-local configuration (for
    /// example Kiro's `.kiro/agents/pumice.json`) without relocating the
    /// user's global profile. The directory is otherwise empty.
    pub workspace_files: Vec<ControlFile>,
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
            .field("workspace_files", &self.workspace_files)
            .finish_non_exhaustive()
    }
}
