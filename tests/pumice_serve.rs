//! Integration tests for the `pumice serve` command (p13.3-serve-supervisor):
//! the real binary, its frontend over real loopback sockets and its
//! supervisor, with the scenario-driven `pumice-test-transcribe` installed
//! as the sibling `pumice-transcribe`. Ports are always `127.0.0.1:0`; no
//! real AI CLI, transcription module or model is involved.

#[path = "../crates/relay/tests/support/install.rs"]
mod install;

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

/// Every wait in these tests is bounded by this.
const WAIT: Duration = Duration::from_secs(15);

const FLOWS: &str = "flows:
  - id: raw
    enabled: true
    transcription_model: turbo
    formatter: null
    relay_route: passthrough
  - id: other
    enabled: false
    transcription_model: small
    formatter: null
    relay_route: passthrough
";

const SERVE: &str = "serve:
  enabled: true
  port: 0
  transcription_model: turbo
";

fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// A disposable install: `pumice` and the stub as `pumice-transcribe`, the
/// module configurations and an orchestrator file. Kept under the target
/// directory so the executables can be hard links (see serve_supervisor.rs).
struct Rig {
    dir: TempDir,
    install: PathBuf,
    cfg: PathBuf,
    log: PathBuf,
}

fn rig(serve: &str, generations: Value) -> Rig {
    let dir = TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).expect("temp dir");
    let install = dir.path().join("bin");
    let cfg = dir.path().join("cfg");
    fs::create_dir_all(&install).unwrap();
    fs::create_dir_all(&cfg).unwrap();
    for (source, name) in [
        (env!("CARGO_BIN_EXE_pumice"), "pumice"),
        (
            env!("CARGO_BIN_EXE_pumice-test-transcribe"),
            "pumice-transcribe",
        ),
    ] {
        install::install_executable(Path::new(source), &install.join(exe(name)));
    }
    let log = dir.path().join("spawns.jsonl");
    let models = json!({
        "schema_version": 1,
        "ok": true,
        "result": [
            {"id": "turbo", "path": "/models/turbo.gguf", "exists": true, "size": 1, "default": true},
            {"id": "absent", "path": "/models/absent.gguf", "exists": false, "size": null, "default": false}
        ]
    });
    let mut scenario = install.join(exe("pumice-transcribe")).into_os_string();
    scenario.push(".scenario.json");
    fs::write(
        scenario,
        json!({
            "models_list": {"stdout": format!("{models}\n"), "exit_code": 0},
            "serve": {"spawn_log_path": log, "generations": generations},
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        cfg.join("relay.yml"),
        format!(
            "providers:\n  - id: claude\n    enabled: true\n    model: haiku-test\n    binary: '{}'\n",
            env!("CARGO_BIN_EXE_pumice-test-cli")
        ),
    )
    .unwrap();
    fs::write(cfg.join("transcribe.yml"), "schema_version: 1\n").unwrap();
    fs::write(
        cfg.join("orchestrator.yml"),
        format!(
            "schema_version: 1\nmodules:\n  transcription:\n    config: ./transcribe.yml\n  relay:\n    location: in_process\n    config: ./relay.yml\n{FLOWS}{serve}"
        ),
    )
    .unwrap();
    Rig {
        dir,
        install,
        cfg,
        log,
    }
}

impl Rig {
    fn command(&self, extra: &[&str]) -> Command {
        let mut command = Command::new(self.install.join(exe("pumice")));
        command
            .arg("serve")
            .arg("--config")
            .arg(self.cfg.join("orchestrator.yml"))
            .args(extra)
            .env("XDG_CONFIG_HOME", self.dir.path())
            .env("APPDATA", self.dir.path())
            .env("HOME", self.dir.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// Runs `pumice serve` to its end (for commands that fail at once).
    fn output(&self, extra: &[&str]) -> std::process::Output {
        let child = self.command(extra).spawn().expect("start pumice serve");
        wait_output(child)
    }

    fn start(&self, extra: &[&str]) -> Running {
        let mut child = self.command(extra).spawn().expect("start pumice serve");
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (sender, records) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let _ = sender.send(line);
            }
        });
        let stderr = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = BufReader::new(stderr).read_to_string(&mut text);
            text
        });
        Running {
            child,
            records,
            stderr: Some(stderr),
        }
    }

    fn spawns(&self) -> Vec<Value> {
        self.log_events("spawn")
    }

    fn log_events(&self, event: &str) -> Vec<Value> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|entry| entry["event"] == event)
            .collect()
    }

    /// No overlapping children, and on Unix none left alive.
    fn assert_children_gone(&self) {
        for spawn in self.spawns() {
            assert_ne!(spawn["prev_alive"], json!(true), "overlap: {spawn}");
            #[cfg(unix)]
            {
                let pid = spawn["pid"].as_u64().unwrap() as libc::pid_t;
                // SAFETY: signal 0 only checks for existence.
                assert!(
                    unsafe { libc::kill(pid, 0) } != 0,
                    "child {pid} left behind"
                );
            }
        }
    }
}

