//! Resolves a CLI program to an absolute executable path before spawning.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::providers::{ProviderError, ProviderErrorCode};

use super::ProgramSpec;

/// Maximum shim content read for translation; larger wrappers are refused.
const SHIM_MAX_BYTES: usize = 64 * 1024;

/// A program ready to spawn: the executable plus arguments that must come
/// before the adapter's own (the package entrypoint of a translated shim).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedProgram {
    pub path: PathBuf,
    pub prefix_args: Vec<OsString>,
}

/// Resolves `program` to an absolute executable path before spawning.
///
/// A bare name (one path component) is looked up on `PATH`, skipping
/// relative entries; on Windows each directory is tried with the `PATHEXT`
/// extensions in order. Anything else is a path, made absolute against the
/// current directory. A program that cannot be found is
/// [`ProviderError::NotInstalled`].
///
/// On Windows, `.cmd`, `.bat` and `.ps1` results would run through a shell
/// interpreter, so they are never spawned: a supported npm `.cmd` shim is
/// translated into a direct launch of its runtime (see
/// [`translate_npm_shim`]); anything else is
/// [`ProviderErrorCode::UnsupportedShim`].
pub fn resolve_program(program: &ProgramSpec) -> Result<ResolvedProgram, ProviderError> {
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

    if cfg!(windows) && is_windows_shim(&resolved) {
        return translate_windows_shim(&resolved, program.npm_entrypoint);
    }
    Ok(ResolvedProgram {
        path: resolved,
        prefix_args: Vec::new(),
    })
}

fn unsupported_shim() -> ProviderError {
    ProviderError::other(ProviderErrorCode::UnsupportedShim)
}

/// Handles a resolved `.cmd`/`.bat`/`.ps1` on Windows. Only standard npm
/// `.cmd` shims for the adapter's declared package are translated; every
/// other wrapper is refused, never spawned through a shell.
fn translate_windows_shim(
    shim_path: &Path,
    npm_entrypoint: Option<&'static str>,
) -> Result<ResolvedProgram, ProviderError> {
    let is_cmd = shim_path
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd"));
    let Some(expected) = npm_entrypoint.filter(|_| is_cmd) else {
        return Err(unsupported_shim());
    };
    let contents = read_shim(shim_path)?;
    translate_npm_shim(shim_path, &contents, expected, &resolve_node)
}

/// Reads a shim for translation, bounded to [`SHIM_MAX_BYTES`]. Read failures
/// refuse the wrapper rather than guess at its meaning.
fn read_shim(path: &Path) -> Result<String, ProviderError> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|_| unsupported_shim())?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(SHIM_MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unsupported_shim())?;
    if bytes.len() > SHIM_MAX_BYTES {
        return Err(unsupported_shim());
    }
    String::from_utf8(bytes).map_err(|_| unsupported_shim())
}

/// Translates a recognized npm `.cmd` shim into a direct launch of its
/// JavaScript runtime: `node <entrypoint> <adapter args…>`. Pure string
/// parsing plus an injectable runtime lookup, so it is unit-testable on every
/// OS. Anything outside the recognized format is refused with
/// [`ProviderErrorCode::UnsupportedShim`]; errors never include shim text.
fn translate_npm_shim(
    shim_path: &Path,
    contents: &str,
    expected_entrypoint: &str,
    lookup_node: &dyn Fn(&Path) -> Option<PathBuf>,
) -> Result<ResolvedProgram, ProviderError> {
    let entrypoint =
        extract_shim_entrypoint(contents, expected_entrypoint).ok_or_else(unsupported_shim)?;
    let Some(shim_dir) = shim_path.parent() else {
        return Err(unsupported_shim());
    };
    // `entrypoint` is the package path inside `node_modules`.
    let entrypoint_path = shim_dir.join("node_modules").join(&entrypoint);
    if !entrypoint_path.is_file() {
        return Err(unsupported_shim());
    }
    let Some(node) = lookup_node(shim_dir) else {
        // The shim is a known wrapper but its runtime is missing.
        return Err(ProviderError::NotInstalled);
    };
    Ok(ResolvedProgram {
        path: node,
        prefix_args: vec![entrypoint_path.into_os_string()],
    })
}

