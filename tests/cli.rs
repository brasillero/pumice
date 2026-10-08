//! CLI entry-point tests (S7.3 follow-up): a bare `pumice` starts the same
//! service as `pumice serve`, `--config` works in the bare form and the
//! `serve` alias, help/version and malformed arguments never start a server,
//! and the bad-config (exit 2) and occupied-port (exit 1) contracts hold.
//! Everything runs against hermetic fake provider binaries and the built-in
//! `passthrough` model — no AI CLI is ever invoked.

mod support;

use std::fs;
use std::io::{BufRead, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::FakeCli;
use tempfile::TempDir;

/// The recorded Handy transcript; `passthrough` must return it byte for byte.
const FIXTURE_TRANSCRIPT: &str = "Reunião com a equipe às nove horas, não esquecer de enviar o relatório para o João e revisar o orçamento.";

/// YAML pointing every binary-probing provider (enabled or not — startup
/// detection probes the whole registry) at its own disposable fake, so no
/// test ever looks a real CLI up on PATH. Every provider stays disabled:
/// these tests exercise the service and `passthrough`, never formatting.
/// Returns the fakes — they must outlive the served process — and the YAML.
fn hermetic_yaml(port: u16) -> (Vec<FakeCli>, String) {
    let mut fakes: Vec<FakeCli> = Vec::new();
    let mut yaml = format!("port: {port}\nproviders:\n");
    for id in ["claude", "codex", "kimi"] {
        let fake = FakeCli::new(json!({}));
        yaml.push_str(&format!(
            "  - id: {id}\n    enabled: false\n    binary: '{}'\n",
            fake.path().display()
        ));
        fakes.push(fake);
    }
    (fakes, yaml)
}

/// Writes `yaml` to `pumice.yaml` inside a fresh directory and returns the
/// directory (kept alive by the caller) and the file's path.
fn write_config(yaml: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("pumice.yaml");
    fs::write(&path, yaml).expect("write config");
    (dir, path)
}

/// A port that was free at observation time; binding it again can in theory
/// race, so the caller retries.
fn grab_free_port() -> u16 {
    let listener = std::net::TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .expect("bind ephemeral port");
    listener.local_addr().unwrap().port()
}

/// A running `pumice` service child process. Killing and reaping are
/// guaranteed: [`Server::stop`] does it for the happy path and `Drop` covers
/// panics, with a bounded wait so a stuck child cannot hang the test suite.
struct Server {
    child: Child,
    port: u16,
}

impl Server {
    /// Stops the service and asserts it reaps cleanly. `Drop` sees the
    /// reaped child and does nothing.
    fn stop(mut self) {
        self.child.kill().expect("stop pumice");
        self.child.wait().expect("pumice reaped");
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_some() {
            return;
        }
        let _ = self.child.kill();
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.child.try_wait().ok().flatten().is_none() {
            if Instant::now() >= deadline {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Spawns `pumice` with `args`, child-specific `envs` and working directory
/// `cwd`, and waits for the startup line naming `port`. `Err` carries the
/// child's stderr for the one tolerable early exit: a lost ephemeral-port
/// race, which the caller retries.
fn try_start(
    args: &[&str],
    envs: &[(&str, &Path)],
    cwd: &Path,
    port: u16,
) -> Result<Server, String> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pumice"));
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in envs {
        command.env(key, value);
    }
    let mut server = Server {
        child: command.spawn().expect("spawn pumice"),
        port,
    };
    let child = &mut server.child;

    // Read the first stdout line on a helper thread: the service prints its
    // loopback address there once it is accepting connections.
    let mut stdout = child.stdout.take().expect("piped stdout");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut line = String::new();
        let mut byte = [0u8; 1];
        loop {
            match stdout.read(&mut byte) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    line.push(byte[0] as char);
                    if byte[0] == b'\n' {
                        break;
                    }
                }
            }
        }
        let _ = tx.send(if line.is_empty() { None } else { Some(line) });
    });
    let line = match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(line) => line,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("pumice did not print its startup line on port {port}");
        }
    };
    let Some(line) = line else {
        let status = child.wait().expect("child exits");
        let stderr = child
            .stderr
            .take()
            .map(|mut err| {
                let mut text = String::new();
                let _ = err.read_to_string(&mut text);
                text
            })
            .unwrap_or_default();
        return Err(format!("exit {status}: {stderr}"));
    };
    assert!(
        line.contains(&format!("http://127.0.0.1:{port}/v1")),
        "startup line must name the IPv4 loopback address: {line}"
    );
    assert!(
        !line.contains("0.0.0.0"),
        "never any other interface: {line}"
    );
    Ok(server)
}