/// Waits for the process with a bounded deadline (killing it past that).
fn wait_output(mut child: Child) -> std::process::Output {
    let deadline = Instant::now() + WAIT;
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("pumice serve did not exit in time");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct Running {
    child: Child,
    records: mpsc::Receiver<String>,
    stderr: Option<std::thread::JoinHandle<String>>,
}

impl Running {
    /// The next stdout record, as JSON; every stdout line must be one.
    fn record(&self) -> Value {
        let line = self
            .records
            .recv_timeout(WAIT)
            .expect("a lifecycle record in time");
        serde_json::from_str(&line).unwrap_or_else(|_| panic!("not a JSON record: {line}"))
    }

    /// Reads `listening` and returns the announced port.
    fn port(&self) -> u16 {
        let listening = self.record();
        assert_eq!(listening["event"], "listening", "{listening}");
        assert_eq!(listening["host"], "127.0.0.1");
        let port = listening["port"].as_u64().unwrap() as u16;
        assert_ne!(port, 0, "port 0 is announced as the actual port");
        port
    }

    fn ready(&self, port: u16) {
        let ready = self.record();
        assert_eq!(ready["event"], "ready", "{ready}");
        assert_eq!(ready["port"], port);
        assert_eq!(ready["version"], env!("CARGO_PKG_VERSION"));
    }

    #[cfg(unix)]
    fn signal(&self, signal: libc::c_int) {
        // SAFETY: plain kill(2) on the child this test owns.
        assert_eq!(
            unsafe { libc::kill(self.child.id() as libc::pid_t, signal) },
            0
        );
    }

    /// Waits for exit; returns (exit code, elapsed, stderr).
    fn finish(mut self) -> (Option<i32>, Duration, String) {
        let started = Instant::now();
        let deadline = started + WAIT;
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                panic!("pumice serve did not exit in time");
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let elapsed = started.elapsed();
        let stderr = self.stderr.take().unwrap().join().unwrap();
        (status.code(), elapsed, stderr)
    }

    /// Ends the service on any platform: SIGTERM where available, else a
    /// kill (the child then sees its stdin close).
    #[cfg_attr(unix, allow(unused_mut))]
    fn stop(mut self) -> (Option<i32>, Duration, String) {
        #[cfg(unix)]
        self.signal(libc::SIGTERM);
        #[cfg(not(unix))]
        let _ = self.child.kill();
        self.finish()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

type HttpResponse = (u16, Vec<(String, String)>, Value);

/// One HTTP/1.1 request over a fresh connection; the server closes it.
/// Returns (status, lowercase headers, JSON body).
fn http(port: u16, method: &str, path: &str, headers: &[(&str, String)]) -> HttpResponse {
    try_http(port, method, path, headers).expect("a complete JSON response")
}

/// [`http`], with `None` for a refused or cut-off connection.
fn try_http(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, String)],
) -> Option<HttpResponse> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(WAIT)).ok()?;
    let mut request = format!("{method} {path} HTTP/1.1\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).ok()?;
    let text = String::from_utf8(response).ok()?;
    let (head, body) = text.split_once("\r\n\r\n")?;
    let mut lines = head.lines();
    let status = lines.next()?.split(' ').nth(1)?.parse().ok()?;
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    Some((status, headers, serde_json::from_str(body).ok()?))
}

