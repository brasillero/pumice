//! Test-only stand-in for the `pumice-transcribe` executable. The dispatcher
//! tests copy this binary next to a copy of `pumice` under the name
//! `pumice-transcribe` and verify the dispatch contract (argument pass-through,
//! exit status propagation, install-relative lookup).
//!
//! Usage: pumice-test-transcribe [EXIT_CODE] [ARGS…]
//!
//! Prints every argument after EXIT_CODE on its own line to stdout and exits
//! with EXIT_CODE (default 0 when absent or unparsable). EXIT_CODE is a full
//! `i32` and goes through `process::exit`, so Windows tests can exercise exit
//! statuses above 255.
//!
//! Unix only, when the PUMICE_STUB_PID_FILE environment variable is set: the
//! stub writes its own PID to that path and waits forever, so a test can
//! prove the dispatcher exec'd it (its PID is `pumice`'s) and deliver a
//! signal directly to it.

use std::ffi::OsStr;
use std::io::Write;
use std::process::exit;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let code: i32 = args
        .next()
        .and_then(|arg| arg.into_string().ok())
        .and_then(|arg| arg.parse().ok())
        .unwrap_or(0);

    #[cfg(unix)]
    if let Some(pid_file) = std::env::var_os("PUMICE_STUB_PID_FILE") {
        std::fs::write(pid_file, format!("{}\n", std::process::id()))
            .expect("write the stub pid file");
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }

    let mut stdout = std::io::stdout().lock();
    for arg in args {
        write_arg(&mut stdout, &arg);
        stdout.write_all(b"\n").expect("write argument line");
    }
    drop(stdout);
    exit(code);
}

/// Writes the argument exactly as it arrived, byte for byte where the
/// platform exposes the raw bytes; a lossy conversion would hide a
/// dispatcher bug that mangles non-UTF-8 arguments.
#[cfg(unix)]
fn write_arg(stdout: &mut impl Write, arg: &OsStr) {
    use std::os::unix::ffi::OsStrExt;
    stdout.write_all(arg.as_bytes()).expect("write argument");
}

#[cfg(not(unix))]
fn write_arg(stdout: &mut impl Write, arg: &OsStr) {
    stdout
        .write_all(arg.to_string_lossy().as_bytes())
        .expect("write argument");
}
