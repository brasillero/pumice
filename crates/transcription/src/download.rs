//! Explicit, pinned model downloads (`arch-modules/D9`, task
//! `p9-transcribe-downloader` decisions D1–D3).
//!
//! Downloads only ever happen because the user ran `models download`; nothing
//! is fetched at startup or because of a request. Bytes stream into a
//! `<file>.part` file created exclusively, are hashed while writing and are
//! renamed into place only when both the byte size and the SHA-256 from the
//! catalog match. Any failure deletes the partial file and restarts from
//! zero next time — there is no resume (D3).
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

/// Buffer size while streaming bytes to disk.
const BUFFER_SIZE: usize = 64 * 1024;

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
    /// Maximum stall between received chunks.
    pub read_stall: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(30),
            read_stall: Duration::from_secs(60),
        }
    }
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
    agent: ureq::Agent,
    redirect_policy: RedirectPolicy,
}

impl Downloader {
    pub fn new(redirect_policy: RedirectPolicy, timeouts: Timeouts) -> Self {
        let agent = ureq::AgentBuilder::new()
            .user_agent(concat!("pumice-transcribe/", env!("CARGO_PKG_VERSION")))
            .timeout_connect(timeouts.connect)
            .timeout_read(timeouts.read_stall)
            .redirects(0)
            .build();
        Self {
            agent,
            redirect_policy,
        }
    }

