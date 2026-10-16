//! The manifest of a staged transcription bundle, `bundle.json` next to the
//! executable (p14.1-windows-vulkan, design §9). The stager lists every file
//! it staged with its role, size and SHA-256; before anything is registered
//! with ggml, startup checks the native libraries it is about to rely on
//! against it. A damaged or mixed-version install then fails as an
//! installation error instead of quietly running without its GPU module.
//!
//! Written by `scripts/build-windows-vulkan.ps1` and
//! `scripts/build-linux-vulkan.sh`; startup requires it on Windows and in
//! every `vulkan` build. The plain Linux `cpu-variants` stage has none; there
//! it is checked only when present. The manifest guards against corruption
//! and mismatched files, not against an attacker who can write to the
//! install directory: that directory is trusted, as it already is for the
//! modules themselves.

// Only the dynamic-backends loader calls into this module; the tests run in
// every build.
#![cfg_attr(not(pumice_dynamic_backends), allow(dead_code))]

use serde::Deserialize;
use std::collections::HashSet;
use std::fmt;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

/// The manifest's file name, next to the executable.
pub const FILE_NAME: &str = "bundle.json";

const SCHEMA_VERSION: u32 = 1;
const PRODUCT: &str = "pumice-transcribe";

/// Which build a manifest must describe.
#[derive(Debug, Clone, Copy)]
pub struct Expected<'a> {
    pub version: &'a str,
    pub target: &'a str,
}

impl Expected<'static> {
    /// This executable's own version and target triple.
    pub fn this_build() -> Self {
        Expected {
            version: env!("CARGO_PKG_VERSION"),
            target: env!("PUMICE_BUILD_TARGET"),
        }
    }
}

/// What a staged file is for. Only `core`, `cpu` and `gpu` files are ever
/// checked at startup; the running executable is listed for the archive's
/// sake only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// `pumice-transcribe(.exe)`, at the bundle root.
    Executable,
    /// An engine library the executable imports, at the bundle root.
    Core,
    /// `cpu/<variant>/<module>`.
    Cpu,
    /// `gpu/<backend>/<module>`.
    Gpu,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    schema_version: u32,
    product: String,
    version: String,
    target: String,
    // Informational: the native build that produced the files.
    #[allow(dead_code)]
    features: Vec<String>,
    #[allow(dead_code)]
    engine: String,
    files: Vec<Entry>,
}

/// One staged file. `path` is relative to the bundle root, with `/`
/// separators.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    path: String,
    role: Role,
    size: u64,
    sha256: String,
}

/// A parsed manifest whose identity matches this build and whose entries
/// are well-formed.
#[derive(Debug)]
pub struct Manifest {
    files: Vec<Entry>,
}

/// How much a verification read and hashed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Verified {
    pub files: usize,
    pub bytes: u64,
}

impl std::ops::AddAssign for Verified {
    fn add_assign(&mut self, other: Verified) {
        self.files += other.files;
        self.bytes += other.bytes;
    }
}

