//! CLI entry-point tests (S7.3 follow-up): a bare `pumice-relay` starts the
//! same service as `pumice-relay serve`, `--config` works in the bare form
//! and with the explicit `serve` command, help/version and malformed arguments never start a
//! server, and the bad-config (exit 2) and occupied-port (exit 1) contracts
//! hold. Everything runs against hermetic fake provider binaries and the
//! built-in `passthrough` model — no AI CLI is ever invoked.

mod support;

use std::fs;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::FakeCli;
use support::bounded::{self, OwnedChild, last_stage, stage};
use tempfile::TempDir;

/// Every process and HTTP wait in these tests is bounded by this.
const WAIT: Duration = Duration::from_secs(15);

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
    for id in ["claude", "codex", "kimi", "kiro"] {
        let fake = FakeCli::new(json!({}));
        yaml.push_str(&format!(
            "  - id: {id}\n    enabled: false\n    model: null\n    binary: '{}'\n    timeout_secs: 30\n",
            fake.path().display()
        ));
        fakes.push(fake);
    }
    (fakes, yaml)
}

/// Writes `yaml` to `relay.yml` inside a fresh directory and returns the
/// directory (kept alive by the caller) and the file's path.
fn write_config(yaml: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("relay.yml");
    fs::write(&path, support::relay_config(yaml)).expect("write config");
    (dir, path)
}

/// A port that was free at observation time; binding it again can in theory
/// race, so the caller retries.
fn grab_free_port() -> u16 {
    let listener = std::net::TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .expect("bind ephemeral port");
    listener.local_addr().unwrap().port()
}

/// A running `pumice-relay` service child process. Killing and reaping are
/// guaranteed: [`Server::stop`] does it for the happy path and the owner's
/// `Drop` covers panics, with a bounded wait so a stuck child cannot hang
/// the test suite.
struct Server {
    process: OwnedChild,
    port: u16,
}

impl Server {
    /// Stops the service and reaps it.
    fn stop(mut self) {
        self.process.kill();
    }
}

/// Spawns `pumice-relay` with `args`, child-specific `envs` and working directory
/// `cwd`, and waits for the startup line naming `port`. `Err` carries the
/// child's stderr for the one tolerable early exit: a lost ephemeral-port
/// race, which the caller retries.
fn try_start(
    args: &[&str],
    envs: &[(&str, &Path)],
    cwd: &Path,
    port: u16,
) -> Result<Server, String> {
    stage(format!("start pumice-relay {args:?} on port {port}"));
    let mut command = Command::new(env!("CARGO_BIN_EXE_pumice-relay"));
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
        process: OwnedChild::new("pumice-relay", command.spawn().expect("spawn pumice-relay")),
        port,
    };

    // The service prints its loopback address on stdout once it is
    // accepting connections; the end of stdout means it exited.
    let deadline = Instant::now() + WAIT;
    let Some(line) = server.process.stdout_line(deadline) else {
        server.process.wait_until(deadline);
        let output = server.process.collect_until(deadline);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("exit {}: {stderr}", output.status));
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
        "pumice-relay exited unexpectedly (attempt {attempt}): {error}"
    );
}

/// Runs `pumice-relay` with `args` to completion in `cwd` and returns its output,
/// within [`WAIT`].
fn run(args: &[&str], cwd: &Path) -> Output {
    stage(format!("run pumice-relay {args:?}"));
    let mut command = Command::new(env!("CARGO_BIN_EXE_pumice-relay"));
    command.args(args).current_dir(cwd);
    output(&mut command)
}

