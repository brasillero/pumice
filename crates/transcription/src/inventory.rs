//! The transcription models available: the model files in the models folder
//! (p17-serve-model-names/D4, D7, D8).
//!
//! Discovery reads the folder once, when a command starts: the result is a
//! snapshot, and nothing watches the folder afterwards.
//!
//! - Only the folder's immediate children count. A candidate is a `.gguf`
//!   file; the extension is compared ASCII case-insensitively, so
//!   `Model.GGUF` counts too. Names starting with `.` (hidden files, macOS
//!   `._*` resource forks), directories, partial downloads (`*.gguf.part`),
//!   download locks, `SHA256SUMS` and other extensions are ignored.
//! - A symbolic link to a file counts, with its target's size and bytes; a
//!   link to a directory is not followed. A broken `.gguf` link, or one to
//!   something that is not a regular file, is reported as a diagnostic and
//!   skipped.
//! - A file whose size and SHA-256 match a catalog entry takes the catalog
//!   id, whatever its file name; only files whose size matches an entry are
//!   hashed. Any other file takes its exact file stem: no lowercasing, no
//!   slugifying. A file is never given a catalog id from its name alone: a
//!   stem equal to a catalog id without the catalog's bytes is an error.
//! - Ids are compared exactly (case-sensitively). An empty, non-UTF-8,
//!   control-character or over-512-byte id is an error, and so are two files
//!   with the same id (two copies of one catalog model included): the error
//!   names both files, and the directory order never picks one. On
//!   case-insensitive file systems (the Windows and macOS defaults) two
//!   names differing only by case cannot both exist; on Linux they are two
//!   ids, except that `a.gguf` and `a.GGUF` are both `a`, a duplicate.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::catalog::{self, CatalogEntry};
use crate::download::sha256_reader;
use crate::{Error, ErrorCode};

/// The longest model id: what a managed child can announce unchanged.
pub const MAX_ID_BYTES: usize = crate::managed::MAX_MODEL_ID_BYTES;

/// One model file and the id it is available under.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Model {
    pub id: String,
    /// The file in the models folder (a symbolic link stays the link).
    pub file: PathBuf,
    pub size: u64,
    /// The file matched a catalog entry by size and SHA-256; `id` is that
    /// entry's id.
    pub catalog: bool,
}

/// Something in the folder that was skipped or looks wrong, without making
/// the inventory unusable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub file: PathBuf,
    pub message: String,
}

/// The models a command can use, read once from the models folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inventory {
    pub dir: PathBuf,
    /// Sorted by id.
    pub models: Vec<Model>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Inventory {
    /// The model with exactly this id.
    pub fn get(&self, id: &str) -> Option<&Model> {
        self.models.iter().find(|model| model.id == id)
    }

    /// The ids, for messages: `a, b`, or `(none)`.
    pub fn id_list(&self) -> String {
        if self.models.is_empty() {
            return "(none)".to_owned();
        }
        let ids: Vec<&str> = self.models.iter().map(|model| model.id.as_str()).collect();
        ids.join(", ")
    }
}

/// Discovers the models in `dir` against the built-in catalog.
pub fn discover(dir: &Path) -> Result<Inventory, Error> {
    discover_with(dir, catalog::CATALOG)
}

/// [`discover`] against another catalog, so tests can use small fake
/// entries.
pub fn discover_with(dir: &Path, catalog: &[CatalogEntry]) -> Result<Inventory, Error> {
    let mut diagnostics = Vec::new();
    let mut models: Vec<Model> = Vec::new();
    for candidate in scan(dir, &mut diagnostics)? {
        let model = name(&candidate.path, candidate.size, catalog, &mut diagnostics)?;
        if let Some(existing) = models.iter().find(|existing| existing.id == model.id) {
            return Err(inventory_error(format!(
                "two files in the models folder have the model id `{}`: {} and {}; remove or \
                 rename one",
                model.id,
                existing.file.display(),
                model.file.display()
            )));
        }
        models.push(model);
    }
    models.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(Inventory {
        dir: dir.to_path_buf(),
        models,
        diagnostics,
    })
}