    /// Downloads `spec` into `models_dir`, unless a verified file is already
    /// installed. `on_progress` is called with `(bytes_received, total)` as
    /// bytes arrive; throttling is the caller's job.
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
        on_progress: &mut dyn FnMut(u64, u64),
    ) -> Result<FetchResult, Error> {
        std::fs::create_dir_all(models_dir).map_err(|e| {
            Error::download_failed(format!("cannot create {}: {e}", models_dir.display()))
        })?;
        let dest = models_dir.join(&spec.file_name);

        // Per-model lock, released by the OS if this process dies. The lock
        // file persists (it holds no data); only the guard matters.
        let lock_path = lock_path(&dest);
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&lock_path)
            .map_err(|e| {
                Error::download_failed(format!("cannot open {}: {e}", lock_path.display()))
            })?;
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
                    lock_path.display()
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

        self.download_locked(spec, &dest, on_progress)
    }

    /// Streams the download into `dest.part` and renames it into place only
    /// when size and SHA-256 match. On any error the partial file is removed
    /// (D3): a partial file never looks installed.
    fn download_locked(
        &self,
        spec: &DownloadSpec,
        dest: &Path,
        on_progress: &mut dyn FnMut(u64, u64),
    ) -> Result<FetchResult, Error> {
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

        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)
            .map_err(|e| {
                Error::download_failed(format!("cannot create {}: {e}", part.display()))
            })?;

        let outcome = (|| {
            let sha256 = self.stream_to_file(spec, &mut file, on_progress)?;
            file.sync_all().map_err(|e| {
                Error::download_failed(format!("cannot sync {}: {e}", part.display()))
            })?;
            std::fs::rename(&part, dest).map_err(|e| {
                Error::download_failed(format!("cannot move {} into place: {e}", part.display()))
            })?;
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
                Err(e)
            }
        }
    }

    /// Performs the HTTP GET (following redirects by hand, so the policy and
    /// the limit are testable) and writes the body to `file`, hashing on the
    /// way. Enforces the exact expected size: more bytes, fewer bytes or a
    /// wrong `Content-Length` are all failures.
    fn stream_to_file(
        &self,
        spec: &DownloadSpec,
        file: &mut File,
        on_progress: &mut dyn FnMut(u64, u64),
    ) -> Result<String, Error> {
        let mut url = url::Url::parse(&spec.url)
            .map_err(|e| Error::download_failed(format!("invalid download URL: {e}")))?;
        let mut redirects = 0u32;
        let response = loop {
            let (status, response) = match self.agent.get(url.as_str()).call() {
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

        let mut reader = response.into_reader();
        let mut hasher = sha2::Sha256::new();
        let mut written = 0u64;
        let mut buffer = vec![0u8; BUFFER_SIZE];
        loop {
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
            on_progress(written, spec.size_bytes);
        }
        if written != spec.size_bytes {
            return Err(Error::download_failed(format!(
                "download ended at {written} bytes but the catalog expects {}",
                spec.size_bytes
            )));
        }

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
    let mut hasher = sha2::Sha256::new();
    let mut buffer = vec![0u8; BUFFER_SIZE];
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|e| Error::download_failed(format!("cannot read {}: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    let digest = hex_lower(&hasher.finalize());
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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const TEST_BYTES: &[u8] = b"the quick brown fox jumps over the lazy dog";

    fn sha256_of(bytes: &[u8]) -> String {
        let mut hasher = sha2::Sha256::new();
        hasher.update(bytes);
        hex_lower(&hasher.finalize())
    }

    /// A response a fake handler writes to one connection.
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
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
    }

    /// Serves `handler` on 127.0.0.1:0 in the background until dropped.
    /// Returns the base URL and a request counter. Only for tests: reads one
    /// request per connection and always answers HTTP/1.1 with connection
    /// close.
    fn serve(
        handler: impl Fn(&str, &mut TcpStream) + Send + Sync + 'static,
    ) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&requests);
        let handler = Arc::new(handler);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = match stream {
                    Ok(stream) => stream,
                    Err(_) => continue,
                };
                let counter = Arc::clone(&counter);
                let handler = Arc::clone(&handler);
                std::thread::spawn(move || {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let path = read_request_path(&mut stream);
                    handler(&path, &mut stream);
                });
            }
        });
        (format!("http://{addr}"), requests)
    }

    fn read_request_path(stream: &mut TcpStream) -> String {
        use std::io::BufRead;
        let mut reader = io::BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
        // Consume the remaining headers.
        loop {
            line.clear();
            reader.read_line(&mut line).unwrap();
            if line.trim_end().is_empty() {
                break;
            }
        }
        path
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
            read_stall: Duration::from_secs(5),
        }
    }

    fn fetch(spec: &DownloadSpec, dir: &Path, force: bool) -> Result<FetchResult, Error> {
        Downloader::new(RedirectPolicy::AllowHttp, test_timeouts()).fetch(
            spec,
            dir,
            force,
            &mut |_, _| {},
        )
    }

    #[test]
    fn download_installs_verified_file() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, requests) = serve(|path, stream| {
            assert_eq!(path, "/file");
            respond(stream, 200, &[], TEST_BYTES);
        });
        let spec = spec_for(&base, TEST_BYTES);

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
        assert!(requests.load(Ordering::SeqCst) == 1);
        // The installed file verifies against the spec.
        verify_file(&path, &spec).unwrap();
    }

    #[test]
    fn wrong_hash_fails_and_cleans_up() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, _) = serve(|_, stream| respond(stream, 200, &[], TEST_BYTES));
        let mut spec = spec_for(&base, TEST_BYTES);
        spec.sha256 = "0".repeat(64);

        let err = fetch(&spec, tmp.path(), false).unwrap_err();
        assert_eq!(err.code, ErrorCode::ChecksumMismatch);

        let dest = tmp.path().join("model.gguf");
        assert!(!dest.exists());
        assert!(!part_path(&dest).exists());
    }

    #[test]
    fn too_many_bytes_fails_and_cleans_up() {
        let tmp = tempfile::tempdir().unwrap();
        let mut bytes = TEST_BYTES.to_vec();
        bytes.extend_from_slice(b"extra");
        let (base, _) = serve(move |_, stream| {
            // No Content-Length: the excess is detected while streaming.
            use std::io::Write as _;
            stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").unwrap();
            stream.write_all(&bytes).unwrap();
        });
        let spec = spec_for(&base, TEST_BYTES); // expects fewer bytes than served

        let err = fetch(&spec, tmp.path(), false).unwrap_err();
        assert_eq!(err.code, ErrorCode::DownloadFailed);
        assert!(err.message.contains("exceeded"));

        let dest = tmp.path().join("model.gguf");
        assert!(!dest.exists());
        assert!(!part_path(&dest).exists());
    }

    #[test]
    fn truncated_stream_fails_and_cleans_up() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, _) = serve(|_, stream| {
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
        let spec = spec_for(&base, TEST_BYTES);

        let err = fetch(&spec, tmp.path(), false).unwrap_err();
        assert_eq!(err.code, ErrorCode::DownloadFailed);

        let dest = tmp.path().join("model.gguf");
        assert!(!dest.exists());
        assert!(!part_path(&dest).exists());
    }

    #[test]
    fn wrong_content_length_fails_early() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, _) = serve(|_, stream| {
            // Body matches the spec but the announced length does not.
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n",
                TEST_BYTES.len() + 1
            );
            use std::io::Write as _;
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(TEST_BYTES).unwrap();
        });
        let spec = spec_for(&base, TEST_BYTES);

        let err = fetch(&spec, tmp.path(), false).unwrap_err();
        assert_eq!(err.code, ErrorCode::DownloadFailed);
        assert!(err.message.contains("announced"));
        assert!(!tmp.path().join("model.gguf").exists());
    }

    #[test]
    fn http_error_status_fails() {
        for status in [404u16, 500] {
            let tmp = tempfile::tempdir().unwrap();
            let (base, _) = serve(move |_, stream| respond(stream, status, &[], b"nope"));
            let spec = spec_for(&base, TEST_BYTES);

            let err = fetch(&spec, tmp.path(), false).unwrap_err();
            assert_eq!(err.code, ErrorCode::DownloadFailed);
            assert!(err.message.contains(&status.to_string()));
            assert!(!tmp.path().join("model.gguf").exists());
        }
    }

    #[test]
    fn https_only_policy_refuses_http_redirect() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, _) = serve(|path, stream| match path {
            "/file" => respond(stream, 302, &[("Location", "/elsewhere")], b""),
            "/elsewhere" => respond(stream, 200, &[], TEST_BYTES),
            other => panic!("unexpected path {other}"),
        });
        let spec = spec_for(&base, TEST_BYTES);

        let err = Downloader::new(RedirectPolicy::HttpsOnly, test_timeouts())
            .fetch(&spec, tmp.path(), false, &mut |_, _| {})
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::DownloadFailed);
        assert!(err.message.contains("non-HTTPS"));
    }

    #[test]
    fn http_redirect_is_followed_under_test_policy() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, requests) = serve(|path, stream| match path {
            "/file" => respond(stream, 302, &[("Location", "/elsewhere")], b""),
            "/elsewhere" => respond(stream, 200, &[], TEST_BYTES),
            other => panic!("unexpected path {other}"),
        });
        let spec = spec_for(&base, TEST_BYTES);

        let result = fetch(&spec, tmp.path(), false).unwrap();
        assert!(matches!(result, FetchResult::Downloaded { .. }));
        assert_eq!(requests.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn relative_redirect_is_resolved() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, _) = serve(|path, stream| match path {
            "/file" => respond(stream, 307, &[("Location", "./target")], b""),
            "/target" => respond(stream, 200, &[], TEST_BYTES),
            other => panic!("unexpected path {other}"),
        });
        let spec = spec_for(&base, TEST_BYTES);

        assert!(matches!(
            fetch(&spec, tmp.path(), false).unwrap(),
            FetchResult::Downloaded { .. }
        ));
    }

    #[test]
    fn redirect_chain_over_limit_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, _) = serve(|path, stream| {
            let next: u32 = path.trim_start_matches("/r").parse().unwrap_or(0);
            let location = format!("/r{}", next + 1);
            respond(stream, 302, &[("Location", &location)], b"");
        });
        let spec = spec_for(&base, TEST_BYTES);

        let err = fetch(&spec, tmp.path(), false).unwrap_err();
        assert_eq!(err.code, ErrorCode::DownloadFailed);
        assert!(err.message.contains("redirects"));
    }

    #[test]
    fn concurrent_download_fails_fast() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, _) = serve(|_, stream| {
            use std::io::Write as _;
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n",
                TEST_BYTES.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(&TEST_BYTES[..1]).unwrap();
            // Hold the transfer so the first download keeps the lock.
            std::thread::sleep(Duration::from_secs(3));
            stream.write_all(&TEST_BYTES[1..]).unwrap();
        });
        let spec = spec_for(&base, TEST_BYTES);
        let spec2 = spec.clone();
        let dir = tmp.path().to_path_buf();

        let first = std::thread::spawn(move || {
            Downloader::new(RedirectPolicy::AllowHttp, test_timeouts()).fetch(
                &spec2,
                &dir,
                false,
                &mut |_, _| {},
            )
        });
        // Give the first download a moment to take the lock.
        std::thread::sleep(Duration::from_millis(300));
        let err = fetch(&spec, tmp.path(), false).unwrap_err();
        assert_eq!(err.code, ErrorCode::DownloadFailed);
        assert!(err.message.contains("already in progress"));

        assert!(matches!(
            first.join().unwrap().unwrap(),
            FetchResult::Downloaded { .. }
        ));
    }

    #[test]
    fn existing_verified_file_is_already_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, requests) = serve(|_, _stream| {
            panic!("the server must not be contacted");
        });
        let spec = spec_for(&base, TEST_BYTES);
        std::fs::write(tmp.path().join("model.gguf"), TEST_BYTES).unwrap();

        let result = fetch(&spec, tmp.path(), false).unwrap();
        let (path, bytes) = match result {
            FetchResult::AlreadyInstalled { path, bytes, .. } => (path, bytes),
            other => panic!("expected AlreadyInstalled, got {other:?}"),
        };
        assert_eq!(bytes, TEST_BYTES.len() as u64);
        assert_eq!(std::fs::read(path).unwrap(), TEST_BYTES);
        assert_eq!(requests.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn existing_corrupt_file_requires_force() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, _) = serve(|_, stream| respond(stream, 200, &[], TEST_BYTES));
        let spec = spec_for(&base, TEST_BYTES);
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
        let tmp = tempfile::tempdir().unwrap();
        let (base, _) = serve(|_, stream| respond(stream, 200, &[], TEST_BYTES));
        let spec = spec_for(&base, TEST_BYTES);
        let dest = tmp.path().join("model.gguf");
        std::fs::write(part_path(&dest), b"leftover bytes").unwrap();

        let result = fetch(&spec, tmp.path(), false).unwrap();
        assert!(matches!(result, FetchResult::Downloaded { .. }));
        assert!(!part_path(&dest).exists());
    }

    #[test]
    fn verify_file_checks_size_and_hash() {
        let tmp = tempfile::tempdir().unwrap();
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
        let tmp = tempfile::tempdir().unwrap();
        let chunk = vec![7u8; 100_000];
        let served = chunk.clone();
        let (base, _) = serve(move |_, stream| respond(stream, 200, &[], &served));
        let spec = spec_for(&base, &chunk);

        let mut seen: Vec<(u64, u64)> = Vec::new();
        Downloader::new(RedirectPolicy::AllowHttp, test_timeouts())
            .fetch(&spec, tmp.path(), false, &mut |bytes, total| {
                seen.push((bytes, total));
            })
            .unwrap();

        assert!(!seen.is_empty());
        assert!(seen.iter().all(|&(_, total)| total == chunk.len() as u64));
        assert!(seen.iter().all(|&(bytes, _)| bytes <= chunk.len() as u64));
        assert_eq!(seen.last().unwrap().0, chunk.len() as u64);
    }
}
