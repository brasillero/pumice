//! Resolves a CLI program to an absolute executable path before spawning.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::providers::{ProviderError, ProviderErrorCode};

use super::ProgramSpec;

/// Resolves `program` to an absolute path.
///
/// A bare name (one path component) is looked up on `PATH`, skipping
/// relative entries; on Windows each directory is tried with the `PATHEXT`
/// extensions in order. Anything else is a path, made absolute against the
/// current directory. A program that cannot be found is
/// [`ProviderError::NotInstalled`].
pub fn resolve_program(program: &ProgramSpec) -> Result<PathBuf, ProviderError> {
    let binary = program.binary.as_path();
    if binary.as_os_str().is_empty() {
        return Err(ProviderError::other(
            ProviderErrorCode::InvalidConfiguration,
        ));
    }
    let resolved = if is_bare_name(binary) {
        search_path(binary.as_os_str(), std::env::var_os("PATH").as_deref())
    } else {
        let absolute = std::path::absolute(binary)
            .map_err(|_| ProviderError::other(ProviderErrorCode::InvalidConfiguration))?;
        find_file(&absolute)
    }
    .ok_or(ProviderError::NotInstalled)?;

    // TODO(S5.1): translate supported npm `.cmd` shims into a direct launch
    // of the underlying runtime plus `program.npm_entrypoint`. Until then they
    // are refused: spawning them would go through cmd.exe.
    if cfg!(windows) && is_windows_shim(&resolved) {
        return Err(ProviderError::other(ProviderErrorCode::UnsupportedShim));
    }
    Ok(resolved)
}

/// True when `path` is a single normal component such as `claude`.
fn is_bare_name(path: &Path) -> bool {
    let mut components = path.components();
    matches!(
        (components.next(), components.next()),
        (Some(std::path::Component::Normal(_)), None)
    )
}

/// True for script wrappers Windows would run through a shell interpreter.
pub fn is_windows_shim(path: &Path) -> bool {
    path.extension().and_then(OsStr::to_str).is_some_and(|ext| {
        ["cmd", "bat", "ps1"]
            .iter()
            .any(|s| ext.eq_ignore_ascii_case(s))
    })
}

fn search_path(name: &OsStr, path_var: Option<&OsStr>) -> Option<PathBuf> {
    let path_var = path_var?;
    std::env::split_paths(path_var)
        .filter(|dir| dir.is_absolute())
        .find_map(|dir| find_file(&dir.join(name)))
}

/// Returns `candidate` if it is a runnable file. On Windows, also tries the
/// `PATHEXT` extensions appended to it.
#[cfg(not(windows))]
fn find_file(candidate: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::metadata(candidate).ok()?;
    (metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .then(|| candidate.to_path_buf())
}

#[cfg(windows)]
fn find_file(candidate: &Path) -> Option<PathBuf> {
    let has_extension = candidate.extension().is_some();
    let exact = has_extension.then(|| candidate.to_path_buf());
    let with_extensions = pathext().into_iter().map(|ext| {
        let mut name = candidate.as_os_str().to_owned();
        name.push(ext);
        PathBuf::from(name)
    });
    exact
        .into_iter()
        .chain(with_extensions)
        .find(|p| p.is_file())
}

/// Extensions from `PATHEXT`, in order, with the Windows default as fallback.
#[cfg(windows)]
fn pathext() -> Vec<OsString> {
    parse_pathext(std::env::var_os("PATHEXT").as_deref())
}

#[cfg_attr(not(windows), allow(dead_code))]
fn parse_pathext(value: Option<&OsStr>) -> Vec<OsString> {
    let value = value
        .and_then(OsStr::to_str)
        .filter(|v| !v.trim().is_empty())
        .unwrap_or(".COM;.EXE;.BAT;.CMD");
    value
        .split(';')
        .map(str::trim)
        .filter(|ext| ext.starts_with('.') && ext.len() > 1)
        .map(OsString::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_bare_names() {
        assert!(is_bare_name(Path::new("claude")));
        assert!(is_bare_name(Path::new("claude.exe")));
        assert!(!is_bare_name(Path::new("./claude")));
        assert!(!is_bare_name(Path::new("bin/claude")));
        assert!(!is_bare_name(Path::new("/usr/bin/claude")));
    }

    #[test]
    fn recognizes_windows_shims() {
        assert!(is_windows_shim(Path::new("claude.cmd")));
        assert!(is_windows_shim(Path::new("C:/npm/claude.CMD")));
        assert!(is_windows_shim(Path::new("claude.bat")));
        assert!(is_windows_shim(Path::new("claude.ps1")));
        assert!(!is_windows_shim(Path::new("claude.exe")));
        assert!(!is_windows_shim(Path::new("claude")));
    }

    #[test]
    fn parses_pathext_in_order() {
        assert_eq!(
            parse_pathext(Some(OsStr::new(".EXE;.CMD; .ps1 ;;bad"))),
            [".EXE", ".CMD", ".ps1"]
        );
        assert_eq!(parse_pathext(None), [".COM", ".EXE", ".BAT", ".CMD"]);
    }

    #[test]
    fn empty_binary_is_invalid() {
        let spec = ProgramSpec {
            binary: PathBuf::new(),
            npm_entrypoint: None,
        };
        assert_eq!(
            resolve_program(&spec),
            Err(ProviderError::other(
                ProviderErrorCode::InvalidConfiguration
            ))
        );
    }

    #[test]
    fn missing_bare_name_is_not_installed() {
        let spec = ProgramSpec {
            binary: PathBuf::from("pumice-no-such-cli-7f3a9"),
            npm_entrypoint: None,
        };
        assert_eq!(resolve_program(&spec), Err(ProviderError::NotInstalled));
    }

    #[test]
    fn search_skips_relative_entries() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir
            .path()
            .join(format!("tool{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&exe, b"").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let name = OsStr::new("tool");
        let joined = std::env::join_paths([Path::new("relative-dir"), dir.path()]).unwrap();
        let found = search_path(name, Some(&joined)).expect("found on PATH");
        // Compare canonical paths: on Windows the PATHEXT case may differ.
        assert_eq!(
            std::fs::canonicalize(found).unwrap(),
            std::fs::canonicalize(&exe).unwrap()
        );
        let relative_only = std::env::join_paths([Path::new("relative-dir")]).unwrap();
        assert_eq!(search_path(name, Some(&relative_only)), None);
    }
}
