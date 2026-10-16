//! Explicit, pinned model downloads (`arch-modules/D9`, task
//! `p9-transcribe-downloader` decisions D1–D3).
//!
//! Downloads only ever happen because the user ran `models download`; nothing
//! is fetched at startup or because of a request. Bytes stream into a
//! `<file>.part` file created exclusively and privately, are hashed while
//! writing and are renamed into place only when both the byte size and the
//! SHA-256 from the catalog match; the installed file is then reopened
//! without following links and re-read and re-hashed against the catalog
//! (D4). Any failure deletes the partial file and restarts from zero next
//! time — there is no resume (D3).
//!
//! Installation renames a name in the models directory, so the trust
//! boundary is that directory: on Unix it (and any parents this module
//! creates) is created mode 0700, and an existing directory writable by
//! group or others is refused. Directories above it (the user's home and
//! data directories) are trusted: a writer able to rename those can already
//! replace anything the user runs (D4). Same-user mutation is not a useful
//! filesystem security boundary and is out of scope.
//!
//! The only request header is `User-Agent: pumice-transcribe/<version>`.
//! There are no credentials, cookies or AI subscription tokens involved.

use crate::catalog::CatalogEntry;
use crate::{Error, ErrorCode};
use sha2::Digest;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How many redirects one download follows at most.
const MAX_REDIRECTS: u32 = 5;

/// Windows attribute/flag constants (winapi `winbase.h` /
/// `FileApi.h`); the std equivalents of the open flag are not exposed.
#[cfg(windows)]
mod win {
    /// `FILE_ATTRIBUTE_REPARSE_POINT`: the entry is a reparse point
    /// (symlink, junction, …).
    pub const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    /// `FILE_FLAG_OPEN_REPARSE_POINT`: open the reparse point itself instead
    /// of following it to its target.
    pub const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
}

/// Buffer size while streaming bytes to disk.
const BUFFER_SIZE: usize = 64 * 1024;

/// Floor for the overall per-download deadline ([`Timeouts::total`]).
const MIN_TOTAL_DEADLINE: Duration = Duration::from_secs(600);

/// Rate used to grow the overall deadline with the model size: one second per
/// 256 KiB, so e.g. the 46 MiB tiny model gets the 10-minute floor while a
/// multi-GB model gets proportionally longer.
const DEADLINE_BYTES_PER_SEC: u64 = 256 * 1024;

/// What a download fetches: the pinned URL, the final file name and the
/// expected byte size and SHA-256. Built from a catalog entry in production;
/// tests construct it directly to point at a fake server on loopback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadSpec {
    pub url: String,
    pub file_name: String,
    pub size_bytes: u64,
    pub sha256: String,
}

impl From<&CatalogEntry> for DownloadSpec {
    fn from(entry: &CatalogEntry) -> Self {
        Self {
            url: entry.url.to_string(),
            file_name: entry.file_name.to_string(),
            size_bytes: entry.size_bytes,
            sha256: entry.sha256.to_string(),
        }
    }
}

/// Which redirect targets a download may follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectPolicy {
    /// Production policy: only `https:` targets. Hugging Face redirects to an
    /// HTTPS CDN; plain HTTP redirects are refused.
    HttpsOnly,
    /// Test-only policy: also allows `http:` so tests can use a loopback
    /// fake server.
    AllowHttp,
}

impl RedirectPolicy {
    fn allows(&self, url: &url::Url) -> bool {
        match self {
            Self::HttpsOnly => url.scheme() == "https",
            Self::AllowHttp => matches!(url.scheme(), "http" | "https"),
        }
    }
}

/// Network timeouts for a download.
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    /// Time allowed to establish the connection.
    pub connect: Duration,
    /// Floor for the overall per-download deadline; the effective deadline
    /// also grows with the model size (see [`Timeouts::total_deadline`]).
    pub total: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(30),
            total: MIN_TOTAL_DEADLINE,
        }
    }
}

impl Timeouts {
    /// Overall budget for one download of `size_bytes`:
    /// `max(self.total, size_bytes / 256 KiB per s)`.
    pub fn total_deadline(&self, size_bytes: u64) -> Duration {
        let size_based = Duration::from_secs(size_bytes.div_ceil(DEADLINE_BYTES_PER_SEC));
        self.total.max(size_based)
    }
}

/// A phase transition of a download, emitted before the phase starts. The CLI
/// surfaces these as JSONL `state` events for progress UIs (see
/// `docs/transcription.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadState {
    /// About to issue the HTTP request. ureq resolves DNS blocking, so a
    /// stalled resolver can outlast every configured timeout.
    Connecting,
    /// Streaming response bytes into the staging file.
    Downloading,
    /// Comparing the streamed SHA-256 against the catalog.
    Verifying,
    /// Renaming the verified file into place.
    Installing,
}

impl DownloadState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Downloading => "downloading",
            Self::Verifying => "verifying",
            Self::Installing => "installing",
        }
    }
}

/// One observable step of a download, for progress UIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadEvent {
    /// A phase transition, before the phase starts.
    State(DownloadState),
    /// `received` of `total` bytes have been written.
    Progress { received: u64, total: u64 },
}

/// What [`Downloader::fetch`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchResult {
    /// A file verified against the spec was already installed.
    AlreadyInstalled {
        path: PathBuf,
        bytes: u64,
        sha256: String,
    },
    /// The file was downloaded, verified and atomically installed.
    Downloaded {
        path: PathBuf,
        bytes: u64,
        sha256: String,
    },
}

/// A file that re-hashed cleanly against its spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedFile {
    pub bytes: u64,
    pub sha256: String,
}

/// Blocking downloader for catalog models using ureq with rustls. A single
/// `Downloader` is cheap; build one per command.
pub struct Downloader {
    redirect_policy: RedirectPolicy,
    timeouts: Timeouts,
}

impl Downloader {
    pub fn new(redirect_policy: RedirectPolicy, timeouts: Timeouts) -> Self {
        Self {
            redirect_policy,
            timeouts,
        }
    }

    /// Builds the HTTP client for one download. The per-download deadline is
    /// a single absolute budget: [`Downloader::stream_to_file`] sets each
    /// request's overall timeout to the time remaining on that budget, so
    /// DNS, connect, redirects, response headers and body reads all share
    /// it and ureq checks the remaining time around every socket operation.
    /// A blocked DNS query itself cannot be interrupted, so it can overshoot
    /// the budget — the deadline is rechecked right before installation to
    /// close that gap.
    /// ureq lets the connect timeout win over the request timeout, so each
    /// hop gets a client whose connect timeout is capped by `remaining`.
    fn agent(&self, remaining: std::time::Duration) -> ureq::Agent {
        ureq::AgentBuilder::new()
            .user_agent(concat!("pumice-transcribe/", env!("CARGO_PKG_VERSION")))
            .timeout_connect(self.timeouts.connect.min(remaining))
            .redirects(0)
            .build()
    }

