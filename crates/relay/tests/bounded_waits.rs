//! The bounded waits in `support/bounded.rs` hold their deadline against a
//! peer that keeps making progress: a child that never stops writing, and a
//! loopback reader that keeps taking a little at a time.

mod support;

use std::io::{self, Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use support::bounded::{self, OwnedChild, TimedOut};

/// How far past its deadline a bounded wait may return.
const MARGIN: Duration = Duration::from_millis(250);

/// Set in the child copy of this test binary that runs [`endless_writer`].
const ENDLESS_WRITER: &str = "PUMICE_TEST_ENDLESS_WRITER";

/// Not a test on its own: in the copy started by
/// [`a_line_wait_ends_at_its_deadline_while_output_keeps_coming`], writes
/// one line to stderr, then more without a newline until killed.
#[test]
fn endless_writer() {
    if std::env::var_os(ENDLESS_WRITER).is_none() {
        return;
    }
    let chunk = [b'x'; 8192];
    let mut stderr = io::stderr().lock();
    stderr
        .write_all(b"writing\n")
        .expect("write the first line");
    while stderr.write_all(&chunk).is_ok() {}
}

#[test]
fn a_line_wait_ends_at_its_deadline_while_output_keeps_coming() {
    let child = Command::new(std::env::current_exe().expect("test binary path"))
        .args([
            "--exact",
            "endless_writer",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(ENDLESS_WRITER, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start the endless writer");
    let mut child = OwnedChild::new("endless writer", child);

    // Once it writes, the wait below is always busy taking output.
    let first = child.stderr_line(Instant::now() + Duration::from_secs(30));
    assert_eq!(first.as_deref(), Some("writing"));

    let within = Duration::from_millis(100);
    let started = Instant::now();
    let line = child.try_stderr_line(started + within);
    let took = started.elapsed();
    assert!(matches!(line, Err(TimedOut)), "got a line: {line:?}");
    assert!(
        took < within + MARGIN,
        "a {within:?} line wait took {took:?}"
    );
}

#[test]
fn a_send_ends_at_its_deadline_while_a_slow_reader_takes_a_little() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
    let port = listener.local_addr().expect("local address").port();
    let reader = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept the sender");
        let mut buffer = vec![0u8; 256 * 1024];
        loop {
            std::thread::sleep(Duration::from_millis(10));
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    });

    let mut stream = bounded::connect(port, Instant::now() + Duration::from_secs(10))
        .expect("connect to the slow reader");
    // Far more than the loopback buffers hold, so the send keeps waiting on
    // the reader, which frees a little of them every 10 ms.
    let bytes = vec![b'x'; 64 * 1024 * 1024];
    let within = Duration::from_millis(100);
    let started = Instant::now();
    let sent = bounded::send(&mut stream, &bytes, started + within);
    let took = started.elapsed();
    drop(stream);
    reader.join().expect("the reader thread");

    let error = sent.expect_err("64 MiB cannot reach a slow reader in 100 ms");
    assert_eq!(error.kind(), io::ErrorKind::TimedOut, "{error}");
    assert!(took < within + MARGIN, "a {within:?} send took {took:?}");
}
