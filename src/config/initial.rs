//! Write the complete starting configuration before Pumice loads settings.
//!
//! Existing entries are never replaced, including malformed files, directories
//! and dangling symlinks. Publishing each completed file without clobbering
//! makes simultaneous first starts safe. Explicit config paths and optional
//! diagnostic reads do not call this helper.

use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const FILES: [(&str, &str); 1] = [("pumice.yml", include_str!("../../pumice.example.yml"))];

#[derive(Debug)]
pub struct InitError {
    path: PathBuf,
    source: io::Error,
}

impl InitError {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot create initial settings: {}", self.source)
    }
}

impl std::error::Error for InitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// Create the starting configuration when the per-user file is absent.
/// Existing installations need no writes. The caller still validates the file.
pub fn initialize(requested: &Path) -> Result<(), InitError> {
    match std::fs::symlink_metadata(requested) {
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(InitError {
                path: requested.to_path_buf(),
                source,
            });
        }
    }
    let directory = requested.parent().expect("per-user config has a directory");
    for (name, text) in FILES {
        let path = directory.join(name);
        create_missing(&path, text).map_err(|source| InitError { path, source })?;
    }
    Ok(())
}

fn create_missing(path: &Path, text: &str) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let directory = path.parent().expect("initial files have a directory");
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(directory)?;
    let mut pending = tempfile::Builder::new()
        .prefix(".pumice-init-")
        .tempfile_in(directory)?;
    pending.write_all(text.as_bytes())?;
    pending.as_file().sync_all()?;
    match pending.persist_noclobber(path) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn creates_complete_examples_and_preserves_existing_settings() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("nested/pumice");
        initialize(&directory.join("pumice.yml")).unwrap();
        for (name, text) in FILES {
            assert_eq!(std::fs::read_to_string(directory.join(name)).unwrap(), text);
        }
        let existing = directory.join("pumice.yml");
        std::fs::write(&existing, "port: 12345\n# intentionally incomplete\n").unwrap();
        initialize(&existing).unwrap();
        assert_eq!(
            std::fs::read_to_string(existing).unwrap(),
            "port: 12345\n# intentionally incomplete\n"
        );
        assert_eq!(std::fs::read_dir(directory).unwrap().count(), FILES.len());
    }

    #[test]
    fn concurrent_starts_publish_only_complete_files() {
        let temp = tempfile::tempdir().unwrap();
        let directory = Arc::new(temp.path().join("pumice"));
        let barrier = Arc::new(Barrier::new(12));
        std::thread::scope(|scope| {
            for _ in 0..12 {
                let directory = directory.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    barrier.wait();
                    initialize(&directory.join("pumice.yml")).unwrap();
                    for (name, text) in FILES {
                        assert_eq!(std::fs::read_to_string(directory.join(name)).unwrap(), text);
                    }
                });
            }
        });
        assert_eq!(std::fs::read_dir(&*directory).unwrap().count(), FILES.len());
    }

    #[test]
    fn reports_unwritable_destination_without_changing_it() {
        let temp = tempfile::tempdir().unwrap();
        let blocked = temp.path().join("blocked");
        std::fs::write(&blocked, "keep").unwrap();
        let error = initialize(&blocked.join("pumice/pumice.yml")).unwrap_err();
        assert!(error.path().starts_with(&blocked));
        assert!(error.to_string().contains("cannot create initial settings"));
        assert_eq!(std::fs::read_to_string(blocked).unwrap(), "keep");
    }

    #[cfg(unix)]
    #[test]
    fn keeps_symlinks_and_creates_private_files_and_directories() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("pumice");
        initialize(&directory.join("pumice.yml")).unwrap();
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let relay = directory.join("pumice.yml");
        assert_eq!(
            std::fs::metadata(&relay).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::remove_file(&relay).unwrap();
        let target = temp.path().join("absent.yml");
        symlink(&target, &relay).unwrap();
        initialize(&relay).unwrap();
        assert_eq!(std::fs::read_link(relay).unwrap(), target);
        assert!(!target.exists());
    }

    #[test]
    fn an_existing_config_requires_no_writes() {
        let temp = tempfile::tempdir().unwrap();
        let relay = temp.path().join("pumice.yml");
        std::fs::write(&relay, "custom settings").unwrap();
        initialize(&relay).unwrap();
        assert_eq!(std::fs::read_to_string(relay).unwrap(), "custom settings");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }
}
