//! Bounded waits for process tests (test-only, never released): an owner
//! for a child process and its output pipes, and a loopback HTTP exchange
//! read by its framing. The workspace root's process tests use it too,
//! through `#[path]`, like `install.rs`. Every wait takes a deadline; when one
//! passes, the owned child is killed and the test fails with the stage it
//! had reached, instead of hanging.
//!
//! Each pipe is read by its own thread into a channel, so collecting the
//! output never blocks on a process that still holds a write end (a
//! descendant that inherited it, for example): past the deadline the
//! reader thread is left behind and the test reports what it got.
//!
//! Each test binary uses a subset of these helpers.
#![allow(dead_code)]

use std::cell::RefCell;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{Child, ExitStatus, Output};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

// --- Stage breadcrumbs ---------------------------------------------------------

thread_local! {
    /// The operation the current test thread is in, and the step of it.
    static STAGE: RefCell<(String, &'static str)> = const { RefCell::new((String::new(), "")) };
}

/// Records the operation the current test reached, for failure messages.
pub fn stage(operation: impl Into<String>) {
    STAGE.with(|stage| *stage.borrow_mut() = (operation.into(), ""));
}

fn step(step: &'static str) {
    STAGE.with(|stage| stage.borrow_mut().1 = step);
}

/// The last stage recorded on this thread.
pub fn last_stage() -> String {
    STAGE.with(|stage| {
        let (operation, step) = &*stage.borrow();
        match (operation.is_empty(), step.is_empty()) {
            (true, true) => "none recorded".to_owned(),
            (false, true) => operation.clone(),
            (true, false) => (*step).to_owned(),
            (false, false) => format!("{operation}: {step}"),
        }
    })
}

fn left(deadline: Instant) -> Option<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    (!left.is_zero()).then_some(left)
}

// --- Child processes -----------------------------------------------------------

/// One output pipe, read to its end by its own thread.
struct Pipe {
    chunks: Option<Receiver<Vec<u8>>>,
    bytes: Vec<u8>,
    /// How much of `bytes` [`Pipe::line`] already returned.
    consumed: usize,
}

