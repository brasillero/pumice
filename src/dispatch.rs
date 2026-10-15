//! `pumice` command dispatch: the orchestrator entry point hands each
//! subcommand to its module. The relay and the formatter run in-process from
//! their libraries (staying inside this single executable); transcription
//! runs as the `pumice-transcribe` program installed next to this executable.
//!
//! On Unix `pumice transcribe` replaces itself with the sibling program
//! (`CommandExt::exec`), so arguments (as `OsString`), signals and the exit
//! status behave exactly as if the sibling had been run directly, with no
//! forwarding layer in between (p10-pumice-entry/D4). On Windows, where exec
//! does not exist, it spawns the sibling, keeps itself alive through console
//! Ctrl-C/Ctrl-Break with a control handler while the child receives the
//! event, and exits with the child's full exit code.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

const OVERVIEW: &str = "\
pumice - connects dictation apps to the AI assistants you already use

usage:
  pumice relay [--config <path>] [--plain]     run the relay service on 127.0.0.1 (default)
  pumice relay serve                           alias for the command above
  pumice relay check-config [--config <path>]  validate and summarize the relay configuration
  pumice relay doctor [--config <path>]        show which provider CLIs are installed
  pumice format list [--config <path>]         list formatters (built-ins and configured)
  pumice format show <id> [--config <path>]    print one formatter definition
  pumice format apply [--formatter <id>]       insert a formatter's rules into one chat request
  pumice format check-config [--config <path>] validate the formatter configuration
  pumice transcribe [ARGS…]                    run the transcription module installed next to pumice
  pumice run <AUDIO> --flow <ID> [--config <path>] [--json]
                                               transcribe, format and relay one audio file
  pumice --help                                print this help
  pumice --version                             print the version

The modules' own options are documented under `pumice relay --help`,
`pumice format --help` and `pumice transcribe --help`.
";

pub fn run() -> ExitCode {
    // `std::env::args` panics on an argument that is not valid Unicode;
    // everything here stays an `OsString` and reaches the sibling unchanged.
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match args.as_slice() {
        [flag] if flag == "-h" || flag == "--help" => {
            print!("{OVERVIEW}");
            ExitCode::SUCCESS
        }
        [flag] if flag == "-V" || flag == "--version" => {
            println!("pumice {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        [command, rest @ ..] if command == "relay" => relay(rest),
        [command, rest @ ..] if command == "format" => format(rest),
        [command, rest @ ..] if command == "transcribe" => transcribe(rest),
        [command, rest @ ..] if command == "run" => crate::orchestrator::cli::run(rest),
        [] => {
            // Bare `pumice` is the future orchestrator; until the stack
            // exists there is nothing to run, so say so instead of silently
            // starting a module (p10-pumice-entry/D1).
            eprint!("{OVERVIEW}");
            ExitCode::from(2)
        }
        [first, ..] => {
            eprintln!("error: unknown command '{}'\n", first.to_string_lossy());
            eprint!("{OVERVIEW}");
            ExitCode::from(2)
        }
    }
}

/// Runs the relay CLI in-process with the same argument handling as the
/// standalone `pumice-relay` binary; a bare `relay` starts the service.
fn relay(args: &[OsString]) -> ExitCode {
    pumice_relay::cli::run_with("pumice relay", args.iter().cloned())
}

/// Runs the formatter CLI in-process with the same argument handling as the
/// standalone `pumice-format` binary (arch-modules/D13); the program name in
/// help and errors is `pumice format`.
fn format(args: &[OsString]) -> ExitCode {
    pumice_formatter::cli::run_with("pumice format", args.iter().cloned())
}

#[cfg(unix)]
fn transcribe(args: &[OsString]) -> ExitCode {
    use std::os::unix::process::CommandExt;

    let Ok(path) = transcribe_path() else {
        return ExitCode::from(1);
    };
    let error = Command::new(&path)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        // Only returns when the exec itself failed; otherwise this process
        // is now the sibling, with its arguments, signals and exit status.
        .exec();
    eprintln!("error: cannot start {}: {error}", path.display());
    ExitCode::from(1)
}

#[cfg(windows)]
fn transcribe(args: &[OsString]) -> ExitCode {
    let Ok(path) = transcribe_path() else {
        return ExitCode::from(1);
    };

    // A console Ctrl-C/Ctrl-Break reaches every process sharing the console.
    // This process must keep waiting for the child while the child shuts
    // down, so a control-handler function reports the event as handled
    // (keeping this process alive) instead of the default handler exiting it
    // mid-shutdown. The NULL-handler "ignore" mode is deliberately not used:
    // that state is inherited by the child, which would then never receive
    // the event (SetConsoleCtrlHandler documentation).
    unsafe extern "system" fn keep_waiting(_ctrl_event: u32) -> i32 {
        windows_sys::Win32::Foundation::TRUE
    }
    let installed = unsafe {
        windows_sys::Win32::System::Console::SetConsoleCtrlHandler(
            Some(keep_waiting),
            windows_sys::Win32::Foundation::TRUE,
        )
    };
    if installed == 0 {
        eprintln!("error: cannot install the console control handler");
        return ExitCode::from(1);
    }

    let mut command = Command::new(&path);
    command
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            eprintln!("error: cannot start {}: {error}", path.display());
            return ExitCode::from(1);
        }
    };
    let status = match child.wait() {
        Ok(status) => status,
        Err(error) => {
            eprintln!("error: cannot wait for {}: {error}", path.display());
            return ExitCode::from(1);
        }
    };
    // `ExitCode` would truncate the status to eight bits; `process::exit`
    // forwards the child's full exit code to the OS.
    std::process::exit(status.code().unwrap_or(1));
}

