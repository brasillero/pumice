//! In-process tests of `pumice serve`'s supervisor (p13.3-serve-supervisor):
//! the real supervisor, `ProcessTree` and probe client against the
//! scenario-driven `pumice-test-transcribe` acting as a managed child. No
//! real transcription module or model is involved. Timings are shortened
//! where a test would otherwise wait for the documented policy; the binary
//! tests (`pumice_serve.rs`) run the defaults. The last tests run the whole
//! service (`serve::run`) to inject panics through its test seams.

#[path = "../crates/relay/tests/support/install.rs"]
mod install;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::Instant;

use pumice_relay::process::CleanupScope;

use pumice::orchestrator::cancel::{self, CancelHandle, CancelReason};
use pumice::orchestrator::serve::child::{ChildSpec, code};
use pumice::orchestrator::serve::http::{self, Frontend, RequestHook, ServiceState};
use pumice::orchestrator::serve::output::{Outputs, PanicHook, Reporter};
use pumice::orchestrator::serve::supervisor::{
    self, Checkpoint, Checkpoints, ChildSnapshot, ChildState, Supervisor, SupervisorConfig,
    SupervisorHandle, Timings,
};
use pumice::orchestrator::serve::{self, Service};

/// Every wait in these tests is bounded by this.
const WAIT: Duration = Duration::from_secs(10);

struct Stub {
    dir: TempDir,
    binary: PathBuf,
    log: PathBuf,
}

fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// A private copy of the stub scripted with `generations`. It is a hard
/// link, made under the target directory so the link works (copying while
/// another test thread forks can fail with ETXTBSY).
fn stub(generations: Value) -> Stub {
    let dir = TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).expect("temp dir");
    let binary = dir.path().join(exe("pumice-transcribe"));
    let source = Path::new(env!("CARGO_BIN_EXE_pumice-test-transcribe"));
    install::install_executable(source, &binary);
    let log = dir.path().join("spawns.jsonl");
    let mut scenario = binary.clone().into_os_string();
    scenario.push(".scenario.json");
    fs::write(
        scenario,
        json!({"serve": {"spawn_log_path": log, "generations": generations}}).to_string(),
    )
    .expect("write the scenario");
    Stub { dir, binary, log }
}

impl Stub {
    fn log(&self, event: &str) -> Vec<Value> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|entry| entry["event"] == event)
            .collect()
    }

    fn spawns(&self) -> Vec<Value> {
        self.log("spawn")
    }

    fn config(&self, timings: Timings) -> SupervisorConfig {
        SupervisorConfig {
            spec: ChildSpec {
                binary: self.binary.clone(),
                config: self.dir.path().join("transcribe.yml"),
                model: "turbo".to_owned(),
            },
            startup_timeout: Duration::from_secs(5),
            timings,
            scope: Arc::new(CleanupScope::new()),
            checkpoints: Checkpoints::default(),
        }
    }

    /// No two children ever overlapped (checked by each spawn on Linux) and,
    /// on Unix, every spawned child is gone.
    fn assert_all_reaped(&self) {
        for spawn in self.spawns() {
            assert_ne!(spawn["prev_alive"], json!(true), "overlap: {spawn}");
            #[cfg(unix)]
            {
                let pid = spawn["pid"].as_u64().unwrap() as libc::pid_t;
                // SAFETY: signal 0 only checks for existence.
                let alive = unsafe { libc::kill(pid, 0) } == 0;
                assert!(!alive, "child {pid} still exists (unreaped or running)");
            }
        }
    }
}

/// Short clocks so recovery tests finish quickly. Probes are effectively
/// off (an aborting stub may dump core for a while, which a fast probe
/// would report first); [`probing`] turns them on.
fn fast() -> Timings {
    Timings {
        probe_interval: Duration::from_secs(30),
        probe_timeout: Duration::from_millis(150),
        probe_failures: 3,
        backoff: [
            Duration::from_millis(50),
            Duration::from_millis(100),
            Duration::from_millis(150),
        ],
        stable_after: Duration::from_secs(60),
        shutdown_grace: Duration::from_millis(500),
        reap_limit: Duration::from_secs(1),
    }
}

fn probing() -> Timings {
    Timings {
        probe_interval: Duration::from_millis(50),
        ..fast()
    }
}