impl Pipe {
    fn new(source: Option<impl Read + Send + 'static>) -> Self {
        let chunks = source.map(|mut source| {
            let (sender, chunks) = mpsc::channel();
            std::thread::spawn(move || {
                let mut buffer = [0u8; 8192];
                loop {
                    match source.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => {
                            if sender.send(buffer[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        Err(_) => break,
                    }
                }
            });
            chunks
        });
        Pipe {
            chunks,
            bytes: Vec::new(),
            consumed: 0,
        }
    }

    fn at_eof(&self) -> bool {
        self.chunks.is_none()
    }

    /// Takes what already arrived, without waiting.
    fn take_ready(&mut self) {
        if let Some(chunks) = &self.chunks {
            while let Ok(chunk) = chunks.try_recv() {
                self.bytes.extend_from_slice(&chunk);
            }
        }
    }

    /// Takes what arrived, waiting until `deadline` for more. False only
    /// when the deadline passed with the pipe still open.
    fn pull(&mut self, deadline: Instant) -> bool {
        let Some(chunks) = &self.chunks else {
            return true;
        };
        let wait = left(deadline).unwrap_or_default();
        match chunks.recv_timeout(wait) {
            Ok(chunk) => {
                self.bytes.extend_from_slice(&chunk);
                self.take_ready();
                true
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.chunks = None;
                true
            }
            Err(RecvTimeoutError::Timeout) => false,
        }
    }

    /// The next line without its newline; `Ok(None)` at the end of the
    /// pipe, `Err(())` past the deadline.
    fn line(&mut self, deadline: Instant) -> Result<Option<String>, ()> {
        loop {
            let rest = &self.bytes[self.consumed..];
            if let Some(end) = rest.iter().position(|&byte| byte == b'\n') {
                let line = String::from_utf8_lossy(&rest[..end])
                    .trim_end_matches('\r')
                    .to_owned();
                self.consumed += end + 1;
                return Ok(Some(line));
            }
            if self.at_eof() {
                if rest.is_empty() {
                    return Ok(None);
                }
                let line = String::from_utf8_lossy(rest).into_owned();
                self.consumed = self.bytes.len();
                return Ok(Some(line));
            }
            if !self.pull(deadline) {
                return Err(());
            }
        }
    }

    /// Reads to the end of the pipe; false past the deadline.
    fn drain(&mut self, deadline: Instant) -> bool {
        while !self.at_eof() {
            if !self.pull(deadline) {
                return false;
            }
        }
        true
    }
}

/// A child process this test owns, with its stdout and stderr (when
/// piped) collected in the background. Dropping it kills and reaps the
/// child, so a failed assertion leaves nothing running; during a panic it
/// also prints the last stage and the stderr collected so far.
pub struct OwnedChild {
    what: &'static str,
    child: Child,
    stdout: Pipe,
    stderr: Pipe,
    status: Option<ExitStatus>,
}

impl OwnedChild {
    /// Takes ownership of `child` (`what` names it in failures) and starts
    /// reading the pipes it has.
    pub fn new(what: &'static str, mut child: Child) -> Self {
        let stdout = Pipe::new(child.stdout.take());
        let stderr = Pipe::new(child.stderr.take());
        OwnedChild {
            what,
            child,
            stdout,
            stderr,
            status: None,
        }
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Fails the test: kills the child and reports the stage and output.
    fn fail(&mut self, problem: &str) -> ! {
        self.kill();
        let stderr = self.stderr_tail();
        panic!(
            "{}: {problem} (last stage: {}); stderr so far: {stderr}",
            self.what,
            last_stage()
        );
    }

    fn stderr_tail(&mut self) -> String {
        self.stderr.take_ready();
        let bytes = &self.stderr.bytes;
        let tail = &bytes[bytes.len().saturating_sub(4096)..];
        format!("{:?}", String::from_utf8_lossy(tail))
    }

    /// The next stdout line before `deadline`; `None` once stdout ended.
    pub fn stdout_line(&mut self, deadline: Instant) -> Option<String> {
        step("read a stdout line");
        let line = self.stdout.line(deadline);
        line.unwrap_or_else(|()| self.fail("no stdout line in time"))
    }

    /// The next stderr line before `deadline`; `None` once stderr ended.
    pub fn stderr_line(&mut self, deadline: Instant) -> Option<String> {
        step("read a stderr line");
        let line = self.stderr.line(deadline);
        line.unwrap_or_else(|()| self.fail("no stderr line in time"))
    }

    /// Waits for the exit until `deadline`, polling.
    pub fn wait_until(&mut self, deadline: Instant) -> ExitStatus {
        step("wait for the exit");
        loop {
            if let Some(status) = self.status {
                return status;
            }
            match self.child.try_wait() {
                Ok(Some(status)) => self.status = Some(status),
                Ok(None) if Instant::now() >= deadline => self.fail("did not exit in time"),
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(error) => self.fail(&format!("could not poll the exit: {error}")),
            }
        }
    }

    /// Reads both pipes to their end until `deadline`. A pipe still open
    /// then is held by some other process (a descendant that inherited
    /// it): the test fails instead of waiting for it.
    pub fn collect_until(&mut self, deadline: Instant) -> Output {
        step("collect the output");
        if !self.stdout.drain(deadline) {
            self.fail("stdout stayed open past the deadline");
        }
        if !self.stderr.drain(deadline) {
            self.fail("stderr stayed open past the deadline");
        }
        Output {
            status: self.status.expect("collected after the exit"),
            stdout: self.stdout.bytes.clone(),
            stderr: self.stderr.bytes.clone(),
        }
    }

    /// Waits for the exit and the whole output, both within `within`.
    pub fn finish(&mut self, within: Duration) -> Output {
        let deadline = Instant::now() + within;
        self.wait_until(deadline);
        self.collect_until(deadline)
    }

    /// Kills the child (if it still runs) and reaps it, bounded.
    pub fn kill(&mut self) {
        if self.status.is_some() {
            return;
        }
        let _ = self.child.kill();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.status = Some(status);
                    return;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => return,
            }
        }
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.kill();
        if std::thread::panicking() {
            let stderr = self.stderr_tail();
            eprintln!(
                "{} at the failure: last stage: {}; stderr so far: {stderr}",
                self.what,
                last_stage()
            );
        }
    }
}

// --- Loopback HTTP ---------------------------------------------------------------

/// Connects to `127.0.0.1:port` before `deadline`.
pub fn connect(port: u16, deadline: Instant) -> io::Result<TcpStream> {
    step("connect");
    let wait = left(deadline).ok_or(io::ErrorKind::TimedOut)?;
    TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), wait)
}

