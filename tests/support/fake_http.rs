//! A scripted fake HTTP server for the generic loopback adapter tests.
//!
//! An owned `tokio` listener on `127.0.0.1:0`: it records each request
//! (method, target, headers, body), then replies per the scripted behavior —
//! a normal HTTP response, a stall (for deadline tests), raw bytes (for
//! malformed framing) or a hang-up that waits for the client to close (for
//! cancellation tests). No test ever talks to a real inference server.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use pumice::providers::generic::{LoopbackEndpoint, parse_endpoint};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// Guards against a wedged test reading forever.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// Request heads and bodies beyond these sizes are never expected in tests.
const MAX_HEAD_BYTES: usize = 64 * 1024;
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// One request captured by the fake server.
#[derive(Clone, Debug)]
pub struct RecordedRequest {
    pub method: String,
    /// The request target exactly as sent (origin form).
    pub target: String,
    /// Header names lowercased, values trimmed.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl RecordedRequest {
    /// The first value of `name` (case-insensitive), when present.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// What the server does after reading one full request.
#[derive(Clone, Debug)]
pub enum Behavior {
    /// Reply with a complete HTTP response, then close.
    Reply(Reply),
    /// Record the request and sleep far past any test deadline.
    Stall,
    /// Record the request, write these raw bytes (possibly malformed HTTP)
    /// and close.
    Raw(Vec<u8>),
    /// Record the request, write these raw bytes, then keep the connection
    /// open without sending anything more.
    RawThenStall(Vec<u8>),
    /// Record the request, then read until the client closes the connection
    /// (used to prove cancellation drops the socket).
    HangUp,
}

/// A scripted HTTP response.
#[derive(Clone, Debug)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    /// A JSON response with the given status.
    pub fn json(status: u16, body: serde_json::Value) -> Reply {
        Reply {
            status,
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: body.to_string().into_bytes(),
        }
    }
}

/// A running fake server. Dropping it stops the accept loop.
pub struct FakeHttp {
    addr: SocketAddr,
    task: JoinHandle<()>,
    recorded: Arc<Mutex<Vec<RecordedRequest>>>,
    accepted: Arc<AtomicUsize>,
    ended: Arc<AtomicUsize>,
}

impl FakeHttp {
    pub async fn spawn(behavior: Behavior) -> FakeHttp {
        FakeHttp::spawn_on(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)), behavior).await
    }

    /// Spawns the server on an explicit address (the IPv6 loopback test needs
    /// `[::1]`, which an IPv4-bound socket never answers).
    pub async fn spawn_on(bind: SocketAddr, behavior: Behavior) -> FakeHttp {
        let listener = TcpListener::bind(bind)
            .await
            .expect("bind fake HTTP server to a loopback address");
        let addr = listener.local_addr().expect("server local address");
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let accepted = Arc::new(AtomicUsize::new(0));
        let ended = Arc::new(AtomicUsize::new(0));
        let task = tokio::spawn(serve(
            listener,
            behavior,
            Arc::clone(&recorded),
            Arc::clone(&accepted),
            Arc::clone(&ended),
        ));
        FakeHttp {
            addr,
            task,
            recorded,
            accepted,
            ended,
        }
    }

    /// The address the server listens on.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The accepted base URL spelling for this server.
    pub fn base_url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }

    /// A ready provider endpoint pointing at this server.
    pub fn endpoint(&self) -> LoopbackEndpoint {
        parse_endpoint(&self.base_url()).expect("the fake server's own URL parses")
    }

    /// How many connections the server has accepted.
    pub fn hit_count(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    /// How many accepted connections have ended (client or server closed).
    pub fn ended_count(&self) -> usize {
        self.ended.load(Ordering::SeqCst)
    }

    /// Every request recorded so far.
    pub async fn recorded(&self) -> Vec<RecordedRequest> {
        self.recorded.lock().await.clone()
    }

    /// The single request the test expects, panicking on anything else.
    pub async fn last_request(&self) -> RecordedRequest {
        let recorded = self.recorded().await;
        assert_eq!(
            recorded.len(),
            1,
            "expected exactly one recorded request, got {}",
            recorded.len()
        );
        recorded[0].clone()
    }
}