/// Names a one-off model file (`file --model-file PATH`) by the same rule,
/// with any diagnostic about it. A missing file is `model_not_found`.
pub fn name_file(path: &Path) -> Result<(Model, Vec<Diagnostic>), Error> {
    name_file_with(path, catalog::CATALOG)
}

/// [`name_file`] against another catalog.
pub fn name_file_with(
    path: &Path,
    catalog: &[CatalogEntry],
) -> Result<(Model, Vec<Diagnostic>), Error> {
    let metadata = fs::metadata(path).map_err(|_| Error::model_not_found(path))?;
    if !metadata.is_file() {
        return Err(Error::model_not_found(path));
    }
    let mut diagnostics = Vec::new();
    let model = name(path, metadata.len(), catalog, &mut diagnostics)?;
    Ok((model, diagnostics))
}

/// The files in `dir` that are copies of `entry` (size and SHA-256), sorted
/// by path. Lenient, unlike [`discover`]: `models download` and `models
/// verify` use it, and other files' problems must not block them. A missing
/// folder has none.
pub fn catalog_copies(dir: &Path, entry: &CatalogEntry) -> Result<Vec<PathBuf>, Error> {
    let mut copies = Vec::new();
    for candidate in scan(dir, &mut Vec::new())? {
        if candidate.size == entry.size_bytes
            && hash_file(&candidate.path, candidate.size)? == entry.sha256
        {
            copies.push(candidate.path);
        }
    }
    Ok(copies)
}

/// Whether `dir` has a candidate model file of `size` bytes, without hashing
/// it (`models catalog`'s `installed`). Unreadable folders have none.
pub fn has_file_of_size(dir: &Path, size: u64) -> bool {
    scan(dir, &mut Vec::new())
        .map(|candidates| candidates.iter().any(|candidate| candidate.size == size))
        .unwrap_or(false)
}

/// A candidate model file and its size (a link's target's).
struct Candidate {
    path: PathBuf,
    size: u64,
}

/// The candidate files among `dir`'s immediate children, sorted by path. A
/// missing folder has none.
fn scan(dir: &Path, diagnostics: &mut Vec<Diagnostic>) -> Result<Vec<Candidate>, Error> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        // Before the first download: no models yet.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(inventory_error(format!(
                "cannot read the models folder {}: {error}",
                dir.display()
            )));
        }
    };
    let read_error = |error: io::Error| {
        inventory_error(format!(
            "cannot read the models folder {}: {error}",
            dir.display()
        ))
    };
    let mut candidates = Vec::new();
    for entry in entries {
        let entry = entry.map_err(read_error)?;
        if !is_candidate_name(&entry.file_name()) {
            continue;
        }
        let path = entry.path();
        let file_type = entry.file_type().map_err(read_error)?;
        let size = if file_type.is_dir() {
            continue;
        } else if file_type.is_symlink() {
            // Followed for files only; a directory link is not followed.
            match fs::metadata(&path) {
                Ok(target) if target.is_file() => target.len(),
                Ok(target) if target.is_dir() => continue,
                Ok(_) => {
                    diagnostics.push(skipped(&path, "links to something that is not a file"));
                    continue;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    diagnostics.push(skipped(&path, "is a broken symbolic link"));
                    continue;
                }
                Err(error) => {
                    diagnostics.push(skipped(
                        &path,
                        &format!("is a symbolic link that cannot be followed ({error})"),
                    ));
                    continue;
                }
            }
        } else if file_type.is_file() {
            entry.metadata().map_err(read_error)?.len()
        } else {
            diagnostics.push(skipped(&path, "is not a regular file"));
            continue;
        };
        candidates.push(Candidate { path, size });
    }
    candidates.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(candidates)
}