/// Asserts an early exit was only the tolerable ephemeral-port race.
fn assert_port_race(error: &str, attempt: usize) {
    assert!(
        error.contains("already in use"),
        "pumice exited unexpectedly (attempt {attempt}): {error}"
    );
}

/// Runs `pumice` with `args` to completion in `cwd` and returns its output.
fn run(args: &[&str], cwd: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pumice"))
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .expect("run pumice")
}

/// A parsed minimal HTTP response (status, body).
struct RawResponse {
    status: u16,
    body: Vec<u8>,
}

impl RawResponse {
    fn body_json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("response body is valid JSON")
    }
}

/// Sends `request` verbatim over HTTP/1.1 with fixed framing and reads the
/// whole response.
fn raw_http(port: u16, method: &str, path: &str, body: &[u8]) -> RawResponse {
    let request = format!(
        "{method} {path} HTTP/1.1\r\nhost: 127.0.0.1\r\nconnection: close\r\ncontent-length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    let mut stream =
        TcpStream::connect(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .expect("read timeout");
    stream.write_all(&request).expect("write request");
    stream.write_all(body).expect("write body");
    let mut response = Vec::new();
    stream.read_to_end(&mut response).expect("read response");
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("header/body separator present");
    let head = String::from_utf8_lossy(&response[..split]);
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .expect("status line has a code")
        .parse()
        .expect("status code is numeric");
    RawResponse {
        status,
        body: response[split + 4..].to_vec(),
    }
}

/// The recorded Handy request with `model` replaced, for `/v1/chat/completions`.
fn handy_request(model: &str) -> Vec<u8> {
    let mut body: Value =
        serde_json::from_str(&support::fixture("handy-request.json")).expect("fixture parses");
    body["model"] = json!(model);
    serde_json::to_vec(&body).expect("fixture re-serializes")
}

/// Asserts the service on `port` answers `/health` and returns the recorded
/// transcript byte for byte through the AI-free `passthrough` model, without
/// running any provider fake.
fn assert_service_works(port: u16, fakes: &[FakeCli]) {
    let health = raw_http(port, "GET", "/health", b"");
    assert_eq!(health.status, 200, "body: {:?}", health.body_json());
    assert_eq!(health.body_json()["status"], "ok");
    assert_eq!(health.body_json()["version"], env!("CARGO_PKG_VERSION"));

    let completion = raw_http(
        port,
        "POST",
        "/v1/chat/completions",
        &handy_request("passthrough"),
    );
    assert_eq!(completion.status, 200, "body: {:?}", completion.body_json());
    let body = completion.body_json();
    assert_eq!(body["model"], "passthrough");
    assert_eq!(
        body["choices"][0]["message"]["content"], FIXTURE_TRANSCRIPT,
        "passthrough returns the exact transcript without calling an AI"
    );
    // Startup detection probes every provider's binary with `--version`
    // (empty stdin); the report must show no formatting call, which would
    // carry the prompt on stdin.
    for fake in fakes {
        if fake.report_path().exists() {
            let stdin = &fake.report()["stdin"];
            assert!(
                stdin.is_null() || stdin.as_str().is_some_and(str::is_empty),
                "no provider fake may receive a formatting call: {stdin}"
            );
        }
    }
}

#[test]
fn bare_pumice_starts_the_service_with_the_per_user_config() {
    for attempt in 1..=3 {
        let port = grab_free_port();
        let (fakes, yaml) = hermetic_yaml(port);
        let (config_dir, config_path) = write_config(&yaml);
        let xdg = config_dir.path().join("xdg");
        let config_home = xdg.join("pumice");
        fs::create_dir_all(&config_home).expect("create per-user config dir");
        fs::copy(&config_path, config_home.join("pumice.yaml")).expect("install per-user config");
        let cwd = config_dir.path().join("work");
        fs::create_dir_all(&cwd).expect("create working directory");

        // The child sees the temp per-user config location through its own
        // environment; the process environment and HOME are never touched.
        let server = match try_start(
            &[],
            &[("XDG_CONFIG_HOME", &xdg), ("APPDATA", &xdg)],
            &cwd,
            port,
        ) {
            Ok(server) => server,
            Err(error) => {
                assert_port_race(&error, attempt);
                continue;
            }
        };
        assert_service_works(server.port, &fakes);
        server.stop();
        return;
    }
    panic!("could not start pumice on a free port after 3 attempts");
}

#[test]
fn bare_pumice_with_config_flag_starts_the_service() {
    for attempt in 1..=3 {
        let port = grab_free_port();
        let (fakes, yaml) = hermetic_yaml(port);
        let (_dir, config_path) = write_config(&yaml);
        let cwd = TempDir::new().expect("temp dir");
        let server = match try_start(
            &["--config", config_path.to_str().expect("UTF-8 path")],
            &[],
            cwd.path(),
            port,
        ) {
            Ok(server) => server,
            Err(error) => {
                assert_port_race(&error, attempt);
                continue;
            }
        };
        assert_service_works(server.port, &fakes);
        server.stop();
        return;
    }
    panic!("could not start pumice on a free port after 3 attempts");
}

#[test]
fn serve_alias_still_starts_the_service() {
    for attempt in 1..=3 {
        let port = grab_free_port();
        let (fakes, yaml) = hermetic_yaml(port);
        let (_dir, config_path) = write_config(&yaml);
        let cwd = TempDir::new().expect("temp dir");
        let server = match try_start(
            &[
                "serve",
                "--config",
                config_path.to_str().expect("UTF-8 path"),
            ],
            &[],
            cwd.path(),
            port,
        ) {
            Ok(server) => server,
            Err(error) => {
                assert_port_race(&error, attempt);
                continue;
            }
        };
        assert_service_works(server.port, &fakes);
        server.stop();
        return;
    }
    panic!("could not start pumice on a free port after 3 attempts");
}

#[test]
fn help_and_version_print_without_starting_the_service() {
    let cwd = TempDir::new().expect("temp dir");
    for flag in ["--help", "-h"] {
        let output = run(&[flag], cwd.path());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(output.status.code(), Some(0), "stderr: {:?}", output.stderr);
        assert!(stdout.contains("usage:"), "help must print usage: {stdout}");
        assert!(
            stdout.contains("pumice [--config <path>]"),
            "help must show the primary bare command: {stdout}"
        );
    }
    for flag in ["--version", "-V"] {
        let output = run(&[flag], cwd.path());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(output.status.code(), Some(0), "stderr: {:?}", output.stderr);
        assert!(
            stdout.contains(&format!("pumice {}", env!("CARGO_PKG_VERSION"))),
            "version must print the crate version: {stdout}"
        );
    }
}

#[test]
fn malformed_args_exit_2_with_usage_without_starting_the_service() {
    let cwd = TempDir::new().expect("temp dir");
    for args in [
        &["nope"][..],
        &["serve", "--bogus"][..],
        &["--config"][..],
        &["--config", "a", "b"][..],
        &["check-config", "--config"][..],
    ] {
        let output = run(args, cwd.path());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            output.status.code(),
            Some(2),
            "args {args:?} must exit 2: {stderr}"
        );
        assert!(
            stderr.contains("usage:"),
            "args {args:?} must print usage: {stderr}"
        );
        assert!(
            output.stdout.is_empty(),
            "args {args:?} must not write stdout: {:?}",
            output.stdout
        );
    }
}

#[test]
fn bad_config_exits_2_naming_the_file_and_line() {
    let dir = TempDir::new().expect("temp dir");
    let config_path = dir.path().join("pumice.yaml");
    fs::write(&config_path, "port: [unclosed\n").expect("write bad config");
    let output = run(
        &["--config", config_path.to_str().expect("UTF-8 path")],
        dir.path(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(2),
        "a bad config must exit 2: {stderr}"
    );
    assert!(
        stderr.contains("pumice.yaml:1"),
        "the error must name the file and line: {stderr}"
    );
}

#[test]
fn occupied_port_exits_1_naming_the_port() {
    let blocker = std::net::TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .expect("bind a port to occupy");
    let port = blocker.local_addr().unwrap().port();
    let (_fakes, yaml) = hermetic_yaml(port);
    let (_dir, config_path) = write_config(&yaml);
    let cwd = TempDir::new().expect("temp dir");
    let output = run(
        &["--config", config_path.to_str().expect("UTF-8 path")],
        cwd.path(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(1),
        "an occupied port must exit 1: {stderr}"
    );
    assert!(
        stderr.contains(&format!("port {port}")),
        "message must name the port: {stderr}"
    );
    assert!(
        stderr.contains("already in use"),
        "message must explain the conflict: {stderr}"
    );
}

/// Reads `child`'s stderr until a line starting with `prefix` arrives, then
/// kills and reaps the child. Panics with what was read when the service
/// exits early or the line never comes.
fn startup_stderr_line(child: &mut Child, prefix: &str) -> String {
    let mut stderr = std::io::BufReader::new(child.stderr.take().expect("piped stderr"));
    let mut line = String::new();
    let mut seen = String::new();
    loop {
        match stderr.read_line(&mut line) {
            Ok(0) => panic!("service exited before printing '{prefix}'; stderr so far: {seen}"),
            Ok(_) => {
                seen.push_str(&line);
                if line.starts_with(prefix) {
                    let found = line.trim_end().to_owned();
                    let _ = child.kill();
                    let _ = child.wait();
                    return found;
                }
                line.clear();
            }
            Err(error) => panic!("cannot read service stderr: {error}"),
        }
    }
}

#[test]
fn startup_prints_the_route_line_naming_the_enabled_providers() {
    let port = grab_free_port();
    let claude = FakeCli::new(json!({}));
    let codex = FakeCli::new(json!({}));
    let (_dir, path) = write_config(&format!(
        "port: {port}\nproviders:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: '{}'\n  - id: codex\n    enabled: true\n    model: gpt-6.1-sol\n    binary: '{}'\n",
        claude.path().display(),
        codex.path().display()
    ));
    let mut child = Command::new(env!("CARGO_BIN_EXE_pumice"))
        .args(["--config", path.to_str().expect("UTF-8 path")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pumice");
    let line = startup_stderr_line(&mut child, "providers:");
    assert_eq!(
        line,
        "providers: claude (haiku), codex (gpt-6.1-sol); on failure or no model: an HTTP error, the app keeps its own text; total timeout 30s"
    );
}

#[test]
fn startup_with_no_enabled_providers_prints_one_clear_line() {
    let port = grab_free_port();
    let (_dir, path) = write_config(&format!(
        "port: {port}\nproviders:\n  - id: claude\n    enabled: false\n"
    ));
    let mut child = Command::new(env!("CARGO_BIN_EXE_pumice"))
        .args(["--config", path.to_str().expect("UTF-8 path")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pumice");
    let line = startup_stderr_line(&mut child, "no providers enabled");
    assert!(
        line.contains(&format!(
            "no providers enabled: every request gets an error and the app keeps its own text (add providers to {})",
            path.display()
        )),
        "{line}"
    );
}

#[cfg(unix)]
#[test]
fn non_unicode_argument_is_a_usage_error_not_a_panic() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let dir = TempDir::new().expect("temp dir");
    let output = Command::new(env!("CARGO_BIN_EXE_pumice"))
        .arg("check-config")
        .arg("--config")
        .arg(OsStr::from_bytes(b"bad-\xff.yaml"))
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .output()
        .expect("run pumice");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("arguments must be valid Unicode"),
        "{stderr}"
    );
    assert!(!stderr.contains("panicked"), "{stderr}");
}
