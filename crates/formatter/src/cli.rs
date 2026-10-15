//! The `pumice-format` CLI: `list`, `show`, `apply` and `check-config`.
//!
//! Designed so a UI can drive it later (arch-modules/D10): every command
//! supports `--json` (`apply` without it prints the bare transformed
//! request JSON), one-shot runs exit after printing, stdout
//! carries only the result, and exit codes are 0 (success), 2 (invalid
//! arguments, config or input — including an instruction conflict) and 1
//! (operational failure). Errors never contain request text.

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args as ClapArgs, CommandFactory, FromArgMatches, Parser, Subcommand};
use serde::Serialize;

use crate::builtins::{BUILTIN_CLEAN, HANDY_MIT_NOTICE, builtin_clean};
use crate::config::{self, Config, ConfigSource, LoadedConfig};
use crate::definition::FormatterDefinition;
use crate::error::{Error, ErrorCode, JsonError};
use crate::transform::{self, ConflictPolicy};

/// The maximum size of one request read by `apply`: 1 MiB.
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;

#[derive(Parser, Debug)]
#[command(
    name = "pumice-format",
    about = "Inject selected formatting rules into OpenAI chat requests",
    version
)]
pub struct Args {
    /// Path to the module configuration file.
    #[arg(long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// List the available formatters (built-ins and configured ones).
    List(ListArgs),
    /// Print one formatter definition as an editable YAML file.
    Show(ShowArgs),
    /// Insert a formatter's rules into one chat request.
    Apply(ApplyArgs),
    /// Validate the configuration file and exit.
    CheckConfig(CheckConfigArgs),
}

#[derive(ClapArgs, Debug)]
struct ListArgs {
    /// Output a JSON envelope instead of a table.
    #[arg(long)]
    json: bool,
}

#[derive(ClapArgs, Debug)]
struct ShowArgs {
    /// Formatter id (a configured id or `builtin:clean`).
    id: String,

    /// Output a JSON envelope instead of a YAML definition.
    #[arg(long)]
    json: bool,
}

#[derive(ClapArgs, Debug)]
struct ApplyArgs {
    /// Formatter to apply (defaults to the configured `default`).
    #[arg(long, value_name = "ID")]
    formatter: Option<String>,

    /// What to do when the request already has non-empty system/developer
    /// instructions: refuse (default) or insert after them.
    #[arg(long, value_name = "POLICY", default_value_t = ConflictPolicy::Reject)]
    on_conflict: ConflictPolicy,

    /// Read the request JSON from this file, or `-` for stdin (default).
    #[arg(long, value_name = "FILE", default_value = "-")]
    request: PathBuf,

    /// Output a JSON envelope around the transformed request.
    #[arg(long)]
    json: bool,
}