    /// Downloads `spec` into `models_dir`, unless a verified file is already
    /// installed. `on_event` receives phase transitions and byte progress as
    /// the download advances; throttling is the caller's job.
    ///
    /// A second concurrent download of the same model fails fast with
    /// `download_failed` ("already in progress") instead of writing the same
    /// file twice. An existing file that fails verification is reported as
    /// corrupt unless `force` replaces it.
    pub fn fetch(
        &self,
        spec: &DownloadSpec,
        models_dir: &Path,
        force: bool,
        on_event: &mut dyn FnMut(DownloadEvent),
    ) -> Result<FetchResult, Error> {
        prepare_models_dir(models_dir)?;
        let dest = models_dir.join(&spec.file_name);

        // Per-model lock, released by the OS if this process dies. The lock
        // file persists (it holds no data); only the guard matters.
        let lock_file = open_lock_file(&lock_path(&dest))?;
        let mut lock = fd_lock::RwLock::new(lock_file);
        let _guard = match lock.try_write() {
            Ok(guard) => guard,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                return Err(Error::new(
                    ErrorCode::DownloadFailed,
                    format!(
                        "another download is already in progress for {}",
                        spec.file_name
                    ),
                ));
            }
            Err(e) => {
                return Err(Error::download_failed(format!(
                    "cannot lock {}: {e}",
                    lock_path(&dest).display()
                )));
            }
        };

        if dest.is_file() {
            match verify_file(&dest, spec) {
                Ok(verified) => {
                    return Ok(FetchResult::AlreadyInstalled {
                        path: dest,
                        bytes: verified.bytes,
                        sha256: verified.sha256,
                    });
                }
                Err(_) if force => {
                    std::fs::remove_file(&dest).map_err(|e| {
                        Error::download_failed(format!("cannot replace {}: {e}", dest.display()))
                    })?;
                }
                Err(_) => {
                    return Err(Error::checksum_mismatch(format!(
                        "{} exists but does not match the catalog entry (corrupt or modified); re-run with --force to replace it",
                        dest.display()
                    )));
                }
            }
        }

        self.download_locked(spec, models_dir, &dest, on_event)
    }

    /// Streams the download into `dest.part` and renames it into place only
    /// when size and SHA-256 match. On any error the partial file is removed
    /// (D3): a partial file never looks installed.
    fn download_locked(
        &self,
        spec: &DownloadSpec,
        models_dir: &Path,
        dest: &Path,
        on_event: &mut dyn FnMut(DownloadEvent),
    ) -> Result<FetchResult, Error> {
        // The directory sync below is Unix-only; on other platforms the
        // parameter is intentionally unused.
        #[cfg(not(unix))]
        let _ = models_dir;

        let part = part_path(dest);
        // A stale .part can only come from a crashed download (failures clean
        // up after themselves); we hold the lock, so removing it is safe.
        match std::fs::remove_file(&part) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(Error::download_failed(format!(
                    "cannot remove stale partial file {}: {e}",
                    part.display()
                )));
            }
        }

        let mut file = create_part_file(&part)?;

        let started = std::time::Instant::now();
        let deadline = self.timeouts.total_deadline(spec.size_bytes);

        // `dest` does not exist here (see `fetch`): until the rename installs
        // our file, removing it is a no-op; afterwards it undoes the
        // installation.
        let outcome = (|| {
            let sha256 = self.stream_to_file(spec, &mut file, started, deadline, on_event)?;
            file.sync_all().map_err(|e| {
                Error::download_failed(format!("cannot sync {}: {e}", part.display()))
            })?;
            // The transfer can overshoot the deadline without a read stall
            // or a socket error (a stalled resolver, slow disk): an expired
            // budget must never install.
            check_deadline(started, deadline, spec.size_bytes)?;
            on_event(DownloadEvent::State(DownloadState::Installing));
            std::fs::rename(&part, dest).map_err(|e| {
                Error::download_failed(format!("cannot move {} into place: {e}", part.display()))
            })?;
            confirm_installed_file(dest, spec)?;
            // Persist the directory entry itself; without this a power loss
            // can roll back the rename after success was reported. There is
            // no directory sync on Windows: the rename is the commit point
            // there. On failure the cleanup below removes the installed file.
            #[cfg(unix)]
            sync_directory(models_dir)?;
            Ok(sha256)
        })();

        match outcome {
            Ok(sha256) => Ok(FetchResult::Downloaded {
                path: dest.to_path_buf(),
                bytes: spec.size_bytes,
                sha256,
            }),
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                let _ = std::fs::remove_file(dest);
                Err(e)
            }
        }
    }

    /// Performs the HTTP GET (following redirects by hand, so the policy and
    /// the limit are testable) and writes the body to `file`, hashing on the
    /// way. Enforces the exact expected size: more bytes, fewer bytes or a
    /// wrong `Content-Length` are all failures. The per-download deadline is
    /// one absolute budget shared by every request: each request (including
    /// every redirect hop) gets the remaining time as its overall timeout,
    /// so a redirect chain cannot extend the budget. The budget is also
    /// polled around body reads.
    fn stream_to_file(
        &self,
        spec: &DownloadSpec,
        file: &mut File,
        started: std::time::Instant,
        deadline: Duration,
        on_event: &mut dyn FnMut(DownloadEvent),
    ) -> Result<String, Error> {
        let mut url = url::Url::parse(&spec.url)
            .map_err(|e| Error::download_failed(format!("invalid download URL: {e}")))?;
        let deadline_at = started + deadline;
        let mut redirects = 0u32;
        on_event(DownloadEvent::State(DownloadState::Connecting));
        let response = loop {
            // Fail immediately once the budget is spent instead of issuing a
            // request with no time left.
            check_deadline(started, deadline, spec.size_bytes)?;
            // Each redirect hop is a fresh request; ureq would otherwise
            // start a new full-length timeout for it. The overall timeout of
            // this request is the remaining budget.
            let remaining = deadline_at.saturating_duration_since(std::time::Instant::now());
            let agent = self.agent(remaining);
            let (status, response) = match agent.get(url.as_str()).timeout(remaining).call() {
                Ok(response) => (response.status(), response),
                Err(ureq::Error::Status(status, response)) => (status, response),
                Err(ureq::Error::Transport(transport)) => {
                    return Err(Error::download_failed(format!(
                        "download failed: {}",
                        transport.kind()
                    )));
                }
            };
            if (200..300).contains(&status) {
                break response;
            }
            if !is_redirect(status) {
                return Err(Error::download_failed(format!(
                    "download failed with HTTP status {status}"
                )));
            }
            // ureq is configured with `redirects(0)`; a 3xx arrives either as
            // an error or, once its own budget is exhausted, as a plain
            // response. Follow it by hand so the policy and limit are
            // testable.
            if redirects >= MAX_REDIRECTS {
                return Err(Error::download_failed(format!(
                    "more than {MAX_REDIRECTS} redirects"
                )));
            }
            let location = response.header("Location").ok_or_else(|| {
                Error::download_failed(format!("HTTP redirect {status} has no Location header"))
            })?;
            let target = url
                .join(location)
                .map_err(|e| Error::download_failed(format!("invalid redirect location: {e}")))?;
            if !self.redirect_policy.allows(&target) {
                return Err(Error::download_failed(format!(
                    "refusing redirect to a non-HTTPS URL ({}://…)",
                    target.scheme()
                )));
            }
            url = target;
            redirects += 1;
        };

        if let Some(length) = response.header("content-length")
            && let Ok(length) = length.parse::<u64>()
            && length != spec.size_bytes
        {
            return Err(Error::download_failed(format!(
                "server announced {length} bytes but the catalog expects {}; refusing the download",
                spec.size_bytes
            )));
        }

        on_event(DownloadEvent::State(DownloadState::Downloading));
        let mut reader = response.into_reader();
        let mut hasher = sha2::Sha256::new();
        let mut written = 0u64;
        let mut buffer = vec![0u8; BUFFER_SIZE];
        loop {
            check_deadline(started, deadline, spec.size_bytes)?;
            let n = reader
                .read(&mut buffer)
                .map_err(|e| Error::download_failed(format!("download interrupted: {e}")))?;
            if n == 0 {
                break;
            }
            written += n as u64;
            if written > spec.size_bytes {
                return Err(Error::download_failed(format!(
                    "download exceeded the expected {} bytes",
                    spec.size_bytes
                )));
            }
            hasher.update(&buffer[..n]);
            file.write_all(&buffer[..n]).map_err(|e| {
                Error::download_failed(format!("cannot write download to disk: {e}"))
            })?;
            on_event(DownloadEvent::Progress {
                received: written,
                total: spec.size_bytes,
            });
        }
        if written != spec.size_bytes {
            return Err(Error::download_failed(format!(
                "download ended at {written} bytes but the catalog expects {}",
                spec.size_bytes
            )));
        }

        on_event(DownloadEvent::State(DownloadState::Verifying));
        let digest = hex_lower(&hasher.finalize());
        if digest != spec.sha256 {
            return Err(Error::checksum_mismatch(format!(
                "downloaded file hash {digest} does not match the catalog hash {}",
                spec.sha256
            )));
        }
        Ok(digest)
    }
}