/// Why the bundle does not match its manifest, in fixed text safe for
/// stderr: bundle-relative paths and sizes only, never file contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleError {
    /// This target requires a manifest and there is none.
    Missing,
    Unreadable,
    /// Not JSON of the manifest's shape (unknown or missing fields too).
    Malformed {
        line: usize,
        column: usize,
    },
    /// Well-formed JSON with an unusable entry.
    Invalid(&'static str),
    /// Written for another product, version, target or schema.
    WrongBuild {
        field: &'static str,
    },
    FileMissing {
        path: String,
    },
    /// On disk in a directory ggml scans, but not in the manifest.
    NotListed {
        path: String,
    },
    SizeMismatch {
        path: String,
        expected: u64,
        actual: u64,
    },
    HashMismatch {
        path: String,
    },
    FileUnreadable {
        path: String,
    },
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BundleError::Missing => write!(f, "{FILE_NAME} is missing"),
            BundleError::Unreadable => write!(f, "{FILE_NAME} cannot be read"),
            BundleError::Malformed { line, column } => write!(
                f,
                "{FILE_NAME} is not a valid manifest (line {line}, column {column})"
            ),
            BundleError::Invalid(reason) => {
                write!(f, "{FILE_NAME} is not a valid manifest ({reason})")
            }
            BundleError::WrongBuild { field } => write!(
                f,
                "{FILE_NAME} was written for another build ({field} differs)"
            ),
            BundleError::FileMissing { path } => {
                write!(f, "{path} is listed in {FILE_NAME} but missing")
            }
            BundleError::NotListed { path } => write!(
                f,
                "{path} is not listed in {FILE_NAME} (a directory ggml scans may hold only \
                 listed files)"
            ),
            BundleError::SizeMismatch {
                path,
                expected,
                actual,
            } => write!(f, "{path} is {actual} bytes; {FILE_NAME} lists {expected}"),
            BundleError::HashMismatch { path } => {
                write!(f, "{path} does not match its SHA-256 in {FILE_NAME}")
            }
            BundleError::FileUnreadable { path } => write!(f, "{path} cannot be read"),
        }
    }
}

impl Manifest {
    /// Reads `<bundle_dir>/bundle.json`. `Ok(None)` when there is none; the
    /// caller decides whether that is allowed.
    pub fn load(bundle_dir: &Path, expected: Expected<'_>) -> Result<Option<Self>, BundleError> {
        match std::fs::read(bundle_dir.join(FILE_NAME)) {
            Ok(bytes) => Self::parse(&bytes, expected).map(Some),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(BundleError::Unreadable),
        }
    }

    pub fn parse(bytes: &[u8], expected: Expected<'_>) -> Result<Self, BundleError> {
        let raw: RawManifest =
            serde_json::from_slice(bytes).map_err(|error| BundleError::Malformed {
                line: error.line(),
                column: error.column(),
            })?;
        let identity = [
            ("schema_version", raw.schema_version == SCHEMA_VERSION),
            ("product", raw.product == PRODUCT),
            ("version", raw.version == expected.version),
            ("target", raw.target == expected.target),
        ];
        if let Some((field, _)) = identity.iter().find(|(_, matches)| !matches) {
            return Err(BundleError::WrongBuild { field });
        }
        let mut seen = HashSet::new();
        for entry in &raw.files {
            validate_entry(entry)?;
            // Windows file names are case-insensitive.
            if !seen.insert(entry.path.to_ascii_lowercase()) {
                return Err(BundleError::Invalid("duplicate path"));
            }
        }
        if !raw.files.iter().any(|entry| entry.role == Role::Core) {
            return Err(BundleError::Invalid("no core library listed"));
        }
        Ok(Manifest { files: raw.files })
    }

    /// Checks every `core` library: all must be present and match.
    pub fn verify_core(&self, bundle_dir: &Path) -> Result<Verified, BundleError> {
        let mut verified = Verified::default();
        for entry in self.files.iter().filter(|entry| entry.role == Role::Core) {
            verified += verify_entry(bundle_dir, entry)?;
        }
        Ok(verified)
    }