#[derive(ClapArgs, Debug)]
struct CheckConfigArgs {
    /// Output a JSON envelope instead of plain text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Serialize)]
struct Envelope {
    schema_version: i32,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonError>,
}

#[derive(Debug, Clone, Serialize)]
struct ListEntry {
    id: String,
    description: String,
    builtin: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<PathBuf>,
    default: bool,
}

enum CommandOutput {
    List(Vec<ListEntry>),
    Show {
        text: String,
        json_result: serde_json::Value,
    },
    Apply(serde_json::Value),
    CheckConfig(Option<PathBuf>),
}

pub fn run() -> ExitCode {
    run_with("pumice-format", std::env::args_os().skip(1))
}

/// Runs the CLI as `name` with an explicit argument list (excluding the
/// program name), so the same code serves the standalone `pumice-format`
/// binary and the in-process `pumice format …` subcommand with the right
/// program name in help and errors.
pub fn run_with(name: &str, args: impl IntoIterator<Item = OsString>) -> ExitCode {
    // `clap` needs owned `String`s here; like the relay, non-Unicode
    // arguments are an explicit usage error, not a panic.
    let Some(argv) = args
        .into_iter()
        .map(|arg| arg.into_string().ok())
        .collect::<Option<Vec<String>>>()
    else {
        eprintln!("error: arguments must be valid Unicode\n");
        eprintln!("{}", usage_hint(name));
        return ExitCode::from(2);
    };
    let wants_json = argv.iter().any(|arg| arg == "--json");

    let mut command = <Args as CommandFactory>::command();
    command.set_bin_name(name);
    let matches = match command.try_get_matches_from(std::iter::once(name.to_owned()).chain(argv)) {
        Ok(matches) => matches,
        Err(err) => return handle_clap_error(err, wants_json),
    };
    let args = match <Args as FromArgMatches>::from_arg_matches(&matches) {
        Ok(args) => args,
        Err(err) => return handle_clap_error(err, wants_json),
    };

    let json = json_flag(&args);
    let mut stdout = io::stdout();
    let mut stderr = io::stderr();

    match run_command(args) {
        Ok(CommandOutput::List(entries)) => match print_list(&entries, json, &mut stdout) {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => write_failure(&mut stderr),
        },
        Ok(CommandOutput::Show { text, json_result }) => {
            if json {
                let envelope = Envelope {
                    schema_version: 1,
                    ok: true,
                    result: Some(json_result),
                    error: None,
                };
                match writeln!(stdout, "{}", serde_json::to_string(&envelope).unwrap()) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(_) => write_failure(&mut stderr),
                }
            } else {
                match writeln!(stdout, "{text}") {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(_) => write_failure(&mut stderr),
                }
            }
        }
        Ok(CommandOutput::Apply(request)) => {
            let printed = if json {
                let envelope = Envelope {
                    schema_version: 1,
                    ok: true,
                    result: Some(request),
                    error: None,
                };
                serde_json::to_string(&envelope).unwrap()
            } else {
                serde_json::to_string(&request).unwrap()
            };
            match writeln!(stdout, "{printed}") {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => write_failure(&mut stderr),
            }
        }
        Ok(CommandOutput::CheckConfig(file)) => {
            match print_check_config(file.as_deref(), json, &mut stdout) {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => write_failure(&mut stderr),
            }
        }
        Err(error) => match print_error(&error, json, &mut stdout, &mut stderr) {
            Ok(()) => exit_code(&error),
            Err(_) => write_failure(&mut stderr),
        },
    }
}

fn usage_hint(name: &str) -> String {
    format!("usage: {name} [--config <path>] <command>")
}

fn handle_clap_error(err: clap::Error, wants_json: bool) -> ExitCode {
    use clap::error::ErrorKind;
    match err.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
            if err.print().is_err() {
                return ExitCode::from(1);
            }
            ExitCode::SUCCESS
        }
        _ if wants_json => {
            let envelope = Envelope {
                schema_version: 1,
                ok: false,
                result: None,
                error: Some(JsonError {
                    code: ErrorCode::InvalidArguments.as_str().to_owned(),
                    message: err.to_string().trim().to_owned(),
                    location: None,
                }),
            };
            if writeln!(
                io::stdout(),
                "{}",
                serde_json::to_string(&envelope).unwrap()
            )
            .is_err()
            {
                return ExitCode::from(1);
            }
            ExitCode::from(2)
        }
        _ => {
            if err.print().is_err() {
                return ExitCode::from(1);
            }
            ExitCode::from(2)
        }
    }
}