/// Re-hashes an installed file against `spec` (used by `models verify`).
pub fn verify_file(path: &Path, spec: &DownloadSpec) -> Result<VerifiedFile, Error> {
    let meta = std::fs::metadata(path).map_err(|_| Error::model_not_found(path))?;
    if !meta.is_file() {
        return Err(Error::model_not_found(path));
    }
    if meta.len() != spec.size_bytes {
        return Err(Error::checksum_mismatch(format!(
            "{} is {} bytes; the catalog expects {}",
            path.display(),
            meta.len(),
            spec.size_bytes
        )));
    }
    let mut file = File::open(path)
        .map_err(|e| Error::download_failed(format!("cannot read {}: {e}", path.display())))?;
    let digest = sha256_reader(&mut file)
        .map_err(|e| Error::download_failed(format!("cannot read {}: {e}", path.display())))?;
    if digest != spec.sha256 {
        return Err(Error::checksum_mismatch(format!(
            "{} does not match the catalog hash (got {digest})",
            path.display()
        )));
    }
    Ok(VerifiedFile {
        bytes: meta.len(),
        sha256: digest,
    })
}

pub(crate) fn sha256_reader(reader: &mut impl Read) -> io::Result<String> {
    let mut hasher = sha2::Sha256::new();
    let mut buffer = vec![0u8; BUFFER_SIZE];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

/// Prepares `dir` for downloads. On Unix the directory (and any parents this
/// call creates) is created private (0700) and an existing directory that is
/// writable by group or others is refused: the downloader hashes one file
/// but installs by name, which is only safe in a directory other users
/// cannot write.
#[cfg(unix)]
fn prepare_models_dir(dir: &Path) -> Result<(), Error> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};

    // Find the first existing ancestor; everything below it is ours to
    // create, shallowest first.
    let mut missing: Vec<&Path> = Vec::new();
    let mut cursor = dir;
    loop {
        match std::fs::metadata(cursor) {
            Ok(meta) if meta.is_dir() => break,
            Ok(_) => {
                return Err(Error::download_failed(format!(
                    "models directory {} exists but is not a directory",
                    cursor.display()
                )));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                missing.push(cursor);
                cursor = cursor.parent().ok_or_else(|| {
                    Error::download_failed(format!(
                        "cannot create models directory {}: no existing parent",
                        dir.display()
                    ))
                })?;
            }
            Err(e) => {
                return Err(Error::download_failed(format!(
                    "cannot inspect models directory {}: {e}",
                    cursor.display()
                )));
            }
        }
    }
    for path in missing.iter().rev() {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .map_err(|e| {
                Error::download_failed(format!(
                    "cannot create models directory {}: {e}",
                    path.display()
                ))
            })?;
        // Persist each newly created directory entry (fsync the parent).
        if let Some(parent) = path.parent() {
            sync_directory(parent)?;
        }
    }
    if missing.is_empty() {
        let meta = std::fs::metadata(dir).map_err(|e| {
            Error::download_failed(format!(
                "cannot inspect models directory {}: {e}",
                dir.display()
            ))
        })?;
        if meta.mode() & 0o022 != 0 {
            return Err(Error::download_failed(format!(
                "models directory {} is writable by group or others (mode {:04o}); refusing to download into a shared directory (fix its permissions, e.g. chmod 700, or set another models_dir)",
                dir.display(),
                meta.mode() & 0o7777
            )));
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn prepare_models_dir(dir: &Path) -> Result<(), Error> {
    std::fs::create_dir_all(dir).map_err(|e| {
        Error::download_failed(format!(
            "cannot create models directory {}: {e}",
            dir.display()
        ))
    })?;
    if !dir.is_dir() {
        return Err(Error::download_failed(format!(
            "models directory {} is not a directory",
            dir.display()
        )));
    }
    Ok(())
}

/// Fsyncs a directory so entries created or renamed inside it are durable.
#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), Error> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|e| {
            Error::download_failed(format!("cannot sync directory {}: {e}", path.display()))
        })
}

/// Opens (creating if needed) the per-model lock file. The lock holds no
/// data, so it is never truncated and never opened through a link: a
/// symlinked lock path must not erase an unrelated file.
fn open_lock_file(path: &Path) -> Result<File, Error> {
    reject_non_regular_lock_path(path)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW).mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Open the leaf as itself: a reparse point swapped in after the
        // pre-check must not be followed to its target.
        options.custom_flags(win::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path).map_err(|e| {
        Error::download_failed(format!("cannot open lock file {}: {e}", path.display()))
    })?;
    // Re-check on the opened handle: a FIFO (or a path type swapped in after
    // the pre-check) is caught by its own metadata, not the name's.
    let meta = file.metadata().map_err(|e| {
        Error::download_failed(format!("cannot inspect lock file {}: {e}", path.display()))
    })?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & win::FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(non_regular_lock_error(path));
        }
    }
    if !meta.file_type().is_file() {
        return Err(non_regular_lock_error(path));
    }
    Ok(file)
}

fn reject_non_regular_lock_path(path: &Path) -> Result<(), Error> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(Error::download_failed(format!(
                "cannot inspect lock file {}: {e}",
                path.display()
            )));
        }
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Any reparse point (symlink, junction, …) is rejected even when it
        // resolves to a regular file.
        if meta.file_attributes() & win::FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(non_regular_lock_error(path));
        }
    }
    if !meta.file_type().is_file() {
        return Err(non_regular_lock_error(path));
    }
    Ok(())
}

fn non_regular_lock_error(path: &Path) -> Error {
    Error::download_failed(format!(
        "lock file {} is not a regular file (symlink, directory or FIFO); refusing to open it",
        path.display()
    ))
}

/// Creates the staging `.part` file exclusively — a stale one was removed
/// under the lock, so the name must not exist — and private, without
/// following links: a concurrent directory writer must not be able to
/// redirect the bytes through a link.
#[cfg(unix)]
fn create_part_file(path: &Path) -> Result<File, Error> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(path)
        .map_err(|e| Error::download_failed(format!("cannot create {}: {e}", path.display())))
}