/// Runs `command` to completion with piped output, within [`WAIT`].
fn output(command: &mut Command) -> Output {
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run pumice-relay");
    OwnedChild::new("pumice-relay", child).finish(WAIT)
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
    stage(format!("{method} {path}"));
    let deadline = Instant::now() + WAIT;
    let fail = |what: &str| -> ! { panic!("{what} (last stage: {})", last_stage()) };
    let mut stream =
        bounded::connect(port, deadline).unwrap_or_else(|error| fail(&format!("connect: {error}")));
    bounded::send(&mut stream, &[request, body.to_vec()].concat(), deadline)
        .unwrap_or_else(|error| fail(&format!("write: {error}")));
    let response = bounded::read_response(&mut stream, deadline)
        .unwrap_or_else(|error| fail(&format!("read the response: {error:?}")));
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
fn bare_pumice_relay_starts_the_service_with_the_per_user_config() {
    for attempt in 1..=3 {
        let port = grab_free_port();
        let (fakes, yaml) = hermetic_yaml(port);
        let (config_dir, config_path) = write_config(&yaml);
        let xdg = config_dir.path().join("xdg");
        let config_home = xdg.join("pumice");
        fs::create_dir_all(&config_home).expect("create per-user config dir");
        fs::copy(&config_path, config_home.join("relay.yml")).expect("install per-user config");
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
    panic!("could not start pumice-relay on a free port after 3 attempts");
}

#[test]
fn bare_pumice_relay_with_config_flag_starts_the_service() {
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
    panic!("could not start pumice-relay on a free port after 3 attempts");
}

#[test]
fn explicit_serve_starts_the_service() {
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
    panic!("could not start pumice-relay on a free port after 3 attempts");
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
            stdout.contains("pumice-relay [--config <path>]"),
            "help must show the primary bare command: {stdout}"
        );
    }
    for flag in ["--version", "-V"] {
        let output = run(&[flag], cwd.path());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(output.status.code(), Some(0), "stderr: {:?}", output.stderr);
        assert!(
            stdout.contains(&format!("pumice-relay {}", env!("CARGO_PKG_VERSION"))),
            "version must print the crate version: {stdout}"
        );
    }
}

