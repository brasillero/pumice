//! Places test executables without a write descriptor in the test process.
//!
//! On Linux, running a file fails with "text file busy" (ETXTBSY) while any
//! process holds it open for writing. A test that writes an executable itself
//! can leak that descriptor into a child another test thread is spawning at
//! the same moment (between its fork and exec), so the first run of the file
//! fails. Every executable is therefore a hard link or, where hard links are
//! unavailable, a copy written by a `cp` child, never by this process.
//!
//! Shared by the relay's tests (`support/mod.rs`) and the workspace root's
//! tests (`#[path]`), the same way the root builds `support/fake_cli.rs`.

use std::fs;
use std::path::Path;

/// Places the executable `src` at `dest`: a hard link, else
/// [`copy_executable`]. Keep `dest` under Cargo's target directory
/// (`CARGO_TARGET_TMPDIR`), on the built binaries' file system, so the link
/// works.
pub fn install_executable(src: &Path, dest: &Path) {
    if fs::hard_link(src, dest).is_err() {
        copy_executable(src, dest);
    }
}

/// Copies the executable `src` to `dest`. On Unix the copy is written by a
/// `cp` child, so this process never holds the new file open for writing;
/// Windows has no ETXTBSY and copies directly.
pub fn copy_executable(src: &Path, dest: &Path) {
    #[cfg(unix)]
    {
        let copied = std::process::Command::new("cp")
            .arg("--")
            .arg(src)
            .arg(dest)
            .status()
            .expect("run cp");
        assert!(
            copied.success(),
            "cp {} {}: {copied}",
            src.display(),
            dest.display()
        );
    }
    #[cfg(not(unix))]
    {
        fs::copy(src, dest).expect("copy executable");
    }
}