impl Drop for FakeHttp {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(
    listener: TcpListener,
    behavior: Behavior,
    recorded: Arc<Mutex<Vec<RecordedRequest>>>,
    accepted: Arc<AtomicUsize>,
    ended: Arc<AtomicUsize>,
) {
    loop {
        let Ok((socket, _)) = listener.accept().await else {
            return;
        };
        accepted.fetch_add(1, Ordering::SeqCst);
        let behavior = behavior.clone();
        let recorded = Arc::clone(&recorded);
        let ended = Arc::clone(&ended);
        tokio::spawn(async move {
            handle(socket, behavior, recorded).await;
            ended.fetch_add(1, Ordering::SeqCst);
        });
    }
}

async fn handle(
    mut socket: TcpStream,
    behavior: Behavior,
    recorded: Arc<Mutex<Vec<RecordedRequest>>>,
) {
    let Ok(Some(request)) = read_request(&mut socket).await else {
        // The client went away before sending a full request.
        return;
    };
    recorded.lock().await.push(request);

    match behavior {
        Behavior::Stall => {
            // Outlive every test deadline; the runtime drops this task when
            // the test ends.
            tokio::time::sleep(Duration::from_secs(300)).await;
        }
        Behavior::Raw(bytes) => {
            let _ = socket.write_all(&bytes).await;
            let _ = socket.shutdown().await;
        }
        Behavior::RawThenStall(bytes) => {
            let _ = socket.write_all(&bytes).await;
            tokio::time::sleep(Duration::from_secs(300)).await;
        }
        Behavior::HangUp => {
            // Wait until the client closes the connection (cancellation must
            // drop the socket), swallowing any data that arrives.
            let mut sink = [0u8; 4096];
            loop {
                match socket.read(&mut sink).await {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {}
                }
            }
        }
        Behavior::Reply(reply) => {
            let mut head = format!("HTTP/1.1 {} {}\r\n", reply.status, reason(reply.status));
            let has_length = reply
                .headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("content-length"));
            for (name, value) in &reply.headers {
                head.push_str(&format!("{name}: {value}\r\n"));
            }
            if !has_length {
                head.push_str(&format!("content-length: {}\r\n", reply.body.len()));
            }
            head.push_str("connection: close\r\n\r\n");
            if socket.write_all(head.as_bytes()).await.is_err() {
                return;
            }
            let _ = socket.write_all(&reply.body).await;
            let _ = socket.shutdown().await;
        }
    }
}

/// Reads one HTTP request (head, then `Content-Length` bytes) from the
/// socket. The tests' client always sends exactly this shape.
async fn read_request(socket: &mut TcpStream) -> Result<Option<RecordedRequest>, ()> {
    let mut buf = Vec::new();
    // The head ends at the first CRLFCRLF.
    let head_end = loop {
        if let Some(position) = find(&buf, b"\r\n\r\n") {
            break position;
        }
        if buf.len() > MAX_HEAD_BYTES {
            return Err(());
        }
        let mut chunk = [0u8; 4096];
        let read = tokio::time::timeout(READ_TIMEOUT, socket.read(&mut chunk))
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?;
        if read == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..read]);
    };

    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or_default().to_owned();
    let mut headers = Vec::new();
    let mut content_length = 0usize;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.trim().eq_ignore_ascii_case("content-length") {
            content_length = value.parse().unwrap_or(0);
        }
        headers.push((name.trim().to_ascii_lowercase(), value.to_owned()));
    }

    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < content_length {
        if body.len() > MAX_BODY_BYTES {
            return Err(());
        }
        let mut chunk = [0u8; 8192];
        let read = tokio::time::timeout(READ_TIMEOUT, socket.read(&mut chunk))
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?;
        if read == 0 {
            return Ok(None);
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length);

    Ok(Some(RecordedRequest {
        method,
        target,
        headers,
        body,
    }))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        408 => "Request Timeout",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Status",
    }
}