#[test]
fn help_after_a_subcommand_prints_its_help_without_running_it() {
    let cwd = TempDir::new().expect("temp dir");
    // `--help` anywhere among a command's flags wins, even next to flags
    // that would otherwise be a usage error or start the service.
    for (args, usage) in [
        (
            &["serve"][..],
            "usage: pumice-relay serve [--config <path>] [--plain]",
        ),
        (
            &["serve", "--plain", "--config", "missing.yml"][..],
            "usage: pumice-relay serve [--config <path>] [--plain]",
        ),
        (
            &["check-config"][..],
            "usage: pumice-relay check-config [--config <path>]",
        ),
        (
            &["--config", "missing.yml", "check-config"][..],
            "usage: pumice-relay check-config [--config <path>]",
        ),
        (
            &["doctor"][..],
            "usage: pumice-relay doctor [--config <path>] [--login-check --provider <id>]",
        ),
        (
            &["doctor", "--provider", "claude"][..],
            "usage: pumice-relay doctor [--config <path>] [--login-check --provider <id>]",
        ),
        // The bare form prints the full usage.
        (&["--plain"][..], "pumice-relay [--config <path>] [--plain]"),
        (
            &["--config", "missing.yml"][..],
            "pumice-relay [--config <path>] [--plain]",
        ),
    ] {
        for flag in ["--help", "-h"] {
            let args = [args, &[flag]].concat();
            let output = run(&args, cwd.path());
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert_eq!(
                output.status.code(),
                Some(0),
                "args {args:?}: {:?}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(stdout.contains(usage), "args {args:?} help: {stdout}");
            assert!(
                output.stderr.is_empty(),
                "args {args:?} must not write stderr: {:?}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

#[test]
fn an_option_value_spelled_like_help_is_a_value_not_a_help_flag() {
    let (_fakes, yaml) = hermetic_yaml(grab_free_port());
    let cwd = TempDir::new().expect("temp dir");
    // A config file literally named `--help`.
    fs::write(cwd.path().join("--help"), support::relay_config(&yaml)).expect("write config");
    for args in [
        &["check-config", "--config", "--help"][..],
        &["--config", "--help", "check-config"][..],
        &["doctor", "--config", "--help"][..],
        &["--config", "--help", "doctor"][..],
    ] {
        let output = run(args, cwd.path());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            output.status.code(),
            Some(0),
            "args {args:?}: {:?}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            stdout.starts_with("config: ") && stdout.contains("--help"),
            "args {args:?} must read the file named --help: {stdout}"
        );
        assert!(!stdout.contains("usage:"), "args {args:?}: {stdout}");
    }

    // `--provider --help` names a provider `--help`: an unknown one, so the
    // check stops before any call.
    for args in [
        &[
            "doctor",
            "--config",
            "--help",
            "--login-check",
            "--provider",
            "--help",
        ][..],
        &[
            "--config",
            "--help",
            "doctor",
            "--login-check",
            "--provider",
            "--help",
        ][..],
    ] {
        let output = run(args, cwd.path());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "args {args:?}: {stderr}");
        assert!(
            stderr.contains("provider \"--help\" is not registered"),
            "args {args:?}: {stderr}"
        );
        assert!(output.stdout.is_empty(), "args {args:?} printed help");
    }

    // A help flag after an option's value still asks for help.
    let output = run(&["check-config", "--config", "--help", "-h"], cwd.path());
    assert_eq!(output.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("usage: pumice-relay check-config [--config <path>]")
    );
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
fn duplicate_config_is_a_usage_error_with_command_specific_usage() {
    let cwd = TempDir::new().expect("temp dir");
    // Leading plus trailing `--config`, for each subcommand and the bare
    // form: a usage error that names its own complete usage line exactly
    // once, and never starts anything.
    for (args, usage) in [
        (
            &["--config", "a.yml", "check-config", "--config", "b.yml"][..],
            "usage: pumice-relay check-config [--config <path>]",
        ),
        (
            &["--config", "a.yml", "serve", "--config", "b.yml"][..],
            "usage: pumice-relay serve [--config <path>] [--plain]",
        ),
        (
            &["--config", "a.yml", "--plain", "--config", "b.yml"][..],
            "usage: pumice-relay [--config <path>] [--plain]",
        ),
        (
            &["--config", "a.yml", "doctor", "--config", "b.yml"][..],
            "usage: pumice-relay doctor [--config <path>] [--login-check --provider <id>]",
        ),
    ] {
        let output = run(args, cwd.path());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            output.status.code(),
            Some(2),
            "args {args:?} must exit 2: {stderr}"
        );
        assert!(
            stderr.contains(usage),
            "args {args:?} must name its own usage: {stderr}"
        );
        assert!(
            !stderr.contains("usage: usage:"),
            "args {args:?} must not nest usage prefixes: {stderr}"
        );
        assert_eq!(
            stderr.matches("usage:").count(),
            1,
            "args {args:?} must print usage once: {stderr}"
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
    let config_path = dir.path().join("relay.yml");
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
        stderr.contains("relay.yml:1"),
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

/// Reads `process`'s stderr until a line starting with `prefix` arrives
/// within [`WAIT`], then kills and reaps the process. Panics with what was
/// read when the service exits early or the line never comes.
fn startup_stderr_line(mut process: OwnedChild, prefix: &str) -> String {
    stage(format!("wait for the stderr line '{prefix}'"));
    let deadline = Instant::now() + WAIT;
    let mut seen = String::new();
    loop {
        let Some(line) = process.stderr_line(deadline) else {
            panic!("service exited before printing '{prefix}'; stderr so far: {seen}");
        };
        if line.starts_with(prefix) {
            process.kill();
            return line.trim_end().to_owned();
        }
        seen.push_str(&line);
        seen.push('\n');
    }
}

#[test]
fn startup_prints_the_route_line_naming_the_enabled_providers() {
    let port = grab_free_port();
    let claude = FakeCli::new(json!({}));
    let codex = FakeCli::new(json!({}));
    let (_dir, path) = write_config(&format!(
        "port: {port}\nproviders:\n  - id: claude\n    enabled: true\n    model: haiku\n    binary: '{}'\n    timeout_secs: 30\n  - id: codex\n    enabled: true\n    model: gpt-6.1-sol\n    binary: '{}'\n    timeout_secs: 30\n",
        claude.path().display(),
        codex.path().display()
    ));
    let child = Command::new(env!("CARGO_BIN_EXE_pumice-relay"))
        .args(["--config", path.to_str().expect("UTF-8 path")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pumice-relay");
    let line = startup_stderr_line(OwnedChild::new("pumice-relay", child), "providers:");
    assert_eq!(
        line,
        "providers: claude (haiku), codex (gpt-6.1-sol); on failure or no model: an HTTP error, the app keeps its own text; total timeout 30s, up to 4 requests at once"
    );
}

#[test]
fn startup_with_no_enabled_providers_prints_one_clear_line() {
    let port = grab_free_port();
    let (_dir, path) = write_config(&format!(
        "port: {port}\nproviders:\n  - id: claude\n    enabled: false\n    model: null\n    binary: claude\n    timeout_secs: 30\n"
    ));
    let child = Command::new(env!("CARGO_BIN_EXE_pumice-relay"))
        .args(["--config", path.to_str().expect("UTF-8 path")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pumice-relay");
    let line = startup_stderr_line(
        OwnedChild::new("pumice-relay", child),
        "no providers enabled",
    );
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
    let output = output(
        Command::new(env!("CARGO_BIN_EXE_pumice-relay"))
            .arg("check-config")
            .arg("--config")
            .arg(OsStr::from_bytes(b"bad-\xff.yaml"))
            .current_dir(dir.path()),
    );
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("arguments must be valid Unicode"),
        "{stderr}"
    );
    assert!(!stderr.contains("panicked"), "{stderr}");
}
