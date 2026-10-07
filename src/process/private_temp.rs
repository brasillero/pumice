//! Private temporary directory helpers.
//!
//! On Unix every directory Pumice creates is 0700 and every file it writes is
//! 0600, so other users cannot read or tamper with control files. On Windows
//! the per-user %TEMP% is already private, so we keep the standard behavior.

#[cfg(unix)]
mod unix {
    use std::fs::{DirBuilder, OpenOptions, Permissions};
    use std::io::{self, Write};
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    use std::path::Path;

    const DIR_MODE: u32 = 0o700;
    const FILE_MODE: u32 = 0o600;

    pub fn tempdir() -> io::Result<tempfile::TempDir> {
        let permissions = Permissions::from_mode(DIR_MODE);
        tempfile::Builder::new()
            .prefix("pumice-")
            .permissions(permissions)
            .tempdir()
    }

    pub fn create_dir(path: &Path) -> io::Result<()> {
        DirBuilder::new().mode(DIR_MODE).create(path)
    }

    pub fn create_dir_all(path: &Path) -> io::Result<()> {
        DirBuilder::new()
            .recursive(true)
            .mode(DIR_MODE)
            .create(path)
    }

    pub fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(FILE_MODE)
            .open(path)?;
        file.write_all(contents)?;
        file.flush()?;
        Ok(())
    }
}

#[cfg(not(unix))]
mod other {
    use std::fs;
    use std::io;
    use std::path::Path;

    pub fn tempdir() -> io::Result<tempfile::TempDir> {
        // Per-user %TEMP% is already private on Windows; keep standard behavior.
        tempfile::Builder::new().prefix("pumice-").tempdir()
    }

    pub fn create_dir(path: &Path) -> io::Result<()> {
        // Per-user %TEMP% is already private on Windows.
        fs::create_dir(path)
    }

    pub fn create_dir_all(path: &Path) -> io::Result<()> {
        // Per-user %TEMP% is already private on Windows.
        fs::create_dir_all(path)
    }

    pub fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
        // Per-user %TEMP% is already private on Windows.
        fs::write(path, contents)
    }
}

#[cfg(unix)]
pub use unix::*;

#[cfg(not(unix))]
pub use other::*;