    /// Checks a module directory ggml is about to scan (`dir` relative, `/`
    /// separators, e.g. `cpu/haswell`): it must hold exactly the files the
    /// manifest lists under it, each matching. ggml loads every file in a
    /// scanned directory whose name looks like a backend module, so an
    /// unlisted file there would run unchecked; elsewhere in the bundle
    /// unlisted files are ignored, since nothing loads them.
    pub fn verify_dir(&self, bundle_dir: &Path, dir: &str) -> Result<Verified, BundleError> {
        let prefix = format!("{dir}/");
        let listed: Vec<&Entry> = self
            .files
            .iter()
            .filter(|entry| {
                matches!(entry.role, Role::Cpu | Role::Gpu)
                    && entry.path.starts_with(&prefix)
                    && !entry.path[prefix.len()..].contains('/')
            })
            .collect();
        let read_dir = std::fs::read_dir(join_relative(bundle_dir, dir)).map_err(|_| {
            BundleError::FileUnreadable {
                path: dir.to_string(),
            }
        })?;
        for item in read_dir {
            let item = item.map_err(|_| BundleError::FileUnreadable {
                path: dir.to_string(),
            })?;
            let name = item.file_name();
            let known = name.to_str().is_some_and(|name| {
                listed
                    .iter()
                    .any(|entry| &entry.path[prefix.len()..] == name)
            });
            if !known {
                return Err(BundleError::NotListed {
                    path: format!("{prefix}{}", name.to_string_lossy()),
                });
            }
        }
        let mut verified = Verified::default();
        for entry in listed {
            verified += verify_entry(bundle_dir, entry)?;
        }
        Ok(verified)
    }
}

/// Rejects paths that could leave the bundle or name the same file twice,
/// and roles in the wrong place.
fn validate_entry(entry: &Entry) -> Result<(), BundleError> {
    let path = entry.path.as_str();
    let components: Vec<&str> = path.split('/').collect();
    let safe = !path.is_empty()
        && !path.contains(['\\', ':'])
        && components
            .iter()
            .all(|part| !part.is_empty() && *part != "." && *part != "..");
    if !safe {
        return Err(BundleError::Invalid("unsafe path"));
    }
    let placed = match entry.role {
        Role::Executable | Role::Core => components.len() == 1,
        Role::Cpu => components.len() == 3 && components[0] == "cpu",
        Role::Gpu => components.len() == 3 && components[0] == "gpu",
    };
    if !placed {
        return Err(BundleError::Invalid("role does not match path"));
    }
    let hex = entry.sha256.len() == 64
        && entry
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !hex {
        return Err(BundleError::Invalid(
            "sha256 is not 64 lowercase hex digits",
        ));
    }
    Ok(())
}

fn join_relative(bundle_dir: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(bundle_dir.to_path_buf(), |path, part| path.join(part))
}