#[cfg(not(unix))]
fn create_part_file(path: &Path) -> Result<File, Error> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // `create_new` maps to CREATE_NEW, which refuses to open an existing
        // name, including a reparse point; opening with the flag keeps the
        // leaf itself from being followed in every case.
        options.custom_flags(win::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options
        .open(path)
        .map_err(|e| Error::download_failed(format!("cannot create {}: {e}", path.display())))
}

/// Confirms the file now installed at `dest` still matches the catalog: the
/// path is reopened without following links and re-read and re-hashed
/// against the catalog entry (D4). Installation renames by name, so a
/// concurrent directory writer could have swapped names between staging and
/// rename; `prepare_models_dir` keeps other users out of the directory, and
/// this re-hash turns any remaining swap into a failure instead of an
/// unverified installation. On any mismatch the destination is removed.
fn confirm_installed_file(dest: &Path, spec: &DownloadSpec) -> Result<VerifiedFile, Error> {
    let mut opened = reopen_installed(dest)?;
    let meta = opened
        .metadata()
        .map_err(|e| installed_file_error(dest, &format!("cannot inspect it: {e}")))?;
    if !meta.is_file() {
        return Err(installed_file_error(dest, "it is not a regular file"));
    }
    if meta.len() != spec.size_bytes {
        return Err(installed_file_mismatch(
            dest,
            &format!(
                "it is {} bytes after installation; the catalog expects {}",
                meta.len(),
                spec.size_bytes
            ),
        ));
    }
    let digest = sha256_reader(&mut opened)
        .map_err(|e| installed_file_error(dest, &format!("cannot re-hash it: {e}")))?;
    if digest != spec.sha256 {
        return Err(installed_file_mismatch(
            dest,
            "its hash changed between verification and installation",
        ));
    }
    Ok(VerifiedFile {
        bytes: meta.len(),
        sha256: digest,
    })
}

/// Reopens the installed path without following links. On Unix this is
/// `O_NOFOLLOW`. On Windows the open carries `FILE_FLAG_OPEN_REPARSE_POINT`,
/// so a symlink or junction swapped in between the pre-check and the open is
/// opened as itself instead of being followed to its target; the opened
/// handle's own attributes then reject it (and any non-regular file) before
/// re-hashing.
#[cfg(unix)]
fn reopen_installed(dest: &Path) -> Result<File, Error> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dest)
        .map_err(|e| installed_file_error(dest, &format!("cannot reopen it: {e}")))
}

#[cfg(not(unix))]
fn reopen_installed(dest: &Path) -> Result<File, Error> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    let reject = || {
        installed_file_error(
            dest,
            "it is not a regular file (symlink, junction, directory or FIFO)",
        )
    };
    match std::fs::symlink_metadata(dest) {
        Ok(meta) => {
            if meta.file_attributes() & win::FILE_ATTRIBUTE_REPARSE_POINT != 0 || !meta.is_file() {
                return Err(reject());
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(reject()),
        Err(e) => {
            return Err(installed_file_error(
                dest,
                &format!("cannot inspect it: {e}"),
            ));
        }
    }
    // Without this flag CreateFile follows a symlink substituted after the
    // pre-check above; the handle would then describe the target and the
    // attribute check could not see the link.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(win::FILE_FLAG_OPEN_REPARSE_POINT)
        .open(dest)
        .map_err(|e| installed_file_error(dest, &format!("cannot reopen it: {e}")))?;
    // The handle's own attributes, not the path's: this is what catches the
    // raced-in reparse point.
    let meta = file
        .metadata()
        .map_err(|e| installed_file_error(dest, &format!("cannot inspect the opened file: {e}")))?;
    if !meta.is_file() || meta.file_attributes() & win::FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(installed_file_error(
            dest,
            "the opened file is not a regular file (symlink, junction, directory or FIFO)",
        ));
    }
    Ok(file)
}

fn installed_file_error(dest: &Path, reason: &str) -> Error {
    let _ = std::fs::remove_file(dest);
    Error::download_failed(format!(
        "installed file {} failed the post-install verification ({reason}); the verified file was replaced during installation",
        dest.display()
    ))
}

fn installed_file_mismatch(dest: &Path, reason: &str) -> Error {
    let _ = std::fs::remove_file(dest);
    Error::checksum_mismatch(format!(
        "installed file {} failed the post-install verification ({reason}); the verified file was replaced during installation",
        dest.display()
    ))
}

/// Fails once `started` has outrun the per-download `deadline`.
fn check_deadline(
    started: std::time::Instant,
    deadline: Duration,
    size_bytes: u64,
) -> Result<(), Error> {
    if started.elapsed() > deadline {
        return Err(Error::download_failed(format!(
            "download exceeded its time budget of {deadline:?} for {size_bytes} bytes"
        )));
    }
    Ok(())
}

/// The temporary name for a download in progress, next to the final file on
/// the same filesystem so the final rename is atomic.
pub fn part_path(dest: &Path) -> PathBuf {
    dest.with_file_name(format!(
        "{}.part",
        dest.file_name().unwrap_or_default().to_string_lossy()
    ))
}