/// Extracts the package path from the single quoted
/// `"%dp0%\node_modules\…"` occurrence in a standard npm shim, normalized to
/// forward slashes. Returns `None` for anything else: no such path, more than
/// one (wrapper logic beyond the standard shim), an unquoted occurrence, or a
/// different package layout.
fn extract_shim_entrypoint(contents: &str, expected_entrypoint: &str) -> Option<String> {
    // ASCII lowercasing preserves byte offsets, so matches map back onto
    // `contents`; batch variables such as `%DP0%` are case-insensitive.
    let lower = contents.to_ascii_lowercase();
    let mut occurrence: Option<(usize, usize)> = None;
    let mut search_from = 0;
    while let Some(rel) = lower[search_from..].find("%dp0%") {
        let at = search_from + rel;
        search_from = at + "%dp0%".len();
        // Require `node_modules` on both sides of a path separator; the
        // shim's `"%dp0%\node.exe"` probe must not count as an occurrence.
        let Some(after) = lower[search_from..]
            .strip_prefix('\\')
            .or_else(|| lower[search_from..].strip_prefix('/'))
            .and_then(|rest| rest.strip_prefix("node_modules"))
            .and_then(|rest| rest.strip_prefix('\\').or_else(|| rest.strip_prefix('/')))
        else {
            continue;
        };
        if occurrence.is_some() {
            return None;
        }
        // The path must be quoted and run to the next quote. `after` is the
        // slice right after the `…%dp0%\<sep>node_modules<sep>` prefix, so
        // the package path starts where `after` starts in `lower`.
        if !lower[..at].ends_with('"') {
            return None;
        }
        let start = lower.len() - after.len();
        let end = lower[start..].find('"')? + start;
        occurrence = Some((start, end));
    }
    let (start, end) = occurrence?;
    let package = contents[start..end].replace('\\', "/");
    (package == expected_entrypoint).then_some(package)
}

/// Picks the JavaScript runtime for a translated shim: the `node.exe` bundled
/// next to the shim, otherwise `node` on PATH.
fn resolve_node(shim_dir: &Path) -> Option<PathBuf> {
    let bundled = shim_dir.join("node.exe");
    if bundled.is_file() {
        return Some(bundled);
    }
    node_on_path()
}

/// Looks up `node` on PATH accepting an `.exe` only: a `node.cmd` or
/// `node.bat` would re-enter the shell this translation exists to avoid.
fn node_on_path() -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .filter(|dir| dir.is_absolute())
        .find_map(|dir| find_node_exe(&dir.join("node")))
}

#[cfg(windows)]
fn find_node_exe(candidate: &Path) -> Option<PathBuf> {
    let mut exe = candidate.as_os_str().to_owned();
    exe.push(".exe");
    let exe = PathBuf::from(exe);
    exe.is_file().then_some(exe)
}