fn host(port: u16) -> (&'static str, String) {
    ("Host", format!("127.0.0.1:{port}"))
}

fn get(port: u16, path: &str) -> (u16, Value) {
    let (status, _, body) = http(port, "GET", path, &[host(port)]);
    (status, body)
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// Polls status until `condition` holds, bounded by [`WAIT`].
fn wait_status(port: u16, what: &str, condition: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + WAIT;
    loop {
        let (status, body) = get(port, "/pumice/v1/status");
        assert_eq!(status, 200);
        if condition(&body) {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {body}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn one_error_record(output: &std::process::Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "exactly one record: {stdout}");
    let record: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(record["event"], "error", "{record}");
    assert_eq!(record["module"], "orchestrator");
    record
}

#[test]
fn serves_health_ready_status_and_flows_then_stops() {
    let rig = rig(SERVE, json!([{}]));
    let running = rig.start(&["--json"]);
    let port = running.port();
    running.ready(port);

    let (status, headers, body) = http(port, "GET", "/health", &[host(port)]);
    assert_eq!(status, 200);
    assert_eq!(
        body,
        json!({"status": "ok", "version": env!("CARGO_PKG_VERSION")})
    );
    assert_eq!(header(&headers, "cache-control"), Some("no-store"));
    assert_eq!(header(&headers, "content-type"), Some("application/json"));

    let (status, body) = get(port, "/ready");
    assert_eq!(status, 200);
    assert_eq!(body["state"], "ready");

    let (status, body) = get(port, "/pumice/v1/status");
    assert_eq!(status, 200);
    assert_eq!(body["schema_version"], 1);
    assert_eq!(body["service"], json!({"state": "running", "ready": true}));
    assert_eq!(body["transcription"]["state"], "ready");
    assert_eq!(body["transcription"]["model"], "turbo");
    assert_eq!(body["transcription"]["generation"], 1);
    assert_eq!(body["transcription"]["model_load_ms"], 7);
    assert_eq!(body["flows"], json!({"enabled": 1}));
    let text = body.to_string();
    assert!(
        !text.contains(&rig.dir.path().display().to_string()),
        "no paths: {text}"
    );

    let (status, body) = get(port, "/pumice/v1/flows");
    assert_eq!(status, 200);
    assert_eq!(
        body,
        json!({
            "schema_version": 1,
            "module": "orchestrator",
            "flows": [{
                "id": "raw",
                "transcription_model": "turbo",
                "formatter": null,
                "relay_route": "passthrough",
                "ready": true,
            }],
        })
    );

    // No OpenAI routes; JSON errors with Allow on 405.
    let (status, body) = get(port, "/v1/models");
    assert_eq!(status, 404);
    assert_eq!(body["error"]["code"], "not_found");
    let (status, headers, body) = http(
        port,
        "POST",
        "/pumice/v1/status",
        &[host(port), ("Content-Length", "0".into())],
    );
    assert_eq!(status, 405);
    assert_eq!(header(&headers, "allow"), Some("GET"));
    assert_eq!(body["error"]["type"], "invalid_request_error");

    let (code, elapsed, _) = running.stop();
    if cfg!(unix) {
        assert_eq!(code, Some(143));
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
        assert_eq!(rig.log_events("eof").len(), 1, "the child saw stdin close");
    }
    rig.assert_children_gone();
}

#[test]
fn the_host_origin_and_fetch_site_guard_runs_first_over_real_sockets() {
    let rig = rig(SERVE, json!([{}]));
    let running = rig.start(&["--json"]);
    let port = running.port();
    running.ready(port);

    for headers in [
        vec![("Host", format!("localhost:{port}"))],
        vec![("Host", "127.0.0.1".to_owned())],
        vec![("Host", format!("evil.example:{port}"))],
    ] {
        let (status, response_headers, body) = http(port, "GET", "/pumice/v1/status", &headers);
        assert_eq!(status, 403, "{headers:?}");
        assert_eq!(body["error"]["code"], "host_not_allowed");
        assert!(header(&response_headers, "access-control-allow-origin").is_none());
    }
    for origin in [
        "null",
        &format!("http://localhost:{port}"),
        "http://127.0.0.1:17569",
    ] {
        let (status, response_headers, body) = http(
            port,
            "GET",
            "/nowhere",
            &[host(port), ("Origin", origin.to_owned())],
        );
        assert_eq!(status, 403, "{origin}");
        assert_eq!(body["error"]["code"], "origin_not_allowed");
        assert!(header(&response_headers, "access-control-allow-origin").is_none());
    }
    // The right Host and no Origin, but cross-site fetch metadata.
    for site in ["cross-site", "same-site"] {
        let (status, response_headers, body) = http(
            port,
            "GET",
            "/pumice/v1/status",
            &[host(port), ("Sec-Fetch-Site", site.to_owned())],
        );
        assert_eq!(status, 403, "{site}");
        assert_eq!(body["error"]["code"], "cross_site_not_allowed");
        assert!(header(&response_headers, "access-control-allow-origin").is_none());
    }
    let (status, _, _) = http(
        port,
        "GET",
        "/health",
        &[host(port), ("Origin", format!("http://127.0.0.1:{port}"))],
    );
    assert_eq!(status, 200);
    let (status, _, _) = http(
        port,
        "GET",
        "/health",
        &[host(port), ("Sec-Fetch-Site", "same-origin".to_owned())],
    );
    assert_eq!(status, 200);
    drop(running.stop());
    rig.assert_children_gone();
}

#[test]
fn status_answers_while_the_child_is_still_starting() {
    let rig = rig(SERVE, json!([{"never_ready": true}]));
    let running = rig.start(&["--json"]);
    let port = running.port();
    for _ in 0..5 {
        let started = Instant::now();
        let (status, body) = get(port, "/pumice/v1/status");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "status never waits"
        );
        assert_eq!(status, 200);
        assert_eq!(body["service"]["state"], "starting");
        assert_eq!(body["transcription"]["state"], "starting");
    }
    let (status, body) = get(port, "/ready");
    assert_eq!(status, 503);
    assert_eq!(body["error"]["code"], "service_starting");
    assert_eq!(body["error"]["type"], "server_error");
    let (status, _) = get(port, "/health");
    assert_eq!(status, 200, "health is independent of the child");
    let (_, flows) = get(port, "/pumice/v1/flows");
    assert_eq!(flows["flows"][0]["ready"], false);

    // A stop during startup is still bounded and leaves nothing behind.
    let (code, elapsed, _) = running.stop();
    if cfg!(unix) {
        assert_eq!(code, Some(143));
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    }
    rig.assert_children_gone();
}

#[test]
fn a_crash_after_ready_restarts_under_the_default_policy() {
    let rig = rig(SERVE, json!([{"exit_after_ready_ms": 100}, {}]));
    let running = rig.start(&["--json"]);
    let port = running.port();
    running.ready(port);
    let restarted = wait_status(port, "generation 2", |body| {
        body["transcription"]["generation"] == 2 && body["transcription"]["state"] == "ready"
    });
    assert_eq!(restarted["transcription"]["restart_attempts"], 1);
    assert_eq!(
        restarted["transcription"]["last_failure"],
        "transcription_exited"
    );
    assert_eq!(restarted["service"]["state"], "running");
    let (status, _) = get(port, "/ready");
    assert_eq!(status, 200);
    drop(running.stop());
    rig.assert_children_gone();
}

#[test]
fn a_handshake_mismatch_fails_the_startup_without_leftovers() {
    let rig = rig(SERVE, json!([{"ready_overrides": {"version": "9.9.9"}}]));
    let running = rig.start(&["--json"]);
    let _port = running.port();
    let error = running.record();
    assert_eq!(error["event"], "error");
    assert_eq!(error["code"], "transcription_version_mismatch");
    let (code, _, stderr) = running.finish();
    assert_eq!(code, Some(1));
    assert!(!stderr.contains("S3NT1NEL"), "no child output: {stderr}");
    assert_eq!(rig.spawns().len(), 1, "not retried");
    rig.assert_children_gone();
}

#[test]
fn a_startup_error_record_from_the_child_is_a_configuration_error() {
    let rig = rig(SERVE, json!([{"error_code": "invalid_config"}]));
    let running = rig.start(&["--json"]);
    let _port = running.port();
    let error = running.record();
    assert_eq!(error["code"], "transcription_module_error");
    assert_eq!(error["cause_code"], "invalid_config");
    assert!(
        !error.to_string().contains("S3NT1NEL"),
        "the child's message is dropped"
    );
    let (code, _, _) = running.finish();
    assert_eq!(code, Some(2));
    rig.assert_children_gone();
}

#[test]
fn configuration_errors_exit_2_before_any_child() {
    let disabled = "serve:\n  enabled: false\n";
    let reserved = "serve:\n  enabled: true\n  port: 7567\n  transcription_model: turbo\n";
    let mismatch = "serve:\n  enabled: true\n  port: 0\n  transcription_model: small\n";
    let missing_model = "serve:\n  enabled: true\n  port: 0\n  transcription_model: absent\n";
    for (serve, expect) in [
        ("", "\"serve\" is not configured"),
        (disabled, "serve is not enabled"),
        (reserved, "port 7567 is reserved"),
        (
            mismatch,
            "flow \"raw\" uses a different transcription model",
        ),
        (missing_model, "model file is missing"),
    ] {
        let rig = rig(serve, json!([{}]));
        if serve == missing_model {
            // Every enabled flow uses the served model, which is not installed.
            let path = rig.cfg.join("orchestrator.yml");
            let text = fs::read_to_string(&path).unwrap();
            fs::write(
                &path,
                text.replace("transcription_model: turbo", "transcription_model: absent"),
            )
            .unwrap();
        }
        let output = rig.output(&["--json"]);
        assert_eq!(output.status.code(), Some(2), "{expect}");
        let record = one_error_record(&output);
        let message = record["message"].as_str().unwrap();
        assert!(message.contains(expect), "{expect}: {record}");
        assert!(message.contains("orchestrator.yml"), "located: {record}");
        assert!(rig.spawns().is_empty(), "no child spawned: {expect}");
    }

    // `--port 7567` is refused the same way, as an argument error.
    let rig = rig(SERVE, json!([{}]));
    let output = rig.output(&["--json", "--port", "7567"]);
    assert_eq!(output.status.code(), Some(2));
    let record = one_error_record(&output);
    assert_eq!(record["code"], "invalid_arguments");
    assert!(record["message"].as_str().unwrap().contains("7567"));

    // Plain mode: the error and the usage on stderr, nothing on stdout.
    let output = rig.output(&["--bogus"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error: invalid_arguments"), "{stderr}");
    assert!(stderr.contains("usage: pumice serve"), "{stderr}");
}

#[test]
fn an_occupied_port_fails_with_no_fallback_and_no_child() {
    let rig = rig(SERVE, json!([{}]));
    let holder = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let taken = holder.local_addr().unwrap().port().to_string();
    let output = rig.output(&["--json", "--port", &taken]);
    assert_eq!(output.status.code(), Some(1));
    let record = one_error_record(&output);
    assert_eq!(record["code"], "port_unavailable");
    assert!(rig.spawns().is_empty(), "the child is never started");
}

#[test]
fn plain_mode_announces_the_url_on_stderr() {
    let rig = rig(SERVE, json!([{}]));
    let running = rig.start(&[]);
    // Plain mode writes nothing on stdout; wait for the child instead.
    let deadline = Instant::now() + WAIT;
    while rig.spawns().is_empty() {
        assert!(Instant::now() < deadline, "the child never started");
        std::thread::sleep(Duration::from_millis(10));
    }
    let (_, _, stderr) = running.stop();
    assert!(
        stderr.contains("pumice serve: listening on http://127.0.0.1:"),
        "{stderr}"
    );
    // Human lines only, never JSON events.
    assert!(
        !stderr.lines().any(|line| line.starts_with('{')),
        "{stderr}"
    );
    rig.assert_children_gone();
}

// Unix only: on Windows the Job Object kills the child together with
// `pumice`, so the lifeline is not what ends it there.
#[cfg(unix)]
#[test]
fn an_abrupt_parent_death_closes_the_child_lifeline() {
    // `pumice serve` killed outright runs no cleanup; the child's stdin
    // closes (no other process holds its write end), so the child ends
    // itself (design §4).
    let rig = rig(SERVE, json!([{}]));
    let mut running = rig.start(&["--json"]);
    let port = running.port();
    running.ready(port);
    running.child.kill().expect("kill pumice serve");
    let _ = running.child.wait();
    let deadline = Instant::now() + WAIT;
    while rig.log_events("eof").is_empty() {
        assert!(Instant::now() < deadline, "the child never saw stdin close");
        std::thread::sleep(Duration::from_millis(10));
    }
    let pid = rig.spawns()[0]["pid"].as_u64().unwrap() as libc::pid_t;
    // The orphan is reaped by init; wait until it is gone.
    // SAFETY: signal 0 only checks for existence.
    while unsafe { libc::kill(pid, 0) } == 0 {
        assert!(Instant::now() < deadline, "the orphaned child kept running");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
mod signals {
    //! Signal teardown is Unix-only here: Windows and macOS console teardown
    //! stays a manual check (p12-pumice-run#2, p13.1-flow-seam/D9).
    use super::*;

    #[test]
    fn ctrl_c_exits_130_after_a_cooperative_stop() {
        let rig = rig(SERVE, json!([{}]));
        let running = rig.start(&["--json"]);
        let port = running.port();
        running.ready(port);
        running.signal(libc::SIGINT);
        let stopped = running.record();
        assert_eq!(
            stopped,
            json!({"schema_version": 1, "event": "stopped", "module": "orchestrator", "exit_code": 130})
        );
        let (code, elapsed, stderr) = running.finish();
        assert_eq!(code, Some(130));
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
        for line in stderr.lines() {
            let event: Value = serde_json::from_str(line).expect("stderr is JSONL events");
            assert_eq!(event["module"], "orchestrator");
        }
        assert_eq!(rig.log_events("eof").len(), 1);
        rig.assert_children_gone();
    }

    #[test]
    fn sigterm_kills_a_child_that_ignores_eof_within_five_seconds() {
        let rig = rig(SERVE, json!([{"ignore_eof": true}]));
        let running = rig.start(&["--json"]);
        let port = running.port();
        running.ready(port);
        running.signal(libc::SIGTERM);
        let (code, elapsed, _) = running.finish();
        assert_eq!(code, Some(143));
        assert!(
            elapsed >= Duration::from_millis(2_400),
            "the grace was given: {elapsed:?}"
        );
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
        rig.assert_children_gone();
    }

    #[test]
    fn a_signal_while_the_frontend_is_polled_stops_admission() {
        let rig = rig(SERVE, json!([{"exit_delay_ms": 1500}]));
        let running = rig.start(&["--json"]);
        let port = running.port();
        running.ready(port);
        running.signal(libc::SIGTERM);
        // The child's stdin closes only after admission stopped; from then
        // on, while the child takes its time, a new connection is refused
        // or answered as stopping, never as ready.
        let deadline = Instant::now() + WAIT;
        while rig.log_events("eof").is_empty() {
            assert!(Instant::now() < deadline, "the child never saw stdin close");
            std::thread::sleep(Duration::from_millis(5));
        }
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if let Some((status, _, _)) = try_http(port, "GET", "/ready", &[host(port)]) {
                assert_ne!(status, 200, "never ready while stopping");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let (code, elapsed, _) = running.finish();
        assert_eq!(code, Some(143));
        assert!(elapsed < Duration::from_secs(5));
        rig.assert_children_gone();
    }
}
