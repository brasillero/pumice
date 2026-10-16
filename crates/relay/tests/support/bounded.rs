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
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
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

/// How long a failure report may spend taking output that already arrived,
/// and how much of it, so a child that keeps writing cannot stall it.
const REPORT_TIME: Duration = Duration::from_millis(100);
const REPORT_BYTES: usize = 1 << 20;

/// How much output one wait takes before it looks at it again.
const PULL_BYTES: usize = 1 << 20;

/// How much output a pipe keeps; its reader thread drops the rest, so a
/// child that never stops writing cannot exhaust memory.
const PIPE_BYTES: usize = 64 << 20;

/// A deadline passed.
#[derive(Debug)]
pub struct TimedOut;

// --- Child processes -----------------------------------------------------------

/// One output pipe, read to its end by its own thread, which keeps its
/// first [`PIPE_BYTES`].
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
                let mut kept = 0;
                loop {
                    match source.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => {
                            let n = n.min(PIPE_BYTES - kept);
                            kept += n;
                            if n > 0 && sender.send(buffer[..n].to_vec()).is_err() {
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

    /// Takes what already arrived, without waiting: until `deadline` and at
    /// most about `limit` bytes, so a writer that never stops cannot keep
    /// it going.
    fn take_ready(&mut self, deadline: Instant, limit: usize) {
        let Some(chunks) = &self.chunks else {
            return;
        };
        let start = self.bytes.len();
        let mut ended = false;
        while self.bytes.len() - start < limit && Instant::now() < deadline {
            match chunks.try_recv() {
                Ok(chunk) => self.bytes.extend_from_slice(&chunk),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    ended = true;
                    break;
                }
            }
        }
        if ended {
            self.chunks = None;
        }
    }

    /// Takes what arrived, waiting until `deadline` for more. False only
    /// when the deadline passed with the pipe still open; nothing is taken
    /// past it.
    fn pull(&mut self, deadline: Instant) -> bool {
        let Some(chunks) = &self.chunks else {
            return true;
        };
        let Some(wait) = left(deadline) else {
            return false;
        };
        match chunks.recv_timeout(wait) {
            Ok(chunk) => {
                self.bytes.extend_from_slice(&chunk);
                self.take_ready(deadline, PULL_BYTES);
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
    /// pipe.
    fn line(&mut self, deadline: Instant) -> Result<Option<String>, TimedOut> {
        // Each byte is searched once, however long the line grows.
        let mut searched = self.consumed;
        loop {
            let rest = &self.bytes[self.consumed..];
            if let Some(end) = self.bytes[searched..]
                .iter()
                .position(|&byte| byte == b'\n')
            {
                let end = searched - self.consumed + end;
                let line = String::from_utf8_lossy(&rest[..end])
                    .trim_end_matches('\r')
                    .to_owned();
                self.consumed += end + 1;
                return Ok(Some(line));
            }
            searched = self.bytes.len();
            if self.at_eof() {
                if rest.is_empty() {
                    return Ok(None);
                }
                let line = String::from_utf8_lossy(rest).into_owned();
                self.consumed = self.bytes.len();
                return Ok(Some(line));
            }
            if !self.pull(deadline) {
                return Err(TimedOut);
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
    /// Extra state for failure reports (see [`OwnedChild::context`]).
    context: Option<Box<dyn Fn() -> String>>,
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
            context: None,
        }
    }

    /// Adds `context()` to every failure report: state the test can read
    /// at that moment (a fake's log, say), metadata only.
    pub fn context(mut self, context: impl Fn() -> String + 'static) -> Self {
        self.context = Some(Box::new(context));
        self
    }

    fn report(&mut self) -> String {
        self.stderr
            .take_ready(Instant::now() + REPORT_TIME, REPORT_BYTES);
        let bytes = &self.stderr.bytes;
        let tail = &bytes[bytes.len().saturating_sub(4096)..];
        let mut report = format!(
            "last stage: {}; stderr so far: {:?}",
            last_stage(),
            String::from_utf8_lossy(tail)
        );
        if let Some(context) = &self.context {
            report.push_str("; context: ");
            report.push_str(&context());
        }
        #[cfg(windows)]
        {
            report.push_str("; children left: ");
            report.push_str(&windows_children(self.child.id()));
        }
        report
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Fails the test: kills the child and reports the stage and output.
    fn fail(&mut self, problem: &str) -> ! {
        self.kill();
        let report = self.report();
        panic!("{}: {problem} ({report})", self.what);
    }

    /// The next stdout line before `deadline`; `None` once stdout ended.
    pub fn stdout_line(&mut self, deadline: Instant) -> Option<String> {
        step("read a stdout line");
        let line = self.stdout.line(deadline);
        line.unwrap_or_else(|TimedOut| self.fail("no stdout line in time"))
    }

    /// The next stderr line before `deadline`; `None` once stderr ended.
    pub fn stderr_line(&mut self, deadline: Instant) -> Option<String> {
        step("read a stderr line");
        let line = self.stderr.line(deadline);
        line.unwrap_or_else(|TimedOut| self.fail("no stderr line in time"))
    }

    /// Like [`OwnedChild::stderr_line`], but returns a timeout instead of
    /// failing the test.
    pub fn try_stderr_line(&mut self, deadline: Instant) -> Result<Option<String>, TimedOut> {
        step("read a stderr line");
        self.stderr.line(deadline)
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
        let stdout_ended = self.stdout.drain(deadline);
        // Past the deadline, still take what stderr has, to report both.
        let stderr_ended = if stdout_ended {
            self.stderr.drain(deadline)
        } else {
            self.stderr
                .take_ready(Instant::now() + REPORT_TIME, REPORT_BYTES);
            self.stderr.at_eof()
        };
        if !(stdout_ended && stderr_ended) {
            let open = match (stdout_ended, stderr_ended) {
                (false, false) => "stdout and stderr",
                (false, true) => "stdout",
                _ => "stderr",
            };
            self.fail(&format!(
                "{open} stayed open past the deadline after the exit ({:?})",
                self.status
            ));
        }
        let full = [("stdout", &self.stdout), ("stderr", &self.stderr)]
            .into_iter()
            .find(|(_, pipe)| pipe.bytes.len() >= PIPE_BYTES);
        if let Some((name, _)) = full {
            self.fail(&format!(
                "{name} passed {PIPE_BYTES} bytes, the rest was dropped"
            ));
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
        if self.status.is_none() {
            self.status = kill_and_reap(&mut self.child);
        }
    }
}

/// Kills `child` (if it still runs) and reaps it, waiting at most 10 s.
fn kill_and_reap(child: &mut Child) -> Option<ExitStatus> {
    let _ = child.kill();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => return None,
        }
    }
    None
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.kill();
        if std::thread::panicking() {
            let report = self.report();
            eprintln!("{} at the failure: {report}", self.what);
        }
    }
}

/// Whether process `pid` still exists, for failure reports only:
/// "alive", "gone" or "unknown". Unix asks the kernel (`kill(pid, 0)`);
/// Windows runs `tasklist`, bounded.
pub fn process_state(pid: u64) -> &'static str {
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return "unknown";
        };
        // SAFETY: signal 0 only checks that the process exists.
        if unsafe { libc::kill(pid, 0) } == 0 {
            return "alive";
        }
        match io::Error::last_os_error().raw_os_error() {
            Some(libc::ESRCH) => "gone",
            // It exists but belongs to someone else.
            Some(libc::EPERM) => "alive",
            _ => "unknown",
        }
    }
    #[cfg(windows)]
    {
        let mut tasklist = std::process::Command::new("tasklist");
        tasklist.args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"]);
        match diagnostic_output(&mut tasklist, Duration::from_secs(10)) {
            Some(text) if text.contains(&format!("\"{pid}\"")) => "alive",
            Some(_) => "gone",
            None => "unknown",
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        "unknown"
    }
}

/// The processes whose parent was `pid` (they keep that parent ID after it
/// exits) and their threads' wait reasons ("Suspended" for a child that
/// never ran), for failure reports. Bounded: "unknown" after 30 s.
#[cfg(windows)]
fn windows_children(pid: u32) -> String {
    let script = format!(
        "Get-CimInstance Win32_Process -Filter 'ParentProcessId={pid}' | ForEach-Object {{ \
         $w = (Get-Process -Id $_.ProcessId -ErrorAction SilentlyContinue).Threads | \
         ForEach-Object {{ $_.WaitReason }}; \
         \"$($_.ProcessId) $($_.Name) [$($w -join ',')]\" }}"
    );
    let mut powershell = std::process::Command::new("powershell");
    powershell.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    match diagnostic_output(&mut powershell, Duration::from_secs(30)) {
        Some(text) => {
            let lines: Vec<&str> = text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .collect();
            if lines.is_empty() {
                "none".to_owned()
            } else {
                lines.join("; ")
            }
        }
        None => "unknown".to_owned(),
    }
}

/// Runs a diagnostic command for its stdout, within `within`. Past it the
/// command is killed and reaped (bounded too) and the answer is `None`, so
/// a stuck diagnostic neither stalls a failure report nor outlives it.
#[cfg(windows)]
fn diagnostic_output(command: &mut std::process::Command, within: Duration) -> Option<String> {
    use std::process::Stdio;
    let deadline = Instant::now() + within;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = Pipe::new(child.stdout.take());
    let ended = stdout.drain(deadline);
    let exited = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            _ => {
                kill_and_reap(&mut child);
                break false;
            }
        }
    };
    (ended && exited).then(|| String::from_utf8_lossy(&stdout.bytes).into_owned())
}

// --- Loopback HTTP ---------------------------------------------------------------

/// Connects to `127.0.0.1:port` before `deadline`.
pub fn connect(port: u16, deadline: Instant) -> io::Result<TcpStream> {
    step("connect");
    let wait = left(deadline).ok_or(io::ErrorKind::TimedOut)?;
    TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), wait)
}

/// Writes all of `bytes` before `deadline`. Each write waits only for what
/// is left of it, so a reader that keeps taking a little cannot stretch it.
pub fn send(stream: &mut TcpStream, bytes: &[u8], deadline: Instant) -> io::Result<()> {
    step("write the request");
    let mut rest = bytes;
    while !rest.is_empty() {
        let wait = left(deadline).ok_or(io::ErrorKind::TimedOut)?;
        stream.set_write_timeout(Some(wait))?;
        match stream.write(rest) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => rest = &rest[n..],
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
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
