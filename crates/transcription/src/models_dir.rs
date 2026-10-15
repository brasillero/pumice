//! Default per-user models directory (task `p9-transcribe-downloader`,
//! decision D2).
//!
//! Downloaded catalog models live outside the installation and outside the
//! configuration file:
//!
//! - Linux: `$XDG_DATA_HOME/pumice/models`, else `~/.local/share/pumice/models`
//! - macOS: absolute `$XDG_DATA_HOME/pumice/models` when set, else
//!   `~/Library/Application Support/pumice/models`
//! - Windows: `%LOCALAPPDATA%\pumice\models`
//!
//! `transcribe.yml` can override the location with `models_dir`.

use std::ffi::OsString;
use std::path::PathBuf;

/// Resolves the default per-user models directory from the environment.
///
/// Returns `None` when the platform conventions give no base directory (for
/// example no `HOME` and no `XDG_DATA_HOME`); callers then require an
/// explicit `models_dir` in `transcribe.yml`.
pub fn default_models_dir(get_env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    default_models_dir_for(get_env, cfg!(windows), cfg!(target_os = "macos"))
}

fn default_models_dir_for(
    get_env: &dyn Fn(&str) -> Option<OsString>,
    windows: bool,
    macos: bool,
) -> Option<PathBuf> {
    let base = if windows {
        get_env("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    } else {
        match get_env("XDG_DATA_HOME") {
            Some(value) if !value.is_empty() => {
                let path = PathBuf::from(&value);
                if path.is_absolute() {
                    Some(path)
                } else {
                    home_data_dir(get_env, macos)
                }
            }
            _ => home_data_dir(get_env, macos),
        }
    }?;
    Some(base.join("pumice").join("models"))
}

fn home_data_dir(get_env: &dyn Fn(&str) -> Option<OsString>, macos: bool) -> Option<PathBuf> {
    let home = PathBuf::from(get_env("HOME")?);
    if macos {
        Some(home.join("Library").join("Application Support"))
    } else {
        Some(home.join(".local").join("share"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let pairs: Vec<(String, OsString)> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), OsString::from(value)))
            .collect();
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, value)| value.clone())
        }
    }

    #[test]
    fn linux_honours_absolute_xdg_data_home() {
        let dir = default_models_dir_for(&env_of(&[("XDG_DATA_HOME", "/tmp/xdg")]), false, false);
        assert_eq!(dir, Some(PathBuf::from("/tmp/xdg/pumice/models")));
    }

    #[test]
    fn linux_falls_back_to_local_share() {
        let dir = default_models_dir_for(&env_of(&[("HOME", "/tmp/home")]), false, false);
        assert_eq!(
            dir,
            Some(PathBuf::from("/tmp/home/.local/share/pumice/models"))
        );
    }

    #[test]
    fn linux_ignores_relative_xdg_data_home() {
        let dir = default_models_dir_for(
            &env_of(&[("XDG_DATA_HOME", "relative/dir"), ("HOME", "/tmp/home")]),
            false,
            false,
        );
        assert_eq!(
            dir,
            Some(PathBuf::from("/tmp/home/.local/share/pumice/models"))
        );
    }

    #[test]
    fn macos_uses_application_support_without_xdg() {
        let dir = default_models_dir_for(&env_of(&[("HOME", "/tmp/home")]), false, true);
        assert_eq!(
            dir,
            Some(
                PathBuf::from("/tmp/home")
                    .join("Library")
                    .join("Application Support")
                    .join("pumice")
                    .join("models")
            )
        );
    }

    #[test]
    fn macos_honours_absolute_xdg_data_home() {
        let dir = default_models_dir_for(
            &env_of(&[("XDG_DATA_HOME", "/tmp/xdg"), ("HOME", "/tmp/home")]),
            false,
            true,
        );
        assert_eq!(dir, Some(PathBuf::from("/tmp/xdg/pumice/models")));
    }

    #[test]
    fn windows_uses_localappdata() {
        let dir = default_models_dir_for(
            &env_of(&[("LOCALAPPDATA", r"C:\Users\me\AppData\Local")]),
            true,
            false,
        );
        assert_eq!(
            dir,
            Some(
                PathBuf::from(r"C:\Users\me\AppData\Local")
                    .join("pumice")
                    .join("models")
            )
        );
    }

    #[test]
    fn no_env_gives_none() {
        assert_eq!(default_models_dir_for(&env_of(&[]), false, false), None);
        assert_eq!(default_models_dir_for(&env_of(&[]), true, false), None);
    }
}