/// A visible name with the `.gguf` extension, in any ASCII case.
fn is_candidate_name(name: &OsStr) -> bool {
    !name.as_encoded_bytes().starts_with(b".")
        && Path::new(name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("gguf"))
}

fn skipped(path: &Path, why: &str) -> Diagnostic {
    Diagnostic {
        file: path.to_path_buf(),
        message: format!("{why}; skipped"),
    }
}

/// The id of the file at `path`, of `size` bytes: a catalog id when size
/// and SHA-256 match an entry, else its exact file stem.
fn name(
    path: &Path,
    size: u64,
    catalog: &[CatalogEntry],
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Model, Error> {
    if catalog.iter().any(|entry| entry.size_bytes == size) {
        let digest = hash_file(path, size)?;
        if let Some(entry) = catalog
            .iter()
            .find(|entry| entry.size_bytes == size && entry.sha256 == digest)
        {
            return Ok(Model {
                id: entry.id.to_owned(),
                file: path.to_path_buf(),
                size,
                catalog: true,
            });
        }
    }

    let stem = path.file_stem().unwrap_or_default();
    let id = stem.to_str().ok_or_else(|| {
        inventory_error(format!(
            "the model file name {} is not valid UTF-8, so it gives no model id; rename it",
            path.display()
        ))
    })?;
    if id.is_empty() {
        return Err(inventory_error(format!(
            "the model file {} gives an empty model id; rename it",
            path.display()
        )));
    }
    if id.chars().any(char::is_control) {
        return Err(inventory_error(format!(
            "the model file name {} has control characters, so it gives no model id; rename it",
            path.display()
        )));
    }
    if id.len() > MAX_ID_BYTES {
        return Err(inventory_error(format!(
            "the model file name {} gives a model id of {} bytes; ids are at most {MAX_ID_BYTES} \
             bytes, rename it",
            path.display(),
            id.len()
        )));
    }
    if let Some(entry) = catalog.iter().find(|entry| entry.id == id) {
        return Err(inventory_error(format!(
            "the model file {} is named after the catalog model `{}` but does not match its size \
             and SHA-256; a file never takes a catalog id from its name alone, so rename it",
            path.display(),
            entry.id
        )));
    }
    let file_name = path.file_name().unwrap_or_default();
    if let Some(entry) = catalog
        .iter()
        .find(|entry| file_name == OsStr::new(entry.file_name))
    {
        diagnostics.push(Diagnostic {
            file: path.to_path_buf(),
            message: format!(
                "has the catalog file name of `{0}` but does not match its size and SHA-256 \
                 (corrupt, or another file), so it is the model `{id}`; check it with `models \
                 verify {0}`",
                entry.id
            ),
        });
    }
    Ok(Model {
        id: id.to_owned(),
        file: path.to_path_buf(),
        size,
        catalog: false,
    })
}

/// The lowercase hex SHA-256 of the regular file at `path`, which must still
/// be `size` bytes.
fn hash_file(path: &Path, size: u64) -> Result<String, Error> {
    let read_error =
        |error: io::Error| inventory_error(format!("cannot read {}: {error}", path.display()));
    let mut file = open_regular(path).map_err(read_error)?;
    let metadata = file.metadata().map_err(read_error)?;
    if !metadata.is_file() || metadata.len() != size {
        return Err(inventory_error(format!(
            "{} changed while the models folder was read; run the command again",
            path.display()
        )));
    }
    sha256_reader(&mut file).map_err(read_error)
}

/// Opens without blocking on a FIFO swapped in after the type check.
#[cfg(unix)]
fn open_regular(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_regular(path: &Path) -> io::Result<File> {
    File::open(path)
}

fn inventory_error(message: String) -> Error {
    Error::new(ErrorCode::InvalidConfig, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;

    /// A fake catalog entry for `bytes`, so no real model is needed.
    fn fake_entry(id: &'static str, file_name: &'static str, bytes: &[u8]) -> CatalogEntry {
        let digest = sha2::Sha256::digest(bytes);
        let sha256: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        CatalogEntry {
            id,
            description: "fake",
            url: "https://example.invalid/model.gguf",
            file_name,
            size_bytes: bytes.len() as u64,
            sha256: Box::leak(sha256.into_boxed_str()),
            license: "test",
        }
    }

    const TINY: &[u8] = b"fake tiny model bytes";
    const TURBO: &[u8] = b"fake turbo model, other bytes";

    fn catalog() -> Vec<CatalogEntry> {
        vec![
            fake_entry("fake-tiny-q8_0", "fake-tiny-Q8_0.gguf", TINY),
            fake_entry("fake-turbo-q8_0", "fake-turbo-Q8_0.gguf", TURBO),
        ]
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    fn ids(inventory: &Inventory) -> Vec<&str> {
        inventory.models.iter().map(|m| m.id.as_str()).collect()
    }

    #[test]
    fn a_catalog_file_takes_the_catalog_id_whatever_its_name() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "renamed by me.gguf", TINY);
        write(tmp.path(), "fake-turbo-Q8_0.gguf", TURBO);
        let inventory = discover_with(tmp.path(), &catalog()).unwrap();
        assert_eq!(ids(&inventory), ["fake-tiny-q8_0", "fake-turbo-q8_0"]);
        let tiny = inventory.get("fake-tiny-q8_0").unwrap();
        assert!(tiny.catalog);
        assert_eq!(tiny.file, tmp.path().join("renamed by me.gguf"));
        assert_eq!(tiny.size, TINY.len() as u64);
        assert!(inventory.diagnostics.is_empty());
    }

    #[test]
    fn other_files_keep_their_exact_stem() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "My Model v2.Q5.gguf", b"custom");
        write(tmp.path(), "Upper.GGUF", b"custom too");
        let inventory = discover_with(tmp.path(), &catalog()).unwrap();
        assert_eq!(ids(&inventory), ["My Model v2.Q5", "Upper"]);
        assert!(inventory.models.iter().all(|m| !m.catalog));
        assert!(inventory.get("upper").is_none(), "ids are case-sensitive");
    }

    #[test]
    fn a_catalog_sized_file_with_other_bytes_keeps_its_stem() {
        let tmp = tempfile::tempdir().unwrap();
        let mut bytes = TINY.to_vec();
        bytes[0] ^= 1;
        write(tmp.path(), "tiny-lookalike.gguf", &bytes);
        let inventory = discover_with(tmp.path(), &catalog()).unwrap();
        assert_eq!(ids(&inventory), ["tiny-lookalike"]);
        assert!(!inventory.models[0].catalog);
    }

    #[test]
    fn a_mismatched_file_under_the_catalog_file_name_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write(tmp.path(), "fake-tiny-Q8_0.gguf", b"corrupt");
        let inventory = discover_with(tmp.path(), &catalog()).unwrap();
        assert_eq!(ids(&inventory), ["fake-tiny-Q8_0"]);
        assert_eq!(inventory.diagnostics.len(), 1);
        assert_eq!(inventory.diagnostics[0].file, path);
        assert!(
            inventory.diagnostics[0]
                .message
                .contains("catalog file name of `fake-tiny-q8_0`")
        );
    }

    #[test]
    fn a_file_named_after_a_catalog_id_must_have_its_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "fake-tiny-q8_0.gguf", b"not the catalog bytes");
        let error = discover_with(tmp.path(), &catalog()).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidConfig);
        assert!(
            error
                .message
                .contains("named after the catalog model `fake-tiny-q8_0`"),
            "{}",
            error.message
        );
    }

    #[test]
    fn duplicate_ids_name_both_files() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "same.gguf", b"one");
        write(tmp.path(), "same.GGUF", b"two");
        let error = discover_with(tmp.path(), &catalog());
        // A case-insensitive file system keeps one file under both names.
        if fs::read_dir(tmp.path()).unwrap().count() == 1 {
            assert_eq!(ids(&error.unwrap()), ["same"]);
            return;
        }
        let error = error.unwrap_err();
        assert!(
            error.message.contains("model id `same`")
                && error.message.contains("same.gguf")
                && error.message.contains("same.GGUF"),
            "{}",
            error.message
        );
    }

    #[test]
    fn two_copies_of_one_catalog_model_are_a_duplicate() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "fake-tiny-Q8_0.gguf", TINY);
        write(tmp.path(), "backup.gguf", TINY);
        let error = discover_with(tmp.path(), &catalog()).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidConfig);
        assert!(
            error.message.contains("model id `fake-tiny-q8_0`")
                && error.message.contains("backup.gguf")
                && error.message.contains("fake-tiny-Q8_0.gguf"),
            "{}",
            error.message
        );
    }

    #[test]
    fn other_entries_are_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "fake-tiny-Q8_0.gguf.part", TINY);
        write(tmp.path(), "fake-tiny-Q8_0.gguf.lock", b"");
        write(tmp.path(), "SHA256SUMS", b"sums");
        write(tmp.path(), "notes.txt", b"text");
        write(tmp.path(), "gguf", b"no extension");
        write(tmp.path(), ".hidden.gguf", b"hidden");
        write(tmp.path(), "._model.gguf", b"resource fork");
        fs::create_dir(tmp.path().join("folder.gguf")).unwrap();
        fs::create_dir(tmp.path().join("nested")).unwrap();
        write(
            &tmp.path().join("nested"),
            "deep.gguf",
            b"not an immediate child",
        );
        let inventory = discover_with(tmp.path(), &catalog()).unwrap();
        assert!(inventory.models.is_empty(), "{:?}", inventory.models);
        assert!(inventory.diagnostics.is_empty());
    }

    #[test]
    fn an_empty_or_missing_folder_has_no_models() {
        let tmp = tempfile::tempdir().unwrap();
        let inventory = discover_with(tmp.path(), &catalog()).unwrap();
        assert!(inventory.models.is_empty() && inventory.diagnostics.is_empty());
        assert_eq!(inventory.id_list(), "(none)");

        let missing = tmp.path().join("missing");
        let inventory = discover_with(&missing, &catalog()).unwrap();
        assert!(inventory.models.is_empty() && inventory.diagnostics.is_empty());
    }

    #[test]
    fn a_folder_that_is_a_file_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let file = write(tmp.path(), "file", b"x");
        let error = discover_with(&file, &catalog()).unwrap_err();
        assert!(error.message.contains("cannot read the models folder"));
    }

    #[test]
    fn ids_have_a_length_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join(format!("{}.gguf", "m".repeat(MAX_ID_BYTES + 1)));
        // Most file systems cap a name at 255 bytes: check the rule directly.
        let error = name(&path, 1, &catalog(), &mut Vec::new()).unwrap_err();
        assert!(
            error.message.contains("at most 512 bytes"),
            "{}",
            error.message
        );
        let path = tmp
            .path()
            .join(format!("{}.gguf", "m".repeat(MAX_ID_BYTES)));
        assert!(name(&path, 1, &catalog(), &mut Vec::new()).is_ok());
    }

    #[test]
    fn control_characters_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bad\u{7}name.gguf");
        let error = name(&path, 1, &catalog(), &mut Vec::new()).unwrap_err();
        assert!(
            error.message.contains("control characters"),
            "{}",
            error.message
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_names_are_rejected_unless_the_catalog_names_them() {
        use std::os::unix::ffi::OsStrExt;
        let tmp = tempfile::tempdir().unwrap();
        let name = OsStr::from_bytes(b"caf\xe9.gguf");
        // Some file systems (macOS APFS) refuse non-UTF-8 names.
        if fs::write(tmp.path().join(name), b"custom").is_err() {
            return;
        }
        let error = discover_with(tmp.path(), &catalog()).unwrap_err();
        assert!(
            error.message.contains("not valid UTF-8"),
            "{}",
            error.message
        );

        fs::write(tmp.path().join(name), TINY).unwrap();
        let inventory = discover_with(tmp.path(), &catalog()).unwrap();
        assert_eq!(ids(&inventory), ["fake-tiny-q8_0"]);
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_links() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = write(outside.path(), "anything", TINY);
        symlink(&target, tmp.path().join("linked.gguf")).unwrap();
        let custom = write(outside.path(), "custom-target.gguf", b"custom");
        symlink(&custom, tmp.path().join("custom-link.gguf")).unwrap();
        fs::create_dir(outside.path().join("dir")).unwrap();
        write(&outside.path().join("dir"), "inside.gguf", b"x");
        symlink(outside.path().join("dir"), tmp.path().join("dir-link.gguf")).unwrap();
        symlink(outside.path().join("gone"), tmp.path().join("broken.gguf")).unwrap();
        symlink(outside.path().join("gone"), tmp.path().join("broken.txt")).unwrap();

        let inventory = discover_with(tmp.path(), &catalog()).unwrap();
        assert_eq!(ids(&inventory), ["custom-link", "fake-tiny-q8_0"]);
        let tiny = inventory.get("fake-tiny-q8_0").unwrap();
        assert_eq!(tiny.file, tmp.path().join("linked.gguf"));
        assert_eq!(tiny.size, TINY.len() as u64);
        assert_eq!(
            inventory.diagnostics.len(),
            1,
            "{:?}",
            inventory.diagnostics
        );
        assert_eq!(
            inventory.diagnostics[0].file,
            tmp.path().join("broken.gguf")
        );
        assert!(
            inventory.diagnostics[0]
                .message
                .contains("broken symbolic link")
        );
    }

    #[test]
    fn one_off_files_follow_the_same_rule() {
        let tmp = tempfile::tempdir().unwrap();
        let tiny = write(tmp.path(), "whatever.bin", TINY);
        let (model, diagnostics) = name_file_with(&tiny, &catalog()).unwrap();
        assert_eq!(model.id, "fake-tiny-q8_0");
        assert!(model.catalog && diagnostics.is_empty());

        let custom = write(tmp.path(), "Custom Name.gguf", b"custom");
        let (model, _) = name_file_with(&custom, &catalog()).unwrap();
        assert_eq!(model.id, "Custom Name");
        assert!(!model.catalog);

        let missing = tmp.path().join("missing.gguf");
        let error = name_file_with(&missing, &catalog()).unwrap_err();
        assert_eq!(error.code, ErrorCode::ModelNotFound);
        let error = name_file_with(tmp.path(), &catalog()).unwrap_err();
        assert_eq!(error.code, ErrorCode::ModelNotFound);
    }

    #[test]
    fn catalog_copies_ignore_other_problems() {
        let tmp = tempfile::tempdir().unwrap();
        let entries = catalog();
        assert!(
            catalog_copies(&tmp.path().join("missing"), &entries[0])
                .unwrap()
                .is_empty()
        );
        write(tmp.path(), "a copy.gguf", TINY);
        write(tmp.path(), "dup.gguf", b"x");
        write(tmp.path(), "dup.GGUF", b"y");
        write(tmp.path(), "fake-turbo-q8_0.gguf", b"impostor");
        assert_eq!(
            catalog_copies(tmp.path(), &entries[0]).unwrap(),
            [tmp.path().join("a copy.gguf")]
        );
        assert!(catalog_copies(tmp.path(), &entries[1]).unwrap().is_empty());
        assert!(has_file_of_size(tmp.path(), TINY.len() as u64));
        assert!(!has_file_of_size(tmp.path(), TURBO.len() as u64));
    }

    #[test]
    fn the_real_catalog_names_are_candidates() {
        for entry in catalog::CATALOG {
            assert!(is_candidate_name(OsStr::new(entry.file_name)));
        }
    }
}
