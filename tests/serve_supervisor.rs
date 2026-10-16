//! In-process tests of `pumice serve`'s supervisor (p13.3-serve-supervisor):
//! the real supervisor, `ProcessTree` and probe client against the
//! scenario-driven `pumice-test-transcribe` acting as a managed child. No
//! real transcription module or model is involved. Timings are shortened
//! where a test would otherwise wait for the documented policy; the binary
//! tests (`pumice_serve.rs`) run the defaults.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::time::Instant;

use pumice::orchestrator::serve::child::{ChildSpec, code};
use pumice::orchestrator::serve::output::Reporter;
use pumice::orchestrator::serve::supervisor::{
    self, ChildSnapshot, ChildState, Supervisor, SupervisorConfig, SupervisorHandle, Timings,
};

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
    if fs::hard_link(source, &binary).is_err() {
        fs::copy(source, &binary).expect("copy the stub");
    }
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
    let restarting = wait_for(&handle, "restarting", |s| s.state == ChildState::Restarting).await;
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