fn run_command(args: Args) -> Result<CommandOutput, Error> {
    match args.command {
        Command::List(_) => {
            let loaded = load_config(args.config.as_deref())?;
            Ok(CommandOutput::List(build_list(&loaded.config)))
        }
        Command::Show(show_args) => {
            if show_args.id == BUILTIN_CLEAN {
                let definition = builtin_clean();
                return Ok(CommandOutput::Show {
                    text: render_builtin_definition(&definition),
                    json_result: show_json_result(&definition, true, None),
                });
            }
            let loaded = load_config(args.config.as_deref())?;
            let entry = loaded
                .config
                .formatters
                .iter()
                .find(|entry| entry.definition.id == show_args.id)
                .ok_or_else(|| Error::formatter_not_found(&show_args.id))?;
            Ok(CommandOutput::Show {
                text: render_file_definition(&entry.definition, &entry.path),
                json_result: show_json_result(&entry.definition, false, Some(&entry.path)),
            })
        }
        Command::Apply(apply_args) => {
            let loaded = load_config(args.config.as_deref())?;
            let definition = resolve_formatter(apply_args.formatter.as_deref(), &loaded.config)?;
            let request = read_request(&apply_args.request)?;
            let request: serde_json::Value = serde_json::from_slice(&request).map_err(|err| {
                Error::invalid_input(format!("the request is not valid JSON: {err}"))
            })?;
            let transformed = transform::apply(request, &definition, apply_args.on_conflict)?;
            Ok(CommandOutput::Apply(transformed))
        }
        Command::CheckConfig(_) => Ok(CommandOutput::CheckConfig(
            match load_config(args.config.as_deref())? {
                LoadedConfig {
                    source: ConfigSource::File(path),
                    ..
                } => Some(path),
                LoadedConfig {
                    source: ConfigSource::NoFile,
                    ..
                } => None,
            },
        )),
    }
}

fn load_config(explicit: Option<&Path>) -> Result<LoadedConfig, Error> {
    config::load(explicit).map_err(Error::invalid_config)
}

/// Resolves which formatter to apply: `--formatter` when given, else the
/// configured `default`. Built-ins resolve before registered definitions.
fn resolve_formatter(
    requested: Option<&str>,
    config: &Config,
) -> Result<FormatterDefinition, Error> {
    let id = match requested.or(config.default.as_deref()) {
        Some(id) => id,
        None => {
            return Err(Error::invalid_arguments(
                "no formatter selected; pass --formatter ID or set `default` in formatter.yml",
            ));
        }
    };
    if id == BUILTIN_CLEAN {
        return Ok(builtin_clean());
    }
    config
        .definition(id)
        .cloned()
        .ok_or_else(|| Error::formatter_not_found(id))
}

/// Reads one request, bounded to [`MAX_REQUEST_BYTES`], from a file or
/// stdin (`-`).
fn read_request(path: &Path) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    if path == Path::new("-") {
        io::stdin()
            .lock()
            .take(MAX_REQUEST_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
    } else {
        let file = std::fs::File::open(path).map_err(|err| {
            Error::invalid_input(format!(
                "cannot open request file {}: {err}",
                path.display()
            ))
        })?;
        file.take(MAX_REQUEST_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
    }
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(Error::input_too_large(bytes.len()));
    }
    Ok(bytes)
}

/// The `result` member of a `show --json` envelope.
#[derive(Debug, Serialize)]
struct ShowResult<'a> {
    id: &'a str,
    description: &'a str,
    system: &'a str,
    builtin: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<&'a Path>,
}

fn show_json_result(
    definition: &FormatterDefinition,
    builtin: bool,
    path: Option<&Path>,
) -> serde_json::Value {
    serde_json::to_value(ShowResult {
        id: &definition.id,
        description: &definition.description,
        system: &definition.system,
        builtin,
        path,
    })
    .unwrap()
}

fn build_list(config: &Config) -> Vec<ListEntry> {
    let mut entries = vec![ListEntry {
        id: BUILTIN_CLEAN.to_owned(),
        description: builtin_clean().description,
        builtin: true,
        path: None,
        default: config.default.as_deref() == Some(BUILTIN_CLEAN),
    }];
    entries.extend(config.formatters.iter().map(|entry| ListEntry {
        id: entry.definition.id.clone(),
        description: entry.definition.description.clone(),
        builtin: false,
        path: Some(entry.path.clone()),
        default: config.default.as_deref() == Some(entry.definition.id.as_str()),
    }));
    entries
}