fn lock_path(dest: &Path) -> PathBuf {
    dest.with_file_name(format!(
        "{}.lock",
        dest.file_name().unwrap_or_default().to_string_lossy()
    ))
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, mpsc};

    const TEST_BYTES: &[u8] = b"the quick brown fox jumps over the lazy dog";

    fn sha256_of(bytes: &[u8]) -> String {
        let mut hasher = sha2::Sha256::new();
        hasher.update(bytes);
        hex_lower(&hasher.finalize())
    }

    /// A temporary models directory the downloader accepts: `tempfile`
    /// inherits the process umask (often 0775 group-writable), which the
    /// downloader rightly refuses.
    fn private_dir() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        tmp
    }

    /// A response a fake handler writes to one connection. A client that
    /// already gave up (the deadline tests) is not a handler failure, so
    /// write errors are left to the client's side of the test.
    fn respond(stream: &mut TcpStream, status: u16, headers: &[(&str, &str)], body: &[u8]) {
        use std::io::Write as _;
        let head = format!(
            "HTTP/1.1 {status} {}\r\n{}content-length: {}\r\n\r\n",
            match status {
                200 => "OK",
                302 => "Found",
                404 => "Not Found",
                500 => "Internal Server Error",
                _ => "X",
            },
            headers
                .iter()
                .map(|(name, value)| format!("{name}: {value}\r\n"))
                .collect::<String>(),
            body.len()
        );
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(body);
    }

    /// Read and write limit on each accepted connection.
    const SERVER_IO_TIMEOUT: Duration = Duration::from_secs(10);
    /// How long the end of a test waits for running handlers.
    const SERVER_DRAIN: Duration = Duration::from_secs(15);

    /// A fake HTTP server on 127.0.0.1:0, owned by one test. Only for
    /// tests: reads one request per connection (with read and write
    /// limits) and hands it to the handler, which answers HTTP/1.1 with
    /// connection close. Dropping it stops the listener, waits (bounded)
    /// for the handlers and fails the test if one panicked or still runs.
    struct FakeServer {
        base: String,
        requests: Arc<AtomicUsize>,
        stop: Arc<AtomicBool>,
        listener: Option<std::thread::JoinHandle<()>>,
        /// One report per handler: `Err` carries its panic message.
        reports: mpsc::Receiver<Result<(), String>>,
    }

    impl FakeServer {
        /// How many connections the server accepted.
        fn requests(&self) -> usize {
            self.requests.load(Ordering::SeqCst)
        }
    }

    impl Drop for FakeServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            // The listener polls `stop`, so this join is short.
            if let Some(listener) = self.listener.take() {
                let _ = listener.join();
            }
            let deadline = std::time::Instant::now() + SERVER_DRAIN;
            let mut failures = Vec::new();
            let mut running = 0;
            for _ in 0..self.requests() {
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                match self.reports.recv_timeout(left) {
                    Ok(Ok(())) => {}
                    Ok(Err(message)) => failures.push(message),
                    Err(_) => running += 1,
                }
            }
            if !std::thread::panicking() {
                assert!(
                    failures.is_empty(),
                    "fake server handler failed: {failures:?}"
                );
                assert_eq!(running, 0, "fake server handlers still running at the end");
            }
        }
    }

    /// Serves `handler` on 127.0.0.1:0 until the returned server is
    /// dropped.
    fn serve(handler: impl Fn(&str, &mut TcpStream) + Send + Sync + 'static) -> FakeServer {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (report, reports) = mpsc::channel();
        let handler = Arc::new(handler);
        let thread = {
            let (requests, stop) = (Arc::clone(&requests), Arc::clone(&stop));
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let mut stream = match listener.accept() {
                        Ok((stream, _)) => stream,
                        Err(_) => {
                            std::thread::sleep(Duration::from_millis(2));
                            continue;
                        }
                    };
                    // Accepted sockets can inherit non-blocking mode.
                    let configured = stream
                        .set_nonblocking(false)
                        .and_then(|()| stream.set_read_timeout(Some(SERVER_IO_TIMEOUT)))
                        .and_then(|()| stream.set_write_timeout(Some(SERVER_IO_TIMEOUT)));
                    requests.fetch_add(1, Ordering::SeqCst);
                    let (handler, report) = (Arc::clone(&handler), report.clone());
                    std::thread::spawn(move || {
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            configured.expect("configure the accepted connection");
                            // A client that sent no request (it gave up) is
                            // not the handler's business.
                            if let Some(path) = read_request_path(&mut stream) {
                                handler(&path, &mut stream);
                            }
                        }));
                        let _ = report.send(result.map_err(|panic| panic_message(&*panic)));
                    });
                }
            })
        };
        FakeServer {
            base: format!("http://{addr}"),
            requests,
            stop,
            listener: Some(thread),
            reports,
        }
    }

    fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
        panic
            .downcast_ref::<&str>()
            .map(|text| (*text).to_owned())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "a non-text panic".to_owned())
    }

    /// The request's path, after reading its head; `None` when the client
    /// sent no complete head.
    fn read_request_path(stream: &mut TcpStream) -> Option<String> {
        use std::io::BufRead;
        let mut reader = io::BufReader::new(stream);
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
        // Consume the remaining headers.
        loop {
            line.clear();
            if reader.read_line(&mut line).ok()? == 0 {
                return None;
            }
            if line.trim_end().is_empty() {
                break;
            }
        }
        Some(path)
    }

    /// A spec pointing at `base_url/path` with matching size and hash.
    fn spec_for(base_url: &str, bytes: &[u8]) -> DownloadSpec {
        DownloadSpec {
            url: format!("{base_url}/file"),
            file_name: "model.gguf".to_string(),
            size_bytes: bytes.len() as u64,
            sha256: sha256_of(bytes),
        }
    }

    fn test_timeouts() -> Timeouts {
        Timeouts {
            connect: Duration::from_secs(5),
            total: Duration::from_secs(600),
        }
    }

    fn fetch(spec: &DownloadSpec, dir: &Path, force: bool) -> Result<FetchResult, Error> {
        Downloader::new(RedirectPolicy::AllowHttp, test_timeouts()).fetch(
            spec,
            dir,
            force,
            &mut |_| {},
        )
    }

    #[test]
    fn download_installs_verified_file() {
        let tmp = private_dir();
        let server = serve(|path, stream| {
            assert_eq!(path, "/file");
            respond(stream, 200, &[], TEST_BYTES);
        });
        let spec = spec_for(&server.base, TEST_BYTES);

        let result = fetch(&spec, tmp.path(), false).unwrap();

        let (path, bytes, sha256) = match result {
            FetchResult::Downloaded {
                path,
                bytes,
                sha256,
            } => (path, bytes, sha256),
            other => panic!("expected Downloaded, got {other:?}"),
        };
        assert_eq!(bytes, TEST_BYTES.len() as u64);
        assert_eq!(sha256, spec.sha256);
        assert_eq!(std::fs::read(&path).unwrap(), TEST_BYTES);
        assert!(!part_path(&path).exists());
        assert!(server.requests() == 1);
        // The installed file verifies against the spec.
        verify_file(&path, &spec).unwrap();
    }

    #[test]
    fn wrong_hash_fails_and_cleans_up() {
        let tmp = private_dir();
        let server = serve(|_, stream| respond(stream, 200, &[], TEST_BYTES));
        let mut spec = spec_for(&server.base, TEST_BYTES);
        spec.sha256 = "0".repeat(64);

        let err = fetch(&spec, tmp.path(), false).unwrap_err();
        assert_eq!(err.code, ErrorCode::ChecksumMismatch);

        let dest = tmp.path().join("model.gguf");
        assert!(!dest.exists());
        assert!(!part_path(&dest).exists());
    }

    #[test]
    fn too_many_bytes_fails_and_cleans_up() {
        let tmp = private_dir();
        let mut bytes = TEST_BYTES.to_vec();
        bytes.extend_from_slice(b"extra");
        let server = serve(move |_, stream| {
            // No Content-Length: the excess is detected while streaming.
            use std::io::Write as _;
            stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").unwrap();
            stream.write_all(&bytes).unwrap();
        });
        let spec = spec_for(&server.base, TEST_BYTES); // expects fewer bytes than served

        let err = fetch(&spec, tmp.path(), false).unwrap_err();
        assert_eq!(err.code, ErrorCode::DownloadFailed);
        assert!(err.message.contains("exceeded"));

        let dest = tmp.path().join("model.gguf");
        assert!(!dest.exists());
        assert!(!part_path(&dest).exists());
    }

    #[test]
    fn truncated_stream_fails_and_cleans_up() {
        let tmp = private_dir();
        let server = serve(|_, stream| {
            // Announce the full length but close after half the bytes.
            use std::io::Write as _;
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n",
                TEST_BYTES.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            let half = TEST_BYTES.len() / 2;
            stream.write_all(&TEST_BYTES[..half]).unwrap();
        });
        let spec = spec_for(&server.base, TEST_BYTES);

        let err = fetch(&spec, tmp.path(), false).unwrap_err();
        assert_eq!(err.code, ErrorCode::DownloadFailed);

        let dest = tmp.path().join("model.gguf");
        assert!(!dest.exists());
        assert!(!part_path(&dest).exists());
    }

    #[test]
    fn wrong_content_length_fails_early() {
        let tmp = private_dir();
        let server = serve(|_, stream| {
            // Body matches the spec but the announced length does not.
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n",
                TEST_BYTES.len() + 1
            );
            use std::io::Write as _;
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(TEST_BYTES).unwrap();
        });
        let spec = spec_for(&server.base, TEST_BYTES);

        let err = fetch(&spec, tmp.path(), false).unwrap_err();
        assert_eq!(err.code, ErrorCode::DownloadFailed);
        assert!(err.message.contains("announced"));
        assert!(!tmp.path().join("model.gguf").exists());
    }

    #[test]
    fn http_error_status_fails() {
        for status in [404u16, 500] {
            let tmp = private_dir();
            let server = serve(move |_, stream| respond(stream, status, &[], b"nope"));
            let spec = spec_for(&server.base, TEST_BYTES);

            let err = fetch(&spec, tmp.path(), false).unwrap_err();
            assert_eq!(err.code, ErrorCode::DownloadFailed);
            assert!(err.message.contains(&status.to_string()));
            assert!(!tmp.path().join("model.gguf").exists());
        }
    }

    #[test]
    fn https_only_policy_refuses_http_redirect() {
        let tmp = private_dir();
        let server = serve(|path, stream| match path {
            "/file" => respond(stream, 302, &[("Location", "/elsewhere")], b""),
            "/elsewhere" => respond(stream, 200, &[], TEST_BYTES),
            other => panic!("unexpected path {other}"),
        });
        let spec = spec_for(&server.base, TEST_BYTES);

        let err = Downloader::new(RedirectPolicy::HttpsOnly, test_timeouts())
            .fetch(&spec, tmp.path(), false, &mut |_| {})
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::DownloadFailed);
        assert!(err.message.contains("non-HTTPS"));
    }

    #[test]
    fn http_redirect_is_followed_under_test_policy() {
        let tmp = private_dir();
        let server = serve(|path, stream| match path {
            "/file" => respond(stream, 302, &[("Location", "/elsewhere")], b""),
            "/elsewhere" => respond(stream, 200, &[], TEST_BYTES),
            other => panic!("unexpected path {other}"),
        });
        let spec = spec_for(&server.base, TEST_BYTES);

        let result = fetch(&spec, tmp.path(), false).unwrap();
        assert!(matches!(result, FetchResult::Downloaded { .. }));
        assert_eq!(server.requests(), 2);
    }

    #[test]
    fn relative_redirect_is_resolved() {
        let tmp = private_dir();
        let server = serve(|path, stream| match path {
            "/file" => respond(stream, 307, &[("Location", "./target")], b""),
            "/target" => respond(stream, 200, &[], TEST_BYTES),
            other => panic!("unexpected path {other}"),
        });
        let spec = spec_for(&server.base, TEST_BYTES);

        assert!(matches!(
            fetch(&spec, tmp.path(), false).unwrap(),
            FetchResult::Downloaded { .. }
        ));
    }

    #[test]
    fn redirect_chain_over_limit_fails() {
        let tmp = private_dir();
        let server = serve(|path, stream| {
            let next: u32 = path.trim_start_matches("/r").parse().unwrap_or(0);
            let location = format!("/r{}", next + 1);
            respond(stream, 302, &[("Location", &location)], b"");
        });
        let spec = spec_for(&server.base, TEST_BYTES);

        let err = fetch(&spec, tmp.path(), false).unwrap_err();
        assert_eq!(err.code, ErrorCode::DownloadFailed);
        assert!(err.message.contains("redirects"));
    }

    #[test]
    fn concurrent_download_fails_fast() {
        let tmp = private_dir();
        // The handler reports the first transfer (the download took its
        // lock before requesting) and holds it until released, bounded.
        let (transferring, transfer) = mpsc::channel();
        let (release, released) = mpsc::channel::<()>();
        let transferring = Mutex::new(transferring);
        let released = Mutex::new(released);
        let server = serve(move |_, stream| {
            use std::io::Write as _;
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n",
                TEST_BYTES.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(&TEST_BYTES[..1]).unwrap();
            transferring.lock().unwrap().send(()).unwrap();
            released
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .expect("released by the test");
            stream.write_all(&TEST_BYTES[1..]).unwrap();
        });
        let spec = spec_for(&server.base, TEST_BYTES);
        let spec2 = spec.clone();
        let dir = tmp.path().to_path_buf();

        let first = std::thread::spawn(move || {
            Downloader::new(RedirectPolicy::AllowHttp, test_timeouts()).fetch(
                &spec2,
                &dir,
                false,
                &mut |_| {},
            )
        });
        transfer
            .recv_timeout(Duration::from_secs(10))
            .expect("the first download started its transfer");
        let err = fetch(&spec, tmp.path(), false).unwrap_err();
        release.send(()).unwrap();
        assert_eq!(err.code, ErrorCode::DownloadFailed);
        assert!(err.message.contains("already in progress"));
        assert_eq!(server.requests(), 1, "the second never connected");

        assert!(matches!(
            first.join().unwrap().unwrap(),
            FetchResult::Downloaded { .. }
        ));
    }

    #[test]
    fn existing_verified_file_is_already_installed() {
        let tmp = private_dir();
        let server = serve(|_, _stream| {
            panic!("the server must not be contacted");
        });
        let spec = spec_for(&server.base, TEST_BYTES);
        std::fs::write(tmp.path().join("model.gguf"), TEST_BYTES).unwrap();

        let result = fetch(&spec, tmp.path(), false).unwrap();
        let (path, bytes) = match result {
            FetchResult::AlreadyInstalled { path, bytes, .. } => (path, bytes),
            other => panic!("expected AlreadyInstalled, got {other:?}"),
        };
        assert_eq!(bytes, TEST_BYTES.len() as u64);
        assert_eq!(std::fs::read(path).unwrap(), TEST_BYTES);
        assert_eq!(server.requests(), 0);
    }

    #[test]
    fn existing_corrupt_file_requires_force() {
        let tmp = private_dir();
        let server = serve(|_, stream| respond(stream, 200, &[], TEST_BYTES));
        let spec = spec_for(&server.base, TEST_BYTES);
        std::fs::write(tmp.path().join("model.gguf"), b"corrupt").unwrap();

        let err = fetch(&spec, tmp.path(), false).unwrap_err();
        assert_eq!(err.code, ErrorCode::ChecksumMismatch);
        assert!(err.message.contains("--force"));

        // The corrupt file is untouched without --force.
        assert_eq!(
            std::fs::read(tmp.path().join("model.gguf")).unwrap(),
            b"corrupt"
        );

        let result = fetch(&spec, tmp.path(), true).unwrap();
        assert!(matches!(result, FetchResult::Downloaded { .. }));
        assert_eq!(
            std::fs::read(tmp.path().join("model.gguf")).unwrap(),
            TEST_BYTES
        );
    }

    #[test]
    fn stale_part_from_crashed_download_is_replaced() {
        let tmp = private_dir();
        let server = serve(|_, stream| respond(stream, 200, &[], TEST_BYTES));
        let spec = spec_for(&server.base, TEST_BYTES);
        let dest = tmp.path().join("model.gguf");
        std::fs::write(part_path(&dest), b"leftover bytes").unwrap();

        let result = fetch(&spec, tmp.path(), false).unwrap();
        assert!(matches!(result, FetchResult::Downloaded { .. }));
        assert!(!part_path(&dest).exists());
    }

    #[test]
    fn verify_file_checks_size_and_hash() {
        let tmp = private_dir();
        let path = tmp.path().join("model.gguf");
        let spec = DownloadSpec {
            url: "http://127.0.0.1:1/file".to_string(),
            file_name: "model.gguf".to_string(),
            size_bytes: TEST_BYTES.len() as u64,
            sha256: sha256_of(TEST_BYTES),
        };

        assert_eq!(
            verify_file(&path, &spec).unwrap_err().code,
            ErrorCode::ModelNotFound
        );

        std::fs::write(&path, b"short").unwrap();
        let err = verify_file(&path, &spec).unwrap_err();
        assert_eq!(err.code, ErrorCode::ChecksumMismatch);
        assert!(err.message.contains("bytes"));

        std::fs::write(&path, TEST_BYTES).unwrap();
        let verified = verify_file(&path, &spec).unwrap();
        assert_eq!(verified.bytes, TEST_BYTES.len() as u64);
        assert_eq!(verified.sha256, spec.sha256);

        std::fs::write(&path, vec![b'x'; TEST_BYTES.len()]).unwrap();
        assert_eq!(
            verify_file(&path, &spec).unwrap_err().code,
            ErrorCode::ChecksumMismatch
        );
    }

    #[test]
    fn progress_receives_bytes_and_total() {
        let tmp = private_dir();
        let chunk = vec![7u8; 100_000];
        let served = chunk.clone();
        let server = serve(move |_, stream| respond(stream, 200, &[], &served));
        let spec = spec_for(&server.base, &chunk);

        let mut seen: Vec<(u64, u64)> = Vec::new();
        Downloader::new(RedirectPolicy::AllowHttp, test_timeouts())
            .fetch(&spec, tmp.path(), false, &mut |event| {
                if let DownloadEvent::Progress { received, total } = event {
                    seen.push((received, total));
                }
            })
            .unwrap();

        assert!(!seen.is_empty());
        assert!(seen.iter().all(|&(_, total)| total == chunk.len() as u64));
        assert!(seen.iter().all(|&(bytes, _)| bytes <= chunk.len() as u64));
        assert_eq!(seen.last().unwrap().0, chunk.len() as u64);
    }

    #[test]
    fn download_emits_state_events_before_each_phase() {
        let tmp = private_dir();
        let server = serve(|_, stream| respond(stream, 200, &[], TEST_BYTES));
        let spec = spec_for(&server.base, TEST_BYTES);

        let mut states = Vec::new();
        Downloader::new(RedirectPolicy::AllowHttp, test_timeouts())
            .fetch(&spec, tmp.path(), false, &mut |event| {
                if let DownloadEvent::State(state) = event {
                    states.push(state);
                }
            })
            .unwrap();

        assert_eq!(
            states,
            vec![
                DownloadState::Connecting,
                DownloadState::Downloading,
                DownloadState::Verifying,
                DownloadState::Installing,
            ]
        );
    }

    #[test]
    fn already_installed_emits_no_state_events() {
        let tmp = private_dir();
        let server = serve(|_, _stream| {
            panic!("the server must not be contacted");
        });
        let spec = spec_for(&server.base, TEST_BYTES);
        std::fs::write(tmp.path().join("model.gguf"), TEST_BYTES).unwrap();

        let mut states = Vec::new();
        let result = Downloader::new(RedirectPolicy::AllowHttp, test_timeouts())
            .fetch(&spec, tmp.path(), false, &mut |event| {
                if let DownloadEvent::State(state) = event {
                    states.push(state);
                }
            })
            .unwrap();
        assert!(matches!(result, FetchResult::AlreadyInstalled { .. }));
        assert!(states.is_empty());
    }

    #[test]
    fn download_exceeding_total_deadline_fails_and_cleans_up() {
        let tmp = private_dir();
        let server = serve(|_, stream| {
            use std::io::Write as _;
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n",
                TEST_BYTES.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            // A steady trickle: no socket stall, but far too slow overall.
            // It ends when the client gives up.
            for &byte in TEST_BYTES {
                if stream.write_all(&[byte]).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let spec = spec_for(&server.base, TEST_BYTES);
        let timeouts = Timeouts {
            connect: Duration::from_secs(5),
            total: Duration::from_millis(200),
        };

        let started = std::time::Instant::now();
        let err = Downloader::new(RedirectPolicy::AllowHttp, timeouts)
            .fetch(&spec, tmp.path(), false, &mut |_| {})
            .unwrap_err();

        // The overall deadline trips in the HTTP layer (the per-request
        // timeout) or in the read loop; either is a typed failure with full
        // cleanup, and neither waits for the trickle to finish.
        assert_eq!(err.code, ErrorCode::DownloadFailed);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a body slower than the budget must not run to completion: {:?}",
            started.elapsed()
        );
        let dest = tmp.path().join("model.gguf");
        assert!(!dest.exists());
        assert!(!part_path(&dest).exists());
    }

    /// Header fragments that each arrive quickly but add up past the overall
    /// deadline must fail: the deadline is applied to the whole request
    /// (headers included), not polled only between body reads.
    #[test]
    fn header_trickle_past_deadline_fails_and_cleans_up() {
        let tmp = private_dir();
        let server = serve(|_, stream| {
            use std::io::Write as _;
            // It ends when the client gives up.
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\n");
            for i in 0..10 {
                let line = format!("x-trickle-{i}: {i}\r\n");
                if stream.write_all(line.as_bytes()).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(300));
            }
            let _ = stream
                .write_all(format!("content-length: {}\r\n\r\n", TEST_BYTES.len()).as_bytes());
            let _ = stream.write_all(TEST_BYTES);
        });
        let spec = spec_for(&server.base, TEST_BYTES);
        let timeouts = Timeouts {
            connect: Duration::from_secs(5),
            total: Duration::from_millis(200),
        };

        let err = Downloader::new(RedirectPolicy::AllowHttp, timeouts)
            .fetch(&spec, tmp.path(), false, &mut |_| {})
            .unwrap_err();

        assert_eq!(err.code, ErrorCode::DownloadFailed);
        let dest = tmp.path().join("model.gguf");
        assert!(!dest.exists(), "a late response must not install");
        assert!(!part_path(&dest).exists());
    }

    /// A close-delimited body whose bytes all arrive quickly but whose EOF
    /// arrives after the overall deadline must not install: the transfer
    /// only completes once the deadline has already expired, and the
    /// pre-install recheck refuses to rename it into place.
    #[test]
    fn body_completing_after_deadline_is_not_installed() {
        let tmp = private_dir();
        let server = serve(|_, stream| {
            use std::io::Write as _;
            // No Content-Length: the body ends at EOF, which is delayed
            // beyond the deadline even though every byte arrived early.
            stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").unwrap();
            stream.write_all(TEST_BYTES).unwrap();
            stream.flush().unwrap();
            std::thread::sleep(Duration::from_millis(2500));
        });
        let spec = spec_for(&server.base, TEST_BYTES);
        let timeouts = Timeouts {
            connect: Duration::from_secs(5),
            total: Duration::from_millis(200),
        };

        let err = Downloader::new(RedirectPolicy::AllowHttp, timeouts)
            .fetch(&spec, tmp.path(), false, &mut |_| {})
            .unwrap_err();

        assert_eq!(err.code, ErrorCode::DownloadFailed);
        let dest = tmp.path().join("model.gguf");
        assert!(!dest.exists(), "an over-deadline body must not install");
        assert!(!part_path(&dest).exists());
    }

    /// The per-download deadline is one absolute budget: every redirect hop
    /// is a fresh request, and each gets only the time remaining — a chain
    /// of slow redirects must fail near the budget, not after summing the
    /// hops.
    #[test]
    fn redirect_chain_cannot_extend_deadline() {
        let tmp = private_dir();
        let server = serve(|path, stream| {
            std::thread::sleep(Duration::from_millis(700));
            match path {
                "/file" => respond(stream, 302, &[("Location", "/hop2")], b""),
                "/hop2" => respond(stream, 302, &[("Location", "/hop3")], b""),
                "/hop3" => respond(stream, 200, &[], TEST_BYTES),
                other => panic!("unexpected path {other}"),
            }
        });
        let spec = spec_for(&server.base, TEST_BYTES);
        let timeouts = Timeouts {
            connect: Duration::from_secs(5),
            total: Duration::from_secs(1),
        };

        let started = std::time::Instant::now();
        let err = Downloader::new(RedirectPolicy::AllowHttp, timeouts)
            .fetch(&spec, tmp.path(), false, &mut |_| {})
            .unwrap_err();

        assert_eq!(err.code, ErrorCode::DownloadFailed);
        assert!(
            started.elapsed() < Duration::from_millis(1800),
            "the redirect chain extended the deadline: {:?}",
            started.elapsed()
        );
        let dest = tmp.path().join("model.gguf");
        assert!(!dest.exists());
        assert!(!part_path(&dest).exists());
        // The second hop was cut off by the remaining budget; the third
        // request never happened.
        assert!(
            server.requests() < 3,
            "the redirect chain must not issue requests past the budget"
        );
    }

    /// Direct check of the deadline guard that runs right before
    /// installation: an expired budget fails even when the transfer itself
    /// produced no error.
    #[test]
    fn expired_deadline_blocks_installation() {
        let started = std::time::Instant::now();
        check_deadline(started, Duration::from_secs(60), TEST_BYTES.len() as u64).unwrap();

        std::thread::sleep(Duration::from_millis(20));
        let err =
            check_deadline(started, Duration::from_millis(5), TEST_BYTES.len() as u64).unwrap_err();
        assert_eq!(err.code, ErrorCode::DownloadFailed);
        assert!(err.message.contains("time budget"));
    }

    #[cfg(unix)]
    #[test]
    fn refuses_group_writable_models_directory() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = private_dir();
        let shared = tmp.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o775)).unwrap();
        let server = serve(|_, _stream| {
            panic!("the server must not be contacted");
        });
        let spec = spec_for(&server.base, TEST_BYTES);

        let err = fetch(&spec, &shared, false).unwrap_err();

        assert_eq!(err.code, ErrorCode::DownloadFailed);
        assert!(
            err.message.contains("shared directory"),
            "unexpected message: {}",
            err.message
        );
        assert_eq!(server.requests(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn creates_models_directory_and_files_private() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = private_dir();
        let nested = tmp.path().join("a").join("b").join("models");
        let server = serve(|_, stream| respond(stream, 200, &[], TEST_BYTES));
        let spec = spec_for(&server.base, TEST_BYTES);

        fetch(&spec, &nested, false).unwrap();

        for dir in [tmp.path().join("a"), tmp.path().join("a/b"), nested.clone()] {
            let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "{} must be private", dir.display());
        }
        let dest = nested.join("model.gguf");
        let mode = std::fs::metadata(&dest).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "installed file must be private");
        let mode = std::fs::metadata(lock_path(&dest))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "lock file must be private");
    }

    /// The review's substitution probe: while bytes stream, another writer of
    /// the models directory moves the real `.part` aside and drops a
    /// different file under the staging name. The rename then installs the
    /// unverified file; the post-install re-hash must catch that, remove the
    /// destination and fail.
    #[cfg(unix)]
    #[test]
    fn swapped_part_during_download_is_detected_and_rejected() {
        let tmp = private_dir();
        let server = serve(|_, stream| {
            use std::io::Write as _;
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n",
                TEST_BYTES.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            for &byte in TEST_BYTES {
                stream.write_all(&[byte]).unwrap();
                stream.flush().unwrap();
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let spec = spec_for(&server.base, TEST_BYTES);
        let dest = tmp.path().join("model.gguf");
        let aside = tmp.path().join("moved-aside.part");
        let mut swapped = false;

        let err = Downloader::new(RedirectPolicy::AllowHttp, test_timeouts())
            .fetch(&spec, tmp.path(), false, &mut |event| {
                if swapped {
                    return;
                }
                if let DownloadEvent::Progress { received: 1, .. } = event {
                    std::fs::rename(part_path(&dest), &aside).unwrap();
                    std::fs::write(part_path(&dest), b"UNVERIFIED").unwrap();
                    swapped = true;
                }
            })
            .unwrap_err();

        assert!(swapped, "the probe must have replaced the staging file");
        assert_eq!(err.code, ErrorCode::ChecksumMismatch);
        assert!(
            err.message.contains("post-install verification"),
            "unexpected message: {}",
            err.message
        );
        assert!(!dest.exists(), "the unverified file must be removed");
    }

    /// The review's lock probe: a pre-existing `<model>.lock` symlink must not
    /// be followed (let alone truncated), even when the command then refuses
    /// an existing corrupt destination.
    #[cfg(unix)]
    #[test]
    fn symlinked_lock_file_is_rejected_without_touching_its_target() {
        let tmp = private_dir();
        let server = serve(|_, stream| respond(stream, 200, &[], TEST_BYTES));
        let spec = spec_for(&server.base, TEST_BYTES);
        let dest = tmp.path().join("model.gguf");
        let sentinel = tmp.path().join("sentinel.txt");
        std::fs::write(&sentinel, b"do not erase").unwrap();
        std::os::unix::fs::symlink(&sentinel, lock_path(&dest)).unwrap();
        std::fs::write(&dest, b"corrupt").unwrap();

        let err = fetch(&spec, tmp.path(), false).unwrap_err();

        assert_eq!(err.code, ErrorCode::DownloadFailed);
        assert!(
            err.message.contains("lock file"),
            "unexpected message: {}",
            err.message
        );
        assert_eq!(
            std::fs::read(&sentinel).unwrap(),
            b"do not erase",
            "the symlinked file must be untouched"
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"corrupt");
    }

    /// The review's raced-in reparse-point probe (Windows): the post-install
    /// reopen must open the installed leaf without following links. A
    /// symlink whose target has the exact catalog bytes must still fail the
    /// post-install verification. Skipped when the sandbox lacks the
    /// privilege to create symlinks.
    #[cfg(windows)]
    #[test]
    fn symlinked_install_is_rejected_even_with_matching_target_bytes() {
        let tmp = private_dir();
        let target = tmp.path().join("target.gguf");
        std::fs::write(&target, TEST_BYTES).unwrap();
        let dest = tmp.path().join("model.gguf");
        if std::os::windows::fs::symlink_file(&target, &dest).is_err() {
            // Past libtest's output capture, so the skip shows in CI logs.
            use std::io::Write as _;
            let _ = writeln!(
                std::io::stderr(),
                "skipped symlinked_install_is_rejected_even_with_matching_target_bytes: \
                 no privilege to create symlinks"
            );
            return;
        }
        let spec = DownloadSpec {
            url: "http://127.0.0.1:1/file".to_string(),
            file_name: "model.gguf".to_string(),
            size_bytes: TEST_BYTES.len() as u64,
            sha256: sha256_of(TEST_BYTES),
        };

        let err = confirm_installed_file(&dest, &spec).unwrap_err();

        assert_eq!(err.code, ErrorCode::DownloadFailed);
        assert!(
            err.message.contains("post-install verification"),
            "unexpected message: {}",
            err.message
        );
        assert!(
            std::fs::symlink_metadata(&dest).is_err(),
            "the link itself must be removed, not the target"
        );
        assert_eq!(std::fs::read(&target).unwrap(), TEST_BYTES);
    }
}