/// Resolves the transcription executable next to the real path of the
/// running `pumice` (canonicalized, so a symlinked `pumice` still resolves
/// its own install directory), printing the reason and `Err(())` when it
/// cannot run. PATH and the working directory are never searched
/// (p10-pumice-entry/D2).
fn transcribe_path() -> Result<PathBuf, ()> {
    match sibling_transcribe() {
        Ok(path) => Ok(path),
        Err(SiblingError::NoSelfPath) => {
            eprintln!("error: cannot locate the running pumice executable");
            Err(())
        }
        Err(SiblingError::Missing(dir)) => {
            eprintln!(
                "error: pumice-transcribe is not installed next to pumice ({}); \
                 install it with the Pumice transcription archive (see docs/transcription.md) \
                 or `cargo install --path crates/transcription --features engine --locked`",
                display_dir(&dir)
            );
            Err(())
        }
    }
}

/// Why the sibling `pumice-transcribe` cannot run.
#[derive(Debug)]
pub(crate) enum SiblingError {
    /// The running pumice executable could not be located.
    NoSelfPath,
    /// No `pumice-transcribe` exists in this install directory.
    Missing(PathBuf),
}

/// The sibling lookup without any output, shared with `pumice run` (which
/// reports the failure through its own envelope).
pub(crate) fn sibling_transcribe() -> Result<PathBuf, SiblingError> {
    let Some(path) = sibling_transcribe_path() else {
        return Err(SiblingError::NoSelfPath);
    };
    if !path.is_file() {
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        return Err(SiblingError::Missing(dir));
    }
    Ok(path)
}

/// The directory as a user would type it: on Windows, `canonicalize` returns
/// a verbatim path (`\\?\C:\...`, `\\?\UNC\server\share\...`), which is
/// shown without the verbatim prefix.
fn display_dir(dir: &Path) -> String {
    let text = dir.display().to_string();
    if cfg!(windows) {
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{rest}");
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return rest.to_string();
        }
    }
    text
}

fn sibling_transcribe_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let dir = exe.parent()?;
    let name = if cfg!(windows) {
        "pumice-transcribe.exe"
    } else {
        "pumice-transcribe"
    };
    Some(dir.join(name))
}