/// Renders a built-in definition as an editable file: `show builtin:clean >
/// my.yaml` must produce a valid definition the user can register, so the
/// reserved `builtin:` id is replaced by a placeholder. The full Handy MIT
/// notice travels with the copy, as the license requires.
fn render_builtin_definition(definition: &FormatterDefinition) -> String {
    let mut header = String::from(
        "# Pumice formatter definition (schema_version: 1).\n\
         # Adapted from the default cleanup prompt of Handy\n\
         # (https://github.com/cjpais/Handy), used under the MIT license:\n\
         #\n",
    );
    for line in HANDY_MIT_NOTICE.lines() {
        // Keep blank license lines bare so they stay valid YAML comments.
        if line.is_empty() {
            header.push_str("#\n");
        } else {
            header.push_str("# ");
            header.push_str(line);
            header.push('\n');
        }
    }
    header.push_str(
        "#\n\
         # Choose a new id below (lowercase letters, digits and '-'; ids\n\
         # starting with \"builtin:\" are reserved), then register the file\n\
         # in formatter.yml.",
    );
    render_definition_yaml(definition, "my-clean", &header)
}

/// Renders a registered definition back into the file's own shape, noting
/// where it was loaded from.
fn render_file_definition(definition: &FormatterDefinition, path: &Path) -> String {
    render_definition_yaml(
        definition,
        &definition.id,
        &format!(
            "# Pumice formatter definition (schema_version: 1).\n\
             # Loaded from {}; edit the file in place.",
            path.display()
        ),
    )
}

fn render_definition_yaml(definition: &FormatterDefinition, id: &str, header: &str) -> String {
    let mut out = String::new();
    for line in header.lines() {
        out.push_str(line);
        out.push('\n');
    }
    let rendered = RenderedDefinition {
        schema_version: 1,
        id,
        description: &definition.description,
        system: &definition.system,
    };
    out.push_str(
        &serde_saphyr::to_string(&rendered).expect("a formatter definition always serializes"),
    );
    out
}

/// The YAML shape of one definition file. `show` serializes through
/// serde-saphyr — the same crate that loads definitions — so the output is
/// always a valid file that reloads to the exact same text, including
/// leading whitespace, carriage returns and trailing newlines.
#[derive(Serialize)]
struct RenderedDefinition<'a> {
    schema_version: u32,
    id: &'a str,
    description: &'a str,
    system: &'a str,
}

fn print_list(entries: &[ListEntry], json: bool, stdout: &mut dyn Write) -> io::Result<()> {
    if json {
        let result = serde_json::json!({ "formatters": entries });
        let envelope = Envelope {
            schema_version: 1,
            ok: true,
            result: Some(result),
            error: None,
        };
        return writeln!(stdout, "{}", serde_json::to_string(&envelope).unwrap());
    }
    let id_width = entries
        .iter()
        .map(|entry| entry.id.len())
        .max()
        .unwrap_or(0)
        .max(2);
    writeln!(
        stdout,
        "{:<id_width$}  {:<9}  {:<7}  DESCRIPTION",
        "ID", "SOURCE", "DEFAULT",
    )?;
    for entry in entries {
        let source = entry
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "built-in".to_owned());
        writeln!(
            stdout,
            "{:<id_width$}  {:<9}  {:<7}  {}",
            entry.id,
            source,
            if entry.default { "yes" } else { "no" },
            entry.description,
            id_width = id_width,
        )?;
    }
    Ok(())
}

fn print_check_config(file: Option<&Path>, json: bool, stdout: &mut dyn Write) -> io::Result<()> {
    if json {
        let envelope = Envelope {
            schema_version: 1,
            ok: true,
            result: Some(serde_json::to_value(serde_json::json!({
                "file": file,
            }))?),
            error: None,
        };
        return writeln!(stdout, "{}", serde_json::to_string(&envelope).unwrap());
    }
    match file {
        Some(path) => writeln!(stdout, "config ok: {}", path.display()),
        None => writeln!(stdout, "config ok: no per-user file found"),
    }
}

