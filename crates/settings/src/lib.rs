//! Write complete starting files before modules load per-user settings.
//!
//! Existing entries are never replaced, including malformed files, directories
//! and dangling symlinks. Publishing each completed file without clobbering
//! makes simultaneous first starts safe. Explicit config paths and optional
//! diagnostic reads do not call this helper.

use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

// Keep the shipped examples and the first-start files identical. The sample
// definition is published first, before the formatter config references it.
const FILES: [(&str, &str); 5] = [
    (
        "example-formatter.clean.yml",
        include_str!("../../../example-formatter.clean.yml"),
    ),
    ("relay.yml", include_str!("../../../relay.example.yml")),
    (
        "transcribe.yml",
        include_str!("../../../transcribe.example.yml"),
    ),
    (
        "formatter.yml",
        include_str!("../../../formatter.example.yml"),
    ),
    (
        "orchestrator.yml",
        include_str!("../../../orchestrator.example.yml"),
    ),
];

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

/// Create the starting set when the requested per-user file is absent.
/// Existing installations need no writes, even if other module files are absent.
/// Callers still load and validate their own file after this returns.
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
        initialize(&directory.join("relay.yml")).unwrap();
        for (name, text) in FILES {
            assert_eq!(std::fs::read_to_string(directory.join(name)).unwrap(), text);
        }
        let existing = directory.join("relay.yml");
        std::fs::write(&existing, "port: 12345\n# intentionally incomplete\n").unwrap();
        std::fs::remove_file(directory.join("transcribe.yml")).unwrap();
        initialize(&directory.join("transcribe.yml")).unwrap();
        assert_eq!(
            std::fs::read_to_string(existing).unwrap(),
            "port: 12345\n# intentionally incomplete\n"
        );
        assert!(directory.join("transcribe.yml").is_file());
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
                    initialize(&directory.join("orchestrator.yml")).unwrap();
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
        let error = initialize(&blocked.join("pumice/relay.yml")).unwrap_err();
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
        initialize(&directory.join("relay.yml")).unwrap();
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let relay = directory.join("relay.yml");
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
    fn an_existing_module_config_requires_no_writes() {
        let temp = tempfile::tempdir().unwrap();
        let relay = temp.path().join("relay.yml");
        std::fs::write(&relay, "custom settings").unwrap();
        initialize(&relay).unwrap();
        assert_eq!(std::fs::read_to_string(relay).unwrap(), "custom settings");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }
}