/// Size first (cheap), then the SHA-256 of the whole file.
fn verify_entry(bundle_dir: &Path, entry: &Entry) -> Result<Verified, BundleError> {
    let path = join_relative(bundle_dir, &entry.path);
    let missing = || BundleError::FileMissing {
        path: entry.path.clone(),
    };
    let unreadable = || BundleError::FileUnreadable {
        path: entry.path.clone(),
    };
    let meta = match std::fs::metadata(&path) {
        Ok(meta) if meta.is_file() => meta,
        Ok(_) => return Err(missing()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(missing()),
        Err(_) => return Err(unreadable()),
    };
    if meta.len() != entry.size {
        return Err(BundleError::SizeMismatch {
            path: entry.path.clone(),
            expected: entry.size,
            actual: meta.len(),
        });
    }
    let mut file = File::open(&path).map_err(|_| unreadable())?;
    let digest = crate::download::sha256_reader(&mut file).map_err(|_| unreadable())?;
    if digest != entry.sha256 {
        return Err(BundleError::HashMismatch {
            path: entry.path.clone(),
        });
    }
    Ok(Verified {
        files: 1,
        bytes: entry.size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const EXPECTED: Expected<'static> = Expected {
        version: "1.2.3",
        target: "x86_64-pc-windows-msvc",
    };

    fn sha256(bytes: &[u8]) -> String {
        crate::download::sha256_reader(&mut &bytes[..]).unwrap()
    }

    /// A staged bundle: `(relative path, role, contents)` files on disk
    /// plus the manifest listing them, as the stager writes it.
    fn stage(files: &[(&str, &str, &[u8])]) -> (tempfile::TempDir, serde_json::Value) {
        let temp = tempfile::tempdir().unwrap();
        let mut entries = Vec::new();
        for (path, role, contents) in files {
            let file = join_relative(temp.path(), path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, contents).unwrap();
            entries.push(json!({
                "path": path,
                "role": role,
                "size": contents.len(),
                "sha256": sha256(contents),
            }));
        }
        let manifest = json!({
            "schema_version": 1,
            "product": "pumice-transcribe",
            "version": "1.2.3",
            "target": "x86_64-pc-windows-msvc",
            "features": ["vulkan"],
            "engine": "transcribe-cpp-sys 0.3.1",
            "files": entries,
        });
        (temp, manifest)
    }

    const BUNDLE: &[(&str, &str, &[u8])] = &[
        ("pumice-transcribe.exe", "executable", b"exe"),
        ("transcribe.dll", "core", b"transcribe"),
        ("ggml-base.dll", "core", b"ggml-base"),
        ("cpu/x64/ggml-cpu-x64.dll", "cpu", b"cpu x64"),
        ("cpu/haswell/ggml-cpu-haswell.dll", "cpu", b"cpu haswell"),
        ("gpu/vulkan/ggml-vulkan.dll", "gpu", b"vulkan module"),
    ];

    fn parse(manifest: &serde_json::Value) -> Result<Manifest, BundleError> {
        Manifest::parse(manifest.to_string().as_bytes(), EXPECTED)
    }

    #[test]
    fn a_staged_bundle_verifies() {
        let (temp, manifest) = stage(BUNDLE);
        let manifest = parse(&manifest).unwrap();
        assert_eq!(
            manifest.verify_core(temp.path()),
            Ok(Verified {
                files: 2,
                bytes: 19
            })
        );
        assert_eq!(
            manifest.verify_dir(temp.path(), "cpu/haswell"),
            Ok(Verified {
                files: 1,
                bytes: 11
            })
        );
        assert_eq!(
            manifest.verify_dir(temp.path(), "gpu/vulkan"),
            Ok(Verified {
                files: 1,
                bytes: 13
            })
        );
    }

    #[test]
    fn load_without_a_manifest_is_none() {
        let temp = tempfile::tempdir().unwrap();
        assert!(Manifest::load(temp.path(), EXPECTED).unwrap().is_none());
    }

    #[test]
    fn load_reads_the_manifest_next_to_the_executable() {
        let (temp, manifest) = stage(BUNDLE);
        std::fs::write(temp.path().join(FILE_NAME), manifest.to_string()).unwrap();
        let manifest = Manifest::load(temp.path(), EXPECTED).unwrap().unwrap();
        assert!(manifest.verify_core(temp.path()).is_ok());
    }

    #[test]
    fn corrupt_bytes_of_the_same_size_fail_the_hash() {
        let (temp, manifest) = stage(BUNDLE);
        let manifest = parse(&manifest).unwrap();
        std::fs::write(
            temp.path()
                .join("gpu")
                .join("vulkan")
                .join("ggml-vulkan.dll"),
            b"vulkan modulX",
        )
        .unwrap();
        assert_eq!(
            manifest.verify_dir(temp.path(), "gpu/vulkan"),
            Err(BundleError::HashMismatch {
                path: "gpu/vulkan/ggml-vulkan.dll".into()
            })
        );
        // The CPU directory is unaffected.
        assert!(manifest.verify_dir(temp.path(), "cpu/x64").is_ok());
    }

    #[test]
    fn a_wrong_size_fails_before_hashing() {
        let (temp, manifest) = stage(BUNDLE);
        let manifest = parse(&manifest).unwrap();
        std::fs::write(temp.path().join("transcribe.dll"), b"transcribe!").unwrap();
        assert_eq!(
            manifest.verify_core(temp.path()),
            Err(BundleError::SizeMismatch {
                path: "transcribe.dll".into(),
                expected: 10,
                actual: 11,
            })
        );
    }

    #[test]
    fn a_missing_core_library_fails() {
        let (temp, manifest) = stage(BUNDLE);
        let manifest = parse(&manifest).unwrap();
        std::fs::remove_file(temp.path().join("ggml-base.dll")).unwrap();
        assert_eq!(
            manifest.verify_core(temp.path()),
            Err(BundleError::FileMissing {
                path: "ggml-base.dll".into()
            })
        );
    }

    #[test]
    fn a_present_module_without_an_entry_fails() {
        // The GPU module is staged but the manifest predates it.
        let (temp, mut manifest) = stage(BUNDLE);
        manifest["files"]
            .as_array_mut()
            .unwrap()
            .retain(|entry| entry["role"] != "gpu");
        let manifest = parse(&manifest).unwrap();
        assert_eq!(
            manifest.verify_dir(temp.path(), "gpu/vulkan"),
            Err(BundleError::NotListed {
                path: "gpu/vulkan/ggml-vulkan.dll".into()
            })
        );
    }

    #[test]
    fn an_unlisted_file_in_a_scanned_dir_fails() {
        let (temp, manifest) = stage(BUNDLE);
        let manifest = parse(&manifest).unwrap();
        let dir = temp.path().join("gpu").join("vulkan");
        std::fs::write(dir.join("ggml-vulkan-extra.dll"), b"").unwrap();
        assert_eq!(
            manifest.verify_dir(temp.path(), "gpu/vulkan"),
            Err(BundleError::NotListed {
                path: "gpu/vulkan/ggml-vulkan-extra.dll".into()
            })
        );
        // A subdirectory counts too: nothing unlisted at all.
        std::fs::remove_file(dir.join("ggml-vulkan-extra.dll")).unwrap();
        std::fs::create_dir(dir.join("extra")).unwrap();
        assert!(matches!(
            manifest.verify_dir(temp.path(), "gpu/vulkan"),
            Err(BundleError::NotListed { .. })
        ));
    }

    #[test]
    fn unlisted_files_outside_scanned_dirs_are_ignored() {
        let (temp, manifest) = stage(BUNDLE);
        let manifest = parse(&manifest).unwrap();
        std::fs::write(temp.path().join("transcribe.yml"), b"").unwrap();
        std::fs::write(temp.path().join("notes.dll"), b"").unwrap();
        std::fs::create_dir_all(temp.path().join("cpu").join("stray")).unwrap();
        assert!(manifest.verify_core(temp.path()).is_ok());
        assert!(manifest.verify_dir(temp.path(), "cpu/x64").is_ok());
    }

    #[test]
    fn a_listed_module_missing_from_its_scanned_dir_fails() {
        let (temp, mut manifest) = stage(BUNDLE);
        manifest["files"].as_array_mut().unwrap().push(json!({
            "path": "cpu/x64/ggml-blas.dll",
            "role": "cpu",
            "size": 0,
            "sha256": sha256(b""),
        }));
        let manifest = parse(&manifest).unwrap();
        assert_eq!(
            manifest.verify_dir(temp.path(), "cpu/x64"),
            Err(BundleError::FileMissing {
                path: "cpu/x64/ggml-blas.dll".into()
            })
        );
    }

    #[test]
    fn the_running_executable_is_never_hashed() {
        let (temp, manifest) = stage(BUNDLE);
        let manifest = parse(&manifest).unwrap();
        std::fs::write(temp.path().join("pumice-transcribe.exe"), b"rebuilt").unwrap();
        assert!(manifest.verify_core(temp.path()).is_ok());
    }

    #[test]
    fn identity_must_match_this_build() {
        let (_temp, manifest) = stage(BUNDLE);
        for (field, value) in [
            ("schema_version", json!(2)),
            ("product", json!("pumice")),
            ("version", json!("1.2.4")),
            ("target", json!("x86_64-unknown-linux-gnu")),
        ] {
            let mut other = manifest.clone();
            other[field] = value;
            assert_eq!(
                parse(&other).unwrap_err(),
                BundleError::WrongBuild { field },
                "{field}"
            );
        }
    }

    #[test]
    fn malformed_json_and_unknown_fields_are_rejected() {
        assert!(matches!(
            Manifest::parse(b"{\"schema_version\": 1,", EXPECTED),
            Err(BundleError::Malformed { line: 1, .. })
        ));
        let (_temp, mut manifest) = stage(BUNDLE);
        manifest["files"][0]["mode"] = json!("0755");
        assert!(matches!(
            parse(&manifest),
            Err(BundleError::Malformed { .. })
        ));
        let (_temp, mut manifest) = stage(BUNDLE);
        manifest.as_object_mut().unwrap().remove("files");
        assert!(matches!(
            parse(&manifest),
            Err(BundleError::Malformed { .. })
        ));
    }

    #[test]
    fn unsafe_or_misplaced_entries_are_rejected() {
        let cases: [(&str, &str, &str); 11] = [
            ("../ggml.dll", "core", "unsafe path"),
            ("/ggml.dll", "core", "unsafe path"),
            ("cpu\\x64\\ggml-cpu-x64.dll", "cpu", "unsafe path"),
            ("C:/ggml.dll", "core", "unsafe path"),
            ("cpu/./ggml-cpu-x64.dll", "cpu", "unsafe path"),
            ("cpu//ggml-cpu-x64.dll", "cpu", "unsafe path"),
            ("", "core", "unsafe path"),
            ("lib/ggml.dll", "core", "role does not match path"),
            ("cpu/ggml-cpu-x64.dll", "cpu", "role does not match path"),
            (
                "gpu/vulkan/ggml-vulkan.dll",
                "cpu",
                "role does not match path",
            ),
            (
                "cpu/x64/ggml-cpu-x64.dll",
                "gpu",
                "role does not match path",
            ),
        ];
        for (path, role, reason) in cases {
            let (_temp, mut manifest) = stage(BUNDLE);
            manifest["files"].as_array_mut().unwrap().push(json!({
                "path": path,
                "role": role,
                "size": 0,
                "sha256": sha256(b""),
            }));
            assert_eq!(
                parse(&manifest).unwrap_err(),
                BundleError::Invalid(reason),
                "{path}"
            );
        }
    }

    #[test]
    fn duplicate_paths_and_bad_hashes_are_rejected() {
        let (_temp, mut manifest) = stage(BUNDLE);
        let mut duplicate = manifest["files"][1].clone();
        duplicate["path"] = json!("TRANSCRIBE.DLL");
        manifest["files"].as_array_mut().unwrap().push(duplicate);
        assert_eq!(
            parse(&manifest).unwrap_err(),
            BundleError::Invalid("duplicate path")
        );
        for sha in ["ABCDEF".repeat(10) + "ABCD", "abc".into(), "g".repeat(64)] {
            let (_temp, mut manifest) = stage(BUNDLE);
            manifest["files"][1]["sha256"] = json!(sha);
            assert_eq!(
                parse(&manifest).unwrap_err(),
                BundleError::Invalid("sha256 is not 64 lowercase hex digits")
            );
        }
    }

    #[test]
    fn a_manifest_without_core_libraries_is_rejected() {
        let (_temp, mut manifest) = stage(BUNDLE);
        manifest["files"]
            .as_array_mut()
            .unwrap()
            .retain(|entry| entry["role"] != "core");
        assert_eq!(
            parse(&manifest).unwrap_err(),
            BundleError::Invalid("no core library listed")
        );
    }

    #[test]
    fn errors_are_fixed_text_naming_the_file() {
        let message = BundleError::HashMismatch {
            path: "gpu/vulkan/ggml-vulkan.dll".into(),
        }
        .to_string();
        assert!(message.contains("gpu/vulkan/ggml-vulkan.dll"), "{message}");
        assert!(message.contains("SHA-256"), "{message}");
    }
}
