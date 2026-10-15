//! `pumice` command dispatch: the orchestrator entry point hands each
//! subcommand to its module. The relay runs in-process from its library
//! (staying inside this single executable); transcription runs as the
//! `pumice-transcribe` program installed next to this executable.

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
  pumice transcribe [ARGS…]                    run the transcription module installed next to pumice
  pumice --help                                print this help
  pumice --version                             print the version

The relay's own options (and transcription's) are documented under
`pumice relay --help` and `pumice transcribe --help`.
";

pub fn run() -> ExitCode {
    // `std::env::args` panics on an argument that is not valid Unicode.
    let Some(args) = std::env::args_os()
        .skip(1)
        .map(|arg| arg.into_string().ok())
        .collect::<Option<Vec<String>>>()
    else {
        eprintln!("error: arguments must be valid Unicode\n");
        eprint!("{OVERVIEW}");
        return ExitCode::from(2);
    };
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["-h" | "--help"] => {
            print!("{OVERVIEW}");
            ExitCode::SUCCESS
        }
        ["-V" | "--version"] => {
            println!("pumice {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        ["relay", rest @ ..] => relay(rest),
        ["transcribe", rest @ ..] => transcribe(rest),
        [] => {
            // Bare `pumice` is the future orchestrator; until the stack
            // exists there is nothing to run, so say so instead of silently
            // starting a module (p10-pumice-entry/D1).
            eprint!("{OVERVIEW}");
            ExitCode::from(2)
        }
        [first, ..] => {
            eprintln!("error: unknown command '{first}'\n");
            eprint!("{OVERVIEW}");
            ExitCode::from(2)
        }
    }
}

/// Runs the relay CLI in-process with the same argument handling as the
/// standalone `pumice-relay` binary; a bare `relay` starts the service.
fn relay(args: &[&str]) -> ExitCode {
    pumice_relay::cli::run_with("pumice relay", args.iter().map(OsString::from))
}

/// Runs the `pumice-transcribe` executable that sits next to the running
/// `pumice`, forwarding arguments, stdio and the exit status. PATH and the
/// working directory are never searched (p10-pumice-entry/D2).
fn transcribe(args: &[&str]) -> ExitCode {
    let Some(path) = transcribe_path() else {
        eprintln!("error: cannot locate the running pumice executable");
        return ExitCode::from(1);
    };
    if !path.is_file() {
        let dir = path
            .parent()
            .map(Path::display)
            .map(|dir| dir.to_string())
            .unwrap_or_default();
        eprintln!(
            "error: pumice-transcribe is not installed next to pumice ({dir}); \
             install it with the Pumice transcription archive (see docs/transcription.md) \
             or `cargo install --path crates/transcription --features engine --locked`"
        );
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
    #[cfg(unix)]
    let child_pid = child.id() as i32;
    #[cfg(unix)]
    signals::forward_to_child(child_pid);
    match child.wait() {
        Ok(status) => {
            #[cfg(unix)]
            signals::clear_child();
            exit_code(status)
        }
        Err(error) => {
            #[cfg(unix)]
            signals::clear_child();
            eprintln!("error: cannot wait for {}: {error}", path.display());
            ExitCode::from(1)
        }
    }
}

/// The transcription executable next to the real path of the running
/// `pumice` (canonicalized, so a symlinked `pumice` still resolves its own
/// install directory).
fn transcribe_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let dir = exe.parent()?;
    let name = if cfg!(windows) {
        "pumice-transcribe.exe"
    } else {
        "pumice-transcribe"
    };
    Some(dir.join(name))
}

#[cfg(unix)]
fn exit_code(status: std::process::ExitStatus) -> ExitCode {
    use std::os::unix::process::ExitStatusExt;
    match status.code() {
        Some(code) => ExitCode::from(code as u8),
        // A signal death becomes the conventional 128+signal status.
        None => ExitCode::from(128 + status.signal().unwrap_or(0) as u8),
    }
}

#[cfg(not(unix))]
fn exit_code(status: std::process::ExitStatus) -> ExitCode {
    ExitCode::from(status.code().unwrap_or(1) as u8)
}

/// On Unix, Ctrl-C and SIGTERM reach the whole foreground process group, so
/// the child already gets them; without a handler this process would still
/// die at once and abandon the child mid-shutdown. Forward both signals to
/// the child and keep waiting: the exit status then reports the child's
/// signal death as 128+signal, exactly as if it had been run directly.
#[cfg(unix)]
mod signals {
    use std::sync::atomic::{AtomicI32, Ordering};

    static CHILD_PID: AtomicI32 = AtomicI32::new(0);

    extern "C" fn forward(signal: libc::c_int) {
        let child = CHILD_PID.load(Ordering::SeqCst);
        if child > 0 {
            unsafe {
                libc::kill(child, signal);
            }
        }
    }

    pub fn forward_to_child(pid: i32) {
        CHILD_PID.store(pid, Ordering::SeqCst);
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = forward as *const () as libc::sighandler_t;
        let _ = unsafe { libc::sigemptyset(&mut action.sa_mask) };
        for signal in [libc::SIGINT, libc::SIGTERM] {
            let _ = unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) };
        }
    }

    /// Stops forwarding: the child is reaped, so its pid must never be
    /// signaled again (pid recycling).
    pub fn clear_child() {
        CHILD_PID.store(0, Ordering::SeqCst);
    }
}