fn print_error(
    error: &Error,
    json: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> io::Result<()> {
    if json {
        let envelope = Envelope {
            schema_version: 1,
            ok: false,
            result: None,
            error: Some(error.to_json_error()),
        };
        writeln!(stdout, "{}", serde_json::to_string(&envelope).unwrap())
    } else {
        writeln!(stderr, "{error}")
    }
}

pub fn exit_code(error: &Error) -> ExitCode {
    let code = match error.code {
        ErrorCode::InvalidArguments
        | ErrorCode::InvalidInput
        | ErrorCode::InputTooLarge
        | ErrorCode::InvalidConfig
        | ErrorCode::FormatterNotFound
        | ErrorCode::FormatterInstructionConflict => 2,
        ErrorCode::Internal => 1,
    };
    ExitCode::from(code)
}

fn write_failure(stderr: &mut dyn Write) -> ExitCode {
    let _ = stderr.write_all(b"failed to write output\n");
    ExitCode::from(1)
}

fn json_flag(args: &Args) -> bool {
    match &args.command {
        Command::List(args) => args.json,
        Command::Show(args) => args.json,
        Command::Apply(args) => args.json,
        Command::CheckConfig(args) => args.json,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `show` output must be a definition file that reloads to the exact
    /// same text it was rendered from.
    fn assert_show_round_trip(definition: &FormatterDefinition) {
        let rendered = render_file_definition(definition, Path::new("formatters/x.yml"));
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("x.yml");
        std::fs::write(&path, &rendered).unwrap();
        let loaded = crate::load_definition_file(&path).unwrap();
        assert_eq!(&loaded, definition, "rendered:\n{rendered}");
    }

    #[test]
    fn builtin_render_carries_a_new_id_placeholder() {
        let rendered = render_builtin_definition(&builtin_clean());
        assert!(rendered.contains("id: my-clean"), "{rendered}");
        assert!(rendered.contains("Handy"), "attribution kept: {rendered}");
        assert!(
            rendered.contains("# Copyright (c) 2025 CJ Pais"),
            "full MIT notice kept: {rendered}"
        );
        // The rendered copy is a valid definition under its placeholder id.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("my.yaml");
        std::fs::write(&path, &rendered).unwrap();
        let loaded = crate::load_definition_file(&path).unwrap();
        assert_eq!(loaded.id, "my-clean");
        assert_eq!(loaded.system, builtin_clean().system);
    }

    #[test]
    fn file_render_round_trips_the_real_id() {
        let definition = FormatterDefinition {
            id: "clean".to_owned(),
            description: "Mine".to_owned(),
            system: "Rule one.\nRule two.".to_owned(),
        };
        let rendered = render_file_definition(&definition, Path::new("formatters/clean.yml"));
        assert!(rendered.contains("id: clean"), "{rendered}");
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("clean.yml");
        std::fs::write(&path, &rendered).unwrap();
        let loaded = crate::load_definition_file(&path).unwrap();
        assert_eq!(loaded, definition);
    }

    #[test]
    fn show_round_trips_leading_whitespace_lines() {
        assert_show_round_trip(&FormatterDefinition {
            id: "indent".to_owned(),
            description: "Indented first line".to_owned(),
            system: "  first\nsecond".to_owned(),
        });
    }

    #[test]
    fn show_round_trips_embedded_carriage_returns() {
        assert_show_round_trip(&FormatterDefinition {
            id: "crlf".to_owned(),
            description: "Carriage returns".to_owned(),
            system: "Rule\r\nSecond\rline".to_owned(),
        });
    }

    #[test]
    fn show_round_trips_trailing_blank_lines() {
        assert_show_round_trip(&FormatterDefinition {
            id: "trailing".to_owned(),
            description: "Trailing blank lines".to_owned(),
            system: "Rule\n\n".to_owned(),
        });
        assert_show_round_trip(&FormatterDefinition {
            id: "no-trailing".to_owned(),
            description: "No trailing newline".to_owned(),
            system: "Rule".to_owned(),
        });
    }

    #[test]
    fn show_round_trips_builtin_clean_exactly() {
        assert_show_round_trip(&FormatterDefinition {
            id: "clean".to_owned(),
            ..builtin_clean()
        });
    }
}