#[cfg(not(windows))]
fn find_node_exe(candidate: &Path) -> Option<PathBuf> {
    find_file(candidate)
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

    const CODEX_SHIM: &str = include_str!("../../tests/fixtures/windows-shims/codex.cmd");
    const CLAUDE_SHIM: &str = include_str!("../../tests/fixtures/windows-shims/claude.cmd");
    const HOSTILE_SHIM: &str = include_str!("../../tests/fixtures/windows-shims/hostile.cmd");
    const CODEX_ENTRYPOINT: &str = "@openai/codex/bin/codex.js";
    const CLAUDE_ENTRYPOINT: &str = "@anthropic-ai/claude-code/cli.js";

    /// Writes the entrypoint file a translated shim would launch, in the
    /// shim's directory, and returns the shim path next to it.
    fn shim_with_entrypoint(shim_name: &str, entrypoint: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("node_modules").join(entrypoint);
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::fs::write(&entry, b"// entrypoint").unwrap();
        let shim = dir.path().join(shim_name);
        (dir, shim)
    }

    #[test]
    fn translates_standard_npm_shims() {
        for (shim_name, contents, entrypoint) in [
            ("codex.cmd", CODEX_SHIM, CODEX_ENTRYPOINT),
            ("claude.cmd", CLAUDE_SHIM, CLAUDE_ENTRYPOINT),
        ] {
            let (_dir, shim) = shim_with_entrypoint(shim_name, entrypoint);
            let node = shim.with_file_name("node.exe");
            let resolved = translate_npm_shim(&shim, contents, entrypoint, &|_| Some(node.clone()))
                .expect("translates");
            assert_eq!(resolved.path, node);
            assert_eq!(
                resolved.prefix_args,
                [shim
                    .parent()
                    .unwrap()
                    .join("node_modules")
                    .join(entrypoint)
                    .into_os_string()]
            );
        }
    }

    #[test]
    fn refuses_shim_for_a_different_package() {
        let (_dir, shim) = shim_with_entrypoint("codex.cmd", CLAUDE_ENTRYPOINT);
        assert_eq!(
            translate_npm_shim(&shim, CODEX_SHIM, CLAUDE_ENTRYPOINT, &|_| None),
            Err(ProviderError::other(ProviderErrorCode::UnsupportedShim))
        );
    }

    #[test]
    fn refuses_shim_without_entrypoint_path() {
        let (_dir, shim) = shim_with_entrypoint("codex.cmd", CODEX_ENTRYPOINT);
        let contents = "@ECHO off\r\nGOTO start\r\n";
        assert_eq!(
            translate_npm_shim(&shim, contents, CODEX_ENTRYPOINT, &|_| None),
            Err(ProviderError::other(ProviderErrorCode::UnsupportedShim))
        );
    }

    #[test]
    fn refuses_shim_with_two_entrypoint_paths() {
        let (_dir, shim) = shim_with_entrypoint("codex.cmd", CODEX_ENTRYPOINT);
        assert_eq!(
            translate_npm_shim(&shim, HOSTILE_SHIM, CODEX_ENTRYPOINT, &|_| None),
            Err(ProviderError::other(ProviderErrorCode::UnsupportedShim))
        );
    }

    #[test]
    fn refuses_unquoted_entrypoint_path() {
        let (_dir, shim) = shim_with_entrypoint("codex.cmd", CODEX_ENTRYPOINT);
        let contents = CODEX_SHIM.replace("\"%dp0%\\node_modules\\", "%dp0%\\node_modules\\");
        assert_eq!(
            translate_npm_shim(&shim, &contents, CODEX_ENTRYPOINT, &|_| None),
            Err(ProviderError::other(ProviderErrorCode::UnsupportedShim))
        );
    }

    #[test]
    fn refuses_missing_entrypoint_file() {
        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join("codex.cmd");
        assert_eq!(
            translate_npm_shim(&shim, CODEX_SHIM, CODEX_ENTRYPOINT, &|_| {
                Some(PathBuf::from("node"))
            }),
            Err(ProviderError::other(ProviderErrorCode::UnsupportedShim))
        );
    }

    #[test]
    fn missing_node_runtime_is_not_installed() {
        let (_dir, shim) = shim_with_entrypoint("codex.cmd", CODEX_ENTRYPOINT);
        assert_eq!(
            translate_npm_shim(&shim, CODEX_SHIM, CODEX_ENTRYPOINT, &|_| None),
            Err(ProviderError::NotInstalled)
        );
    }

    #[test]
    fn resolve_node_prefers_bundled_exe() {
        let dir = tempfile::tempdir().unwrap();
        let bundled = dir.path().join("node.exe");
        std::fs::write(&bundled, b"").unwrap();
        assert_eq!(resolve_node(dir.path()), Some(bundled));
    }

    #[test]
    fn oversized_shim_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join("codex.cmd");
        std::fs::write(&shim, vec![b'@'; SHIM_MAX_BYTES + 1]).unwrap();
        assert_eq!(
            read_shim(&shim),
            Err(ProviderError::other(ProviderErrorCode::UnsupportedShim))
        );
    }

    #[test]
    fn windows_shim_translation_entry_gate() {
        let (_dir, cmd) = shim_with_entrypoint("codex.cmd", CODEX_ENTRYPOINT);
        std::fs::write(&cmd, CODEX_SHIM).unwrap();
        let bat = cmd.with_extension("bat");
        std::fs::write(&bat, CODEX_SHIM).unwrap();
        let ps1 = cmd.with_extension("ps1");
        std::fs::write(&ps1, CODEX_SHIM).unwrap();
        let node = cmd.with_file_name("node.exe");
        std::fs::write(&node, b"").unwrap();

        // A supported `.cmd` with its entrypoint and runtime translates.
        let resolved = translate_windows_shim(&cmd, Some(CODEX_ENTRYPOINT)).expect("translates");
        assert_eq!(resolved.path, node);
        assert_eq!(resolved.prefix_args.len(), 1);

        // `.cmd` without a declared entrypoint, and non-`.cmd` wrappers:
        // all refused, none read.
        for (path, entrypoint) in [
            (cmd.clone(), None),
            (bat, Some(CODEX_ENTRYPOINT)),
            (ps1, Some(CODEX_ENTRYPOINT)),
        ] {
            assert_eq!(
                translate_windows_shim(&path, entrypoint),
                Err(ProviderError::other(ProviderErrorCode::UnsupportedShim)),
                "{}",
                path.display()
            );
        }
    }
}