/// Writes all of `bytes` before `deadline`.
pub fn send(stream: &mut TcpStream, bytes: &[u8], deadline: Instant) -> io::Result<()> {
    step("write the request");
    let wait = left(deadline).ok_or(io::ErrorKind::TimedOut)?;
    stream.set_write_timeout(Some(wait))?;
    stream.write_all(bytes)
}

/// Why a framed response could not be read.
#[derive(Debug)]
pub enum ReadError {
    /// The connection ended before any byte of an answer.
    NoAnswer,
    /// `deadline` passed; the text says how far the response got.
    Timeout(String),
}

/// Reads one HTTP/1.1 response by its framing (`Content-Length`, chunked,
/// or to the end of the connection when it has neither) before `deadline`,
/// without waiting for the server to close the connection. Returns the
/// head, the blank line and the decoded body. An answer cut short by the
/// end of the connection (a reset after a complete answer, say) is
/// returned as it is, for the caller's parser to judge.
pub fn read_response(stream: &mut TcpStream, deadline: Instant) -> Result<Vec<u8>, ReadError> {
    step("read the response");
    let mut received = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    let mut ended = false;
    loop {
        if let Some(response) = complete(&received, ended) {
            return Ok(response);
        }
        if ended {
            return if received.is_empty() {
                Err(ReadError::NoAnswer)
            } else {
                Ok(received)
            };
        }
        let Some(wait) = left(deadline) else {
            return Err(ReadError::Timeout(format!(
                "{} bytes of a response, not complete",
                received.len()
            )));
        };
        if stream.set_read_timeout(Some(wait)).is_err() {
            ended = true;
            continue;
        }
        match stream.read(&mut chunk) {
            Ok(0) => ended = true,
            Ok(n) => received.extend_from_slice(&chunk[..n]),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => ended = true,
        }
    }
}

/// The response in `received` (head and decoded body) once its framing
/// says it is complete.
fn complete(received: &[u8], ended: bool) -> Option<Vec<u8>> {
    let split = received
        .windows(4)
        .position(|window| window == b"\r\n\r\n")?
        + 4;
    let head = String::from_utf8_lossy(&received[..split]).to_ascii_lowercase();
    let body = &received[split..];
    let header = |name: &str| {
        head.lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(key, _)| key.trim() == name)
            .map(|(_, value)| value.trim().to_owned())
    };
    let mut response = received[..split].to_vec();
    if header("transfer-encoding").is_some_and(|value| value.contains("chunked")) {
        response.extend_from_slice(&dechunk(body)?);
    } else if let Some(length) = header("content-length") {
        let length: usize = length.parse().ok()?;
        response.extend_from_slice(body.get(..length)?);
    } else if ended {
        response.extend_from_slice(body);
    } else {
        return None;
    }
    Some(response)
}

/// The data of a complete chunked body; `None` until its last chunk.
fn dechunk(mut body: &[u8]) -> Option<Vec<u8>> {
    let mut data = Vec::new();
    loop {
        let line_end = body.windows(2).position(|window| window == b"\r\n")?;
        let size = std::str::from_utf8(&body[..line_end]).ok()?;
        let size = usize::from_str_radix(size.split(';').next()?.trim(), 16).ok()?;
        body = &body[line_end + 2..];
        if size == 0 {
            // The last chunk; trailers end with an empty line.
            body.windows(2).position(|window| window == b"\r\n")?;
            return Some(data);
        }
        data.extend_from_slice(body.get(..size)?);
        body = body.get(size + 2..)?;
    }
}