/// Polls the snapshot until `condition` holds, bounded by [`WAIT`].
async fn wait_for(
    handle: &SupervisorHandle,
    what: &str,
    condition: impl Fn(&ChildSnapshot) -> bool,
) -> ChildSnapshot {
    let deadline = Instant::now() + WAIT;
    loop {
        let snapshot = handle.snapshot();
        if condition(&snapshot) {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {snapshot:?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn start(stub: &Stub, timings: Timings) -> Supervisor {
    let mut supervisor = supervisor::spawn(stub.config(timings), Reporter::silent());
    tokio::time::timeout(WAIT, supervisor.initial())
        .await
        .expect("initial result in time")
        .expect("the first child becomes ready");
    supervisor
}

async fn shutdown(supervisor: Supervisor) {
    assert!(
        supervisor.shutdown(Duration::from_secs(4)).await,
        "cleanup confirmed"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_healthy_child_is_ready_and_stops_on_stdin_eof() {
    let stub = stub(json!([{}]));
    let supervisor = start(&stub, Timings::default()).await;
    let handle = supervisor.handle();
    let snapshot = handle.snapshot();
    assert_eq!(snapshot.state, ChildState::Ready);
    assert_eq!(snapshot.generation, 1);
    assert_eq!(snapshot.model_load_ms, Some(7));
    assert!(handle.ready_child().is_some());

    let spawns = stub.spawns();
    assert_eq!(spawns.len(), 1);
    let argv: Vec<&str> = spawns[0]["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(argv[2..7], ["serve", "--model", "turbo", "--port", "0"]);
    assert_eq!(argv[7..9], ["--managed", "--instance-id"]);
    assert_eq!(argv[9], spawns[0]["instance_id"].as_str().unwrap());
    assert!(
        Path::new(argv[1]).is_absolute(),
        "absolute transcription config"
    );

    let started = Instant::now();
    shutdown(supervisor).await;
    assert!(
        started.elapsed() < Duration::from_millis(2_500),
        "cooperative stop"
    );
    assert_eq!(stub.log("eof").len(), 1, "the child saw its stdin close");
    assert_eq!(handle.snapshot().state, ChildState::Stopping);
    stub.assert_all_reaped();
}

#[tokio::test(flavor = "multi_thread")]
async fn handshake_mismatches_fail_the_startup_without_retry_or_leftover() {
    let long_line = "x".repeat(5000);
    let cases = [
        (
            json!({"ready_overrides": {"instance_id": "g9-other"}}),
            code::PROTOCOL,
        ),
        (
            json!({"ready_overrides": {"protocol_version": 2}}),
            code::PROTOCOL,
        ),
        (
            json!({"ready_overrides": {"version": "9.9.9"}}),
            code::VERSION_MISMATCH,
        ),
        (
            json!({"ready_overrides": {"model": "small"}}),
            code::MODEL_MISMATCH,
        ),
        (
            json!({"ready_overrides": {"capabilities": ["openai_transcriptions"]}}),
            code::CAPABILITIES_MISSING,
        ),
        (
            json!({"ready_overrides": {"host": "0.0.0.0"}}),
            code::ADDRESS_INVALID,
        ),
        (
            json!({"ready_overrides": {"port": 0}}),
            code::ADDRESS_INVALID,
        ),
        (json!({"ready_overrides": {"extra": true}}), code::PROTOCOL),
        (json!({"stdout_before": ["not a record"]}), code::PROTOCOL),
        (json!({"stdout_before": [long_line]}), code::PROTOCOL),
        (
            json!({"stdout_before": [r#"{"schema_version":1,"event":"stopped","module":"transcription","instance_id":"x"}"#]}),
            code::PROTOCOL,
        ),
        (
            json!({"ready_ok_count": 0, "ready_failure": "wrong_instance"}),
            code::READY_CHECK_FAILED,
        ),
        (json!({"error_code": "invalid_config"}), code::MODULE_ERROR),
    ];
    for (behavior, expected) in cases {
        let stub = stub(json!([behavior.clone()]));
        let mut supervisor = supervisor::spawn(stub.config(fast()), Reporter::silent());
        let error = tokio::time::timeout(WAIT, supervisor.initial())
            .await
            .expect("initial result in time")
            .expect_err("startup must fail");
        assert_eq!(error.failure.code, expected, "{behavior}");
        assert!(!error.cleanup_failed);
        let handle = supervisor.handle();
        assert_eq!(handle.snapshot().state, ChildState::Failed, "{behavior}");
        assert_eq!(handle.snapshot().last_failure, Some(expected));
        // The child was killed and reaped before the result was reported.
        stub.assert_all_reaped();
        shutdown(supervisor).await;
        assert_eq!(
            stub.spawns().len(),
            1,
            "initial startup is never retried: {behavior}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_startup_error_record_keeps_only_its_code() {
    let stub = stub(json!([{"error_code": "model_not_found"}]));
    let mut supervisor = supervisor::spawn(stub.config(fast()), Reporter::silent());
    let error = supervisor.initial().await.expect_err("startup fails");
    assert_eq!(error.failure.code, code::MODULE_ERROR);
    assert_eq!(error.failure.child_code, Some("model_not_found"));
    assert!(error.failure.fatal);
    shutdown(supervisor).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_child_that_never_becomes_ready_times_out() {
    let stub = stub(json!([{"never_ready": true}]));
    let mut config = stub.config(fast());
    config.startup_timeout = Duration::from_millis(300);
    let mut supervisor = supervisor::spawn(config, Reporter::silent());
    let started = Instant::now();
    let error = supervisor.initial().await.expect_err("startup fails");
    assert_eq!(error.failure.code, code::STARTUP_TIMEOUT);
    assert!(started.elapsed() < Duration::from_secs(3));
    stub.assert_all_reaped();
    shutdown(supervisor).await;
    assert_eq!(stub.spawns().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hanging_ready_check_counts_against_the_startup_timeout() {
    let stub = stub(json!([{"ready_ok_count": 0, "ready_failure": "hang"}]));
    let mut config = stub.config(fast());
    config.startup_timeout = Duration::from_millis(400);
    let mut supervisor = supervisor::spawn(config, Reporter::silent());
    let error = supervisor.initial().await.expect_err("startup fails");
    assert_eq!(error.failure.code, code::STARTUP_TIMEOUT);
    stub.assert_all_reaped();
    shutdown(supervisor).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stderr_flood_never_blocks_the_child() {
    let stub = stub(json!([{"stderr_flood_bytes": 8 * 1024 * 1024}]));
    let supervisor = start(&stub, fast()).await;
    assert_eq!(supervisor.handle().snapshot().state, ChildState::Ready);
    shutdown(supervisor).await;
    stub.assert_all_reaped();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_crash_after_ready_restarts_after_backoff_with_a_new_generation() {
    let stub = stub(json!([{"crash_after_ready_ms": 50}, {}]));
    let mut timings = fast();
    timings.backoff[0] = Duration::from_millis(400);
    let supervisor = start(&stub, timings).await;
    let handle = supervisor.handle();
    // Retirement publishes `restarting` before recovery sets the retry delay,
    // so wait for both.
    let restarting = wait_for(&handle, "restarting with its retry delay", |s| {
        s.state == ChildState::Restarting && s.next_retry_delay_ms.is_some()
    })
    .await;
    assert_eq!(restarting.restart_attempts, 1);
    assert_eq!(restarting.next_retry_delay_ms, Some(400));
    assert_eq!(restarting.last_failure, Some(code::EXITED));
    assert_eq!(handle.ready_child(), None);
    let ready = wait_for(&handle, "generation 2", |s| {
        s.state == ChildState::Ready && s.generation == 2
    })
    .await;
    assert_eq!(ready.restart_attempts, 1);
    assert_eq!(ready.next_retry_delay_ms, None);
    let spawns = stub.spawns();
    assert_eq!(spawns.len(), 2);
    assert_ne!(
        spawns[0]["instance_id"], spawns[1]["instance_id"],
        "fresh ids"
    );
    shutdown(supervisor).await;
    stub.assert_all_reaped();
}

#[tokio::test(flavor = "multi_thread")]
async fn restarts_are_bounded_per_episode_then_failed() {
    // Every generation crashes after ready; one restart fails to start
    // (a transient module error), which also uses an attempt.
    let stub = stub(json!([
        {"exit_after_ready_ms": 20},
        {"error_code": "model_load_failed"},
        {"exit_after_ready_ms": 20}
    ]));
    let supervisor = start(&stub, fast()).await;
    let handle = supervisor.handle();
    let failed = wait_for(&handle, "failed", |s| s.state == ChildState::Failed).await;
    assert_eq!(failed.restart_attempts, 3);
    assert_eq!(failed.generation, 4, "the first child and three restarts");
    assert_eq!(failed.next_retry_delay_ms, None, "no more timers");
    assert_eq!(failed.last_failure, Some(code::EXITED));
    // Stays failed: a few backoff periods later, nothing new was spawned.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(stub.spawns().len(), 4);
    assert_eq!(handle.snapshot().state, ChildState::Failed);
    shutdown(supervisor).await;
    stub.assert_all_reaped();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fatal_error_during_recovery_stops_it_at_once() {
    let stub = stub(json!([
        {"exit_after_ready_ms": 20},
        {"ready_overrides": {"version": "9.9.9"}}
    ]));
    let supervisor = start(&stub, fast()).await;
    let handle = supervisor.handle();
    let failed = wait_for(&handle, "failed", |s| s.state == ChildState::Failed).await;
    assert_eq!(failed.last_failure, Some(code::VERSION_MISMATCH));
    assert_eq!(failed.restart_attempts, 1);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(stub.spawns().len(), 2);
    shutdown(supervisor).await;
    stub.assert_all_reaped();
}

#[tokio::test(flavor = "multi_thread")]
async fn three_failed_probes_recycle_the_child() {
    for mode in ["hang", "503"] {
        // The handshake's own check is the first answer; probes then fail.
        let stub = stub(json!([{"ready_ok_count": 1, "ready_failure": mode}, {}]));
        let supervisor = start(&stub, probing()).await;
        let handle = supervisor.handle();
        let ready = wait_for(&handle, "generation 2", |s| {
            s.state == ChildState::Ready && s.generation == 2
        })
        .await;
        assert_eq!(ready.last_failure, Some(code::PROBE_FAILED), "{mode}");
        assert_eq!(ready.restart_attempts, 1);
        shutdown(supervisor).await;
        stub.assert_all_reaped();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_probe_answering_for_another_instance_is_fatal() {
    let stub = stub(json!([{"ready_ok_count": 1, "ready_failure": "wrong_instance"}]));
    let supervisor = start(&stub, probing()).await;
    let handle = supervisor.handle();
    let failed = wait_for(&handle, "failed", |s| s.state == ChildState::Failed).await;
    assert_eq!(failed.last_failure, Some(code::READY_CHECK_FAILED));
    assert_eq!(stub.spawns().len(), 1);
    shutdown(supervisor).await;
    stub.assert_all_reaped();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lifecycle_protocol_error_after_ready_is_fatal() {
    let stub = stub(json!([{"stdout_after": ["{\"event\":\"ready\"}"]}]));
    let supervisor = start(&stub, fast()).await;
    let handle = supervisor.handle();
    let failed = wait_for(&handle, "failed", |s| s.state == ChildState::Failed).await;
    assert_eq!(failed.last_failure, Some(code::PROTOCOL));
    shutdown(supervisor).await;
    stub.assert_all_reaped();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_protocol_error_written_just_before_an_exit_stays_fatal() {
    // Once the handshake's own `/ready` check is answered, the child writes
    // a broken line and exits at once: the line and the exit race, and
    // whichever the supervisor sees first, the protocol error wins and
    // nothing is restarted (p13.1-flow-seam/D5).
    for line in ["not a record", "{\"event\":\"ready\"}"] {
        let stub = stub(json!([
            {"stdout_after": [line], "exit_after_ready_checks": 1},
            {}
        ]));
        let supervisor = start(&stub, fast()).await;
        let handle = supervisor.handle();
        let failed = wait_for(&handle, "failed", |s| s.state == ChildState::Failed).await;
        assert_eq!(failed.last_failure, Some(code::PROTOCOL), "{line}");
        assert_eq!(failed.generation, 1, "{line}");
        shutdown(supervisor).await;
        assert_eq!(stub.spawns().len(), 1, "no restart after {line:?}");
        stub.assert_all_reaped();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_recycle_hook_replaces_the_ready_generation_only() {
    let stub = stub(json!([{}]));
    let supervisor = start(&stub, Timings::default()).await;
    let handle = supervisor.handle();
    assert!(!handle.recycle(2), "not the current generation");
    assert!(handle.recycle(1));
    let ready = wait_for(&handle, "generation 2", |s| {
        s.state == ChildState::Ready && s.generation == 2
    })
    .await;
    assert_eq!(ready.last_failure, Some(code::RECYCLED));
    assert_eq!(ready.restart_attempts, 0, "a recycle is not a failure");
    assert!(
        !handle.recycle(1),
        "a stale request never recycles the successor"
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(handle.snapshot().generation, 2);
    assert_eq!(stub.spawns().len(), 2);
    shutdown(supervisor).await;
    stub.assert_all_reaped();
}

/// What a frontend answered while a generation was being retired.
#[derive(Debug)]
struct Retiring {
    generation: u64,
    snapshot: ChildSnapshot,
    ready_status: u16,
    flows_ready: serde_json::Value,
    #[cfg_attr(not(unix), allow(dead_code))]
    child_alive: bool,
}

fn answer(frontend: &Frontend, path: &str) -> (u16, Value) {
    let mut headers = hyper::HeaderMap::new();
    headers.insert(hyper::header::HOST, "127.0.0.1:41234".parse().unwrap());
    let response = http::respond(
        frontend,
        &hyper::Method::GET,
        &path.parse().unwrap(),
        &headers,
    );
    let status = response.status().as_u16();
    let body = futures_bytes(response.into_body());
    (status, serde_json::from_slice(&body).unwrap())
}

fn futures_bytes(body: http_body_util::Full<bytes::Bytes>) -> bytes::Bytes {
    use http_body_util::BodyExt;
    futures_now(body.collect()).unwrap().to_bytes()
}

/// Polls a future that is ready at once (a `Full` body).
fn futures_now<F: std::future::Future>(future: F) -> F::Output {
    let waker = std::task::Waker::noop();
    let mut context = std::task::Context::from_waker(waker);
    let mut future = std::pin::pin!(future);
    match future.as_mut().poll(&mut context) {
        std::task::Poll::Ready(output) => output,
        std::task::Poll::Pending => panic!("a full body is ready at once"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_retiring_generation_is_never_offered_as_ready() {
    // A recycled generation is retired while still alive; a crashed one
    // after its exit. Either way, from the moment it is retired, the
    // snapshot, `/ready` and the flows no longer offer it.
    for recycle in [true, false] {
        let stub = stub(if recycle {
            json!([{}])
        } else {
            json!([{"exit_after_ready_ms": 20}, {}])
        });
        let seen: Arc<Mutex<Vec<Retiring>>> = Arc::default();
        let slot: Arc<OnceLock<(Frontend, SupervisorHandle, PathBuf)>> = Arc::default();
        let mut config = stub.config(fast());
        {
            let seen = Arc::clone(&seen);
            let slot = Arc::clone(&slot);
            config.checkpoints = Checkpoints::new(move |point| {
                let Checkpoint::Retiring(generation) = point else {
                    return;
                };
                let (frontend, handle, log) = slot.get().expect("set before any retirement");
                let pid = fs::read_to_string(log)
                    .unwrap_or_default()
                    .lines()
                    .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                    .filter(|entry| entry["event"] == "spawn")
                    .nth(generation as usize - 1)
                    .and_then(|entry| entry["pid"].as_u64());
                #[cfg(unix)]
                // SAFETY: signal 0 only checks for existence.
                let child_alive =
                    pid.is_some_and(|pid| unsafe { libc::kill(pid as libc::pid_t, 0) } == 0);
                #[cfg(not(unix))]
                let child_alive = pid.is_some();
                let (ready_status, _) = answer(frontend, "/ready");
                let (_, flows) = answer(frontend, "/pumice/v1/flows");
                seen.lock().unwrap().push(Retiring {
                    generation,
                    snapshot: handle.snapshot(),
                    ready_status,
                    flows_ready: flows["flows"][0]["ready"].clone(),
                    child_alive,
                });
            });
        }
        let mut supervisor = supervisor::spawn(config, Reporter::silent());
        let handle = supervisor.handle();
        let frontend = Frontend::new(
            41234,
            handle.clone(),
            "turbo".to_owned(),
            vec![http::FlowInfo {
                id: "raw".to_owned(),
                transcription_model: "turbo".to_owned(),
                formatter: None,
                relay_route: "passthrough".to_owned(),
            }],
        );
        frontend.set_state(ServiceState::Running);
        assert!(
            slot.set((frontend, handle.clone(), stub.log.clone()))
                .is_ok()
        );
        tokio::time::timeout(WAIT, supervisor.initial())
            .await
            .expect("initial result in time")
            .expect("the first child becomes ready");
        if recycle {
            assert!(handle.recycle(1));
        }
        wait_for(&handle, "generation 2", |s| {
            s.state == ChildState::Ready && s.generation == 2
        })
        .await;
        let seen = std::mem::take(&mut *seen.lock().unwrap());
        assert_eq!(seen.len(), 1, "{seen:?}");
        let retiring = &seen[0];
        assert_eq!(retiring.generation, 1);
        assert_eq!(
            retiring.snapshot.state,
            ChildState::Restarting,
            "{retiring:?}"
        );
        assert_eq!(retiring.snapshot.port, None);
        assert_eq!(retiring.snapshot.model_load_ms, None);
        assert_eq!(retiring.ready_status, 503, "{retiring:?}");
        assert_eq!(retiring.flows_ready, json!(false));
        #[cfg(unix)]
        if recycle {
            assert!(retiring.child_alive, "retired before it was killed");
        }
        shutdown(supervisor).await;
        stub.assert_all_reaped();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_child_ignoring_eof_is_killed_after_the_grace() {
    let stub = stub(json!([{"ignore_eof": true}]));
    let mut timings = fast();
    timings.shutdown_grace = Duration::from_millis(300);
    let supervisor = start(&stub, timings).await;
    let started = Instant::now();
    shutdown(supervisor).await;
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(300),
        "the grace was given: {elapsed:?}"
    );
    assert!(elapsed < Duration::from_secs(2), "then killed: {elapsed:?}");
    assert_eq!(stub.log("eof").len(), 1);
    stub.assert_all_reaped();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_slow_exit_inside_the_grace_is_waited_for() {
    let stub = stub(json!([{"exit_delay_ms": 200}]));
    let supervisor = start(&stub, Timings::default()).await;
    let started = Instant::now();
    shutdown(supervisor).await;
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(200), "{elapsed:?}");
    assert!(
        elapsed < Duration::from_millis(2_500),
        "exited on its own: {elapsed:?}"
    );
    stub.assert_all_reaped();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shutdown_during_startup_stops_the_child() {
    let stub = stub(json!([{"never_ready": true}]));
    let mut supervisor = supervisor::spawn(stub.config(fast()), Reporter::silent());
    let handle = supervisor.handle();
    wait_for(&handle, "generation 1 spawned", |s| s.generation == 1).await;
    // The spawn is logged by the child itself once it runs.
    let deadline = Instant::now() + WAIT;
    while stub.spawns().is_empty() {
        assert!(Instant::now() < deadline, "the child never started");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let pending = tokio::time::timeout(Duration::from_millis(50), supervisor.initial()).await;
    assert!(pending.is_err(), "still starting");
    let started = Instant::now();
    shutdown(supervisor).await;
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(stub.log("eof").len(), 1);
    stub.assert_all_reaped();
}

/// The whole service (`serve::run`) on `127.0.0.1:0` with the stub as its
/// child; the returned handle fires its "signal".
async fn start_service(
    config: SupervisorConfig,
    request_hook: Option<RequestHook>,
) -> (tokio::task::JoinHandle<u8>, u16, CancelHandle) {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind 127.0.0.1:0");
    let port = listener.local_addr().unwrap().port();
    let (signal, signals) = cancel::context();
    let service = Service {
        listener,
        scope: Arc::clone(&config.scope),
        supervisor: config,
        model: "turbo".to_owned(),
        flows: Vec::new(),
        dictations: None,
        request_hook,
        cleanup_hook: None,
    };
    let task = tokio::spawn(async move { serve::run(service, &Reporter::silent(), signals).await });
    (task, port, signal)
}

/// One `GET` over a real socket: the status code, or `None` when the
/// connection closed without an answer.
async fn get_status(port: u16, path: &str) -> Option<u16> {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .ok()?;
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.ok()?;
    let mut answer = Vec::new();
    let _ = tokio::time::timeout(WAIT, stream.read_to_end(&mut answer)).await;
    let answer = String::from_utf8_lossy(&answer);
    answer.strip_prefix("HTTP/1.1 ")?.get(..3)?.parse().ok()
}

async fn wait_ready(port: u16) {
    let deadline = Instant::now() + WAIT;
    while get_status(port, "/ready").await != Some(200) {
        assert!(Instant::now() < deadline, "the service never became ready");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_supervisor_panic_stops_the_service_with_exit_1() {
    let stub = stub(json!([{}]));
    let mut config = stub.config(fast());
    config.checkpoints = Checkpoints::new(|point| {
        if point == Checkpoint::Watching(1) {
            panic!("S3NT1NEL injected supervisor panic");
        }
    });
    let scope = Arc::clone(&config.scope);
    let (task, _port, _signal) = start_service(config, None).await;
    let exit = tokio::time::timeout(WAIT, task)
        .await
        .expect("the service stops on its own")
        .expect("the service task itself does not panic");
    assert_eq!(exit, 1);
    // The panic dropped the ready generation: its tree was killed, handed
    // to the scope and reaped by the service's bounded drain.
    assert!(scope.is_empty(), "nothing left unreaped");
    assert_eq!(stub.spawns().len(), 1);
    stub.assert_all_reaped();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_handler_panic_stops_the_service_with_exit_1() {
    let stub = stub(json!([{}]));
    let config = stub.config(fast());
    let hook: RequestHook = Arc::new(|path: &str| {
        if path == "/pumice/v1/status" {
            panic!("S3NT1NEL injected handler panic");
        }
    });
    let (task, port, _signal) = start_service(config, Some(hook)).await;
    wait_ready(port).await;
    let started = Instant::now();
    assert_eq!(
        get_status(port, "/pumice/v1/status").await,
        None,
        "no answer"
    );
    let exit = tokio::time::timeout(WAIT, task)
        .await
        .expect("the service stops on its own")
        .expect("the service task itself does not panic");
    assert_eq!(exit, 1);
    assert!(started.elapsed() < Duration::from_secs(5), "D9 budget");
    // The same bounded teardown as a signal: admission is gone and the
    // child was stopped cooperatively, then reaped.
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err()
    );
    assert_eq!(stub.log("eof").len(), 1, "the child saw its stdin close");
    stub.assert_all_reaped();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_signal_still_ends_the_service_with_its_own_code() {
    let stub = stub(json!([{}]));
    let (task, port, signal) = start_service(stub.config(fast()), None).await;
    wait_ready(port).await;
    signal.cancel(CancelReason::Terminate);
    let exit = tokio::time::timeout(WAIT, task).await.unwrap().unwrap();
    assert_eq!(exit, 143);
    stub.assert_all_reaped();
}

/// Set in the re-executed copy of this test binary.
const PANIC_HOOK_ENV: &str = "PUMICE_TEST_PANIC_HOOK";

#[test]
fn the_panic_hook_reports_the_location_never_the_payload() {
    if let Some(mode) = std::env::var_os(PANIC_HOOK_ENV) {
        // The re-executed copy: panic on another thread under the hook.
        let outputs = Outputs::start(mode == "json");
        let hook = PanicHook::install(outputs.reporter());
        let joined = std::thread::spawn(|| panic!("S3NT1NEL request text")).join();
        assert!(joined.is_err());
        drop(hook);
        outputs.finish();
        return;
    }
    for mode in ["plain", "json"] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "the_panic_hook_reports_the_location_never_the_payload",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(PANIC_HOOK_ENV, mode)
            .output()
            .expect("re-run this test");
        assert!(output.status.success(), "{mode}: {output:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stdout.contains("S3NT1NEL") && !stderr.contains("S3NT1NEL"),
            "{mode}: {stderr}"
        );
        if mode == "plain" {
            // The location is the source path as the compiler wrote it, so
            // Windows has `tests\serve_supervisor.rs`.
            let location = format!(
                "pumice serve: internal error: a task panicked at tests{}serve_supervisor.rs:",
                std::path::MAIN_SEPARATOR
            );
            assert!(stderr.contains(&location), "{stderr}");
        } else {
            let event = stderr
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .find(|event| event["event"] == "panicked")
                .unwrap_or_else(|| panic!("no panicked event: {stderr}"));
            assert_eq!(event["code"], "internal");
            assert_eq!(event["module"], "orchestrator");
        }
    }
}
