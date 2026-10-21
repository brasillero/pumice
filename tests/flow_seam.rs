//! The flow execution seam (`pumice::orchestrator::flow`) driven the way a
//! caller other than the CLI drives it: an in-memory transcription transport,
//! its own arrival instant, cancellation and event sink, and the relay's
//! portable fake AI CLI behind the real Claude adapter. An integration test
//! so Cargo builds `pumice-root-test-cli` for it (p13.1-flow-seam).

#[path = "../crates/relay/tests/support/install.rs"]
mod install;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::time::Instant;

use pumice::orchestrator::cancel::{self, CancelGuard, CancelReason};
use pumice::orchestrator::config::{self, LoadedConfig};
use pumice::orchestrator::events::{EventSink, event};
use pumice::orchestrator::flow::{AudioSource, Operation, Runtime, Transcriber, prepare_flow};
use pumice::orchestrator::result::{RunEnvelope, RunOutput, StageStatus, stage};
use pumice::orchestrator::transcription::{SttFailure, TranscriptOutput};
use pumice_relay::process::CleanupScope;

fn never_cancelled() -> CancelGuard {
    let (_sender, receiver) = tokio::sync::watch::channel(None);
    receiver
}

/// The dictated text: Unicode, a newline and a trailing space, so any
/// rewrite is visible.
const RAW: &str =
    "S3NT1NEL um dictation \u{2014} \u{fc}n\u{ef}c\u{f6}d\u{e9} line\nsecond line with spaces ";
/// The fake provider's answer, distinct from [`RAW`].
const CLEANED: &str = "Cleaned dictation \u{2014} done.";
const HOUSE_RULES: &str = "HOUSE RULES: keep it short.";

const FLOWS: &str = "  - id: raw
    enabled: true
    transcription_model: turbo
    formatter: null
    relay_route: passthrough
  - id: clean
    enabled: true
    transcription_model: turbo
    formatter: builtin:clean
    relay_route: claude
  - id: plain
    enabled: true
    transcription_model: turbo
    formatter: null
    relay_route: claude
  - id: house
    enabled: true
    transcription_model: turbo
    formatter: house
    relay_route: claude
  - id: tight
    enabled: true
    transcription_model: turbo
    formatter: null
    relay_route: passthrough
    budgets_ms:
      total: 4000
";

/// An in-memory transcription transport. It records the deadline it was
/// given, signals `started` when a transcription begins and, like the real
/// transport, stops at that deadline or when the operation is cancelled.
struct FakeTranscriber {
    behavior: FakeStt,
    deadlines: Mutex<Vec<Instant>>,
    started: Arc<tokio::sync::Notify>,
}

enum FakeStt {
    Text(String),
    /// Never answers on its own.
    Hang,
}

impl FakeTranscriber {
    fn text(text: &str) -> FakeTranscriber {
        FakeTranscriber {
            behavior: FakeStt::Text(text.to_owned()),
            deadlines: Mutex::new(Vec::new()),
            started: Arc::default(),
        }
    }

    fn hang() -> FakeTranscriber {
        FakeTranscriber {
            behavior: FakeStt::Hang,
            deadlines: Mutex::new(Vec::new()),
            started: Arc::default(),
        }
    }

    fn deadlines(&self) -> Vec<Instant> {
        self.deadlines.lock().expect("lock").clone()
    }
}

impl Transcriber for FakeTranscriber {
    async fn transcribe(
        &self,
        audio: &AudioSource,
        model: &str,
        deadline: Instant,
        cancel: &CancelGuard,
    ) -> Result<TranscriptOutput, SttFailure> {
        let AudioSource::File(path) = audio else {
            panic!("the fake transcribes files only");
        };
        assert!(path.ends_with("audio.wav"));
        assert_eq!(model, "turbo");
        self.deadlines.lock().expect("lock").push(deadline);
        self.started.notify_one();
        match &self.behavior {
            FakeStt::Text(text) => Ok(TranscriptOutput {
                text: text.clone(),
                language: Some("en".to_owned()),
                audio_seconds: 2.1,
                model_load_ms: None,
                transcription_ms: Some(7),
            }),
            FakeStt::Hang => {
                let mut cancel = cancel.clone();
                tokio::select! {
                    reason = cancel::cancelled(&mut cancel) => Err(SttFailure::Cancelled(reason)),
                    () = tokio::time::sleep_until(deadline) => Err(SttFailure::Timeout),
                }
            }
        }
    }
}

/// Records every event; the sink only ever receives `&'static str`
/// metadata, so no text can cross it.
#[derive(Default)]
struct RecordingSink {
    events: Vec<(Option<&'static str>, &'static str, Option<&'static str>)>,
}

impl EventSink for RecordingSink {
    fn emit(
        &mut self,
        stage: Option<&'static str>,
        event: &'static str,
        _elapsed_ms: Option<u64>,
        code: Option<&'static str>,
    ) {
        self.events.push((stage, event, code));
    }
}

/// The portable fake AI CLI, built by Cargo for this integration test.
fn fake_cli() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_pumice-root-test-cli"))
}

/// A disposable configuration: the orchestrator file with [`FLOWS`], the
/// relay pointed at a fake Claude CLI running `fake`, a formatter
/// registry with one `house` formatter, and an audio file the fake
/// transport never reads.
struct Rig {
    dir: TempDir,
    loaded: LoadedConfig,
}

fn rig(fake: Value) -> Rig {
    // Under Cargo's target directory, so the fake CLI's hard link stays on
    // the built binary's file system.
    let dir = TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).expect("temp dir");
    let root = dir.path();
    let cli = root.join(format!("fake-claude{}", std::env::consts::EXE_SUFFIX));
    install::install_executable(fake_cli(), &cli);
    let mut scenario = fake;
    scenario["report_path"] = json!(root.join("report.json").display().to_string());
    scenario["count_path"] = json!(root.join("count.txt").display().to_string());
    scenario["report_arg_files"] = json!(["--system-prompt-file"]);
    let mut scenario_path = cli.as_os_str().to_owned();
    scenario_path.push(".scenario.json");
    fs::write(PathBuf::from(scenario_path), scenario.to_string()).expect("write scenario");

    fs::write(
        root.join("relay.yml"),
        format!(
            "port: 7567\ntotal_timeout_secs: 30\nmax_parallel: 4\nproviders:\n  - id: claude\n    enabled: true\n    model: haiku-test\n    binary: '{}'\n    timeout_secs: 30\ndebug_log:\n  enabled: false\n  path: pumice-debug.jsonl\n",
            cli.display()
        ),
    )
    .expect("write relay.yml");
    fs::write(
        root.join("house.yml"),
        format!("schema_version: 1\nid: house\ndescription: test\nsystem: '{HOUSE_RULES}'\n"),
    )
    .expect("write house.yml");
    fs::write(
        root.join("formatter.yml"),
        "schema_version: 1\nformatters:\n  - path: house.yml\n",
    )
    .expect("write formatter.yml");
    fs::write(root.join("transcribe.yml"), "schema_version: 1\n").expect("write transcribe.yml");
    fs::write(
        root.join("orchestrator.yml"),
        format!(
            "schema_version: 1\nmodules:\n  transcription:\n    config: ./transcribe.yml\n  formatter:\n    config: ./formatter.yml\n  relay:\n    location: in_process\n    config: ./relay.yml\nflows:\n{FLOWS}"
        ),
    )
    .expect("write orchestrator.yml");
    fs::write(root.join("audio.wav"), b"RIFF never read").expect("write audio");
    let loaded = config::load(Some(&root.join("orchestrator.yml"))).expect("valid config");
    Rig { dir, loaded }
}

fn answering(text: &str) -> Value {
    json!({
        "stdout": json!({"type": "result", "subtype": "success", "is_error": false, "result": text}).to_string(),
        "exit_code": 0,
    })
}

impl Rig {
    fn runtime(&self) -> Runtime {
        Runtime::build(&self.loaded, Arc::new(CleanupScope::new())).expect("runtime")
    }

    fn audio(&self) -> AudioSource {
        AudioSource::File(self.dir.path().join("audio.wav"))
    }

    fn fake_report(&self) -> Option<Value> {
        fs::read(self.dir.path().join("report.json"))
            .ok()
            .map(|bytes| serde_json::from_slice(&bytes).expect("report is JSON"))
    }

    fn fake_calls(&self) -> usize {
        fs::read_to_string(self.dir.path().join("count.txt"))
            .map(|text| text.lines().count())
            .unwrap_or(0)
    }

    /// One operation against `runtime`, as a caller other than the CLI
    /// drives it: its own arrival instant, cancellation and events.
    async fn run_on(
        &self,
        runtime: &Runtime,
        flow_id: &str,
        transcriber: &FakeTranscriber,
        arrival: Instant,
        cancel: CancelGuard,
        events: Option<&mut dyn EventSink>,
    ) -> RunOutput {
        let mut operation = Operation::start(
            "op".to_owned(),
            arrival,
            Some(flow_id.to_owned()),
            cancel,
            events,
        );
        let outcome = match prepare_flow(&self.loaded, flow_id, operation.arrival()) {
            Ok(prepared) => {
                operation
                    .run_stages(runtime, &prepared, transcriber, &self.audio())
                    .await
            }
            Err(error) => Err(error),
        };
        operation.finish(outcome, Some(runtime.scope())).await
    }

    async fn run(&self, flow_id: &str, transcriber: &FakeTranscriber) -> RunOutput {
        let runtime = self.runtime();
        self.run_on(
            &runtime,
            flow_id,
            transcriber,
            Instant::now(),
            never_cancelled(),
            None,
        )
        .await
    }
}

fn envelope(output: RunOutput) -> Value {
    let envelope: RunEnvelope = output.into_envelope();
    serde_json::to_value(&envelope).expect("serializes")
}

fn ago(duration: Duration) -> Instant {
    Instant::now()
        .checked_sub(duration)
        .expect("the monotonic clock has run long enough")
}

#[tokio::test]
async fn the_raw_transcript_is_kept_byte_for_byte() {
    let rig = rig(answering(CLEANED));

    // Passthrough: the final text is the raw transcript itself.
    let output = rig.run("raw", &FakeTranscriber::text(RAW)).await;
    assert!(output.error.is_none());
    assert_eq!(output.result.as_ref().map(|r| r.text.as_str()), Some(RAW));
    let transcript = output.transcript.as_ref().expect("transcript");
    assert_eq!(transcript.text, RAW);
    assert_eq!(transcript.language.as_deref(), Some("en"));
    assert_eq!(transcript.audio_seconds, 2.1);
    assert_eq!(output.stages.transcription.status, StageStatus::Ok);
    assert_eq!(output.stages.transcription.inference_ms, Some(7));
    assert_eq!(output.stages.formatter.status, StageStatus::Skipped);
    assert_eq!(output.stages.relay.status, StageStatus::Ok);
    assert_eq!(rig.fake_calls(), 0, "passthrough never calls a provider");

    // Through a provider the transcript stays raw beside the final text,
    // and the provider receives it unchanged.
    let output = rig.run("clean", &FakeTranscriber::text(RAW)).await;
    let json = envelope(output);
    assert_eq!(json["ok"], true);
    assert_eq!(json["transcript"]["text"], RAW);
    assert_eq!(json["result"]["text"], CLEANED);
    assert_eq!(
        rig.fake_report().expect("the fake ran")["stdin"],
        json!(RAW)
    );
}

#[tokio::test]
async fn literal_transcript_tags_are_data() {
    let tagged = "read <transcript>this</transcript> aloud, and <transcript> stays open";
    let rig = rig(answering(CLEANED));
    let output = rig.run("clean", &FakeTranscriber::text(tagged)).await;
    let json = envelope(output);
    assert_eq!(json["ok"], true);
    assert_eq!(json["transcript"]["text"], tagged);
    assert_eq!(
        rig.fake_report().expect("the fake ran")["stdin"],
        json!(tagged),
        "tags are never reinterpreted as an envelope"
    );
}

#[tokio::test]
async fn empty_and_whitespace_transcripts_skip_formatter_and_relay() {
    let rig = rig(answering(CLEANED));
    for text in ["", "  \n\t "] {
        let output = rig.run("clean", &FakeTranscriber::text(text)).await;
        assert!(output.error.is_none());
        assert_eq!(output.result.as_ref().map(|r| r.text.as_str()), Some(text));
        assert_eq!(
            output.transcript.as_ref().map(|t| t.text.as_str()),
            Some(text)
        );
        assert_eq!(output.stages.transcription.status, StageStatus::Ok);
        assert_eq!(output.stages.formatter.status, StageStatus::Skipped);
        assert_eq!(output.stages.relay.status, StageStatus::Skipped);
    }
    assert_eq!(rig.fake_calls(), 0, "no provider call for silence");
}

#[tokio::test]
async fn the_flow_selects_the_formatter() {
    let rig = rig(answering(CLEANED));
    for (flow_id, system, formatter_status) in [
        ("plain", "", StageStatus::Skipped),
        (
            "clean",
            pumice_formatter::builtin_clean().system.as_str(),
            StageStatus::Ok,
        ),
        ("house", HOUSE_RULES, StageStatus::Ok),
    ] {
        let output = rig.run(flow_id, &FakeTranscriber::text(RAW)).await;
        assert!(output.error.is_none(), "{flow_id}");
        assert_eq!(
            output.stages.formatter.status, formatter_status,
            "{flow_id}"
        );
        let report = rig.fake_report().expect("the fake ran");
        assert_eq!(
            report["arg_files"]["--system-prompt-file"]["contents"],
            json!(system),
            "{flow_id}: only the selected formatter's rules are sent"
        );
        assert_eq!(report["stdin"], json!(RAW), "{flow_id}");
    }
    assert_eq!(rig.fake_calls(), 3);
}

#[tokio::test]
async fn a_relay_failure_keeps_the_raw_transcript_and_has_no_result() {
    let rig = rig(json!({"stdout": "garbage", "exit_code": 3}));
    let output = rig.run("clean", &FakeTranscriber::text(RAW)).await;
    let error = output.error.as_ref().expect("relay failed");
    assert_eq!(error.stage, stage::RELAY);
    assert_eq!(error.code, "relay_failed");
    assert_eq!(output.exit_code(), 1);
    assert_eq!(output.stages.transcription.status, StageStatus::Ok);
    assert_eq!(output.stages.formatter.status, StageStatus::Ok);
    assert_eq!(output.stages.relay.status, StageStatus::Error);
    let json = envelope(output);
    assert_eq!(json["ok"], false);
    assert_eq!(json["transcript"]["text"], RAW);
    assert!(json.get("result").is_none(), "no final text: {json}");
    assert_eq!(json["error"]["code"], "relay_failed");
    assert_eq!(rig.fake_calls(), 1, "one call, no retry");
}

#[tokio::test]
async fn the_deadline_is_anchored_to_the_callers_instant() {
    let rig = rig(answering(CLEANED));
    let runtime = rig.runtime();
    // `tight` has a 4 s total: work stops 2.5 s earlier, 1.5 s after the
    // arrival. Its transcription cap (120 s) never binds.
    let work = Duration::from_millis(1_500);

    let fresh = Instant::now();
    let stt = FakeTranscriber::text(RAW);
    let output = rig
        .run_on(&runtime, "tight", &stt, fresh, never_cancelled(), None)
        .await;
    assert!(output.error.is_none());
    assert_eq!(stt.deadlines(), vec![fresh + work]);

    // An arrival in the past shortens what is left, never resets it.
    let earlier = ago(Duration::from_secs(1));
    let stt = FakeTranscriber::text(RAW);
    let output = rig
        .run_on(&runtime, "tight", &stt, earlier, never_cancelled(), None)
        .await;
    assert!(output.error.is_none());
    assert_eq!(stt.deadlines(), vec![earlier + work]);
    assert!(
        output.total >= Duration::from_secs(1),
        "total runs from arrival"
    );

    // A stalled transcription stops at the shortened work deadline and
    // reports the overall budget, not the stage.
    let earlier = ago(Duration::from_secs(1));
    let started = std::time::Instant::now();
    let output = rig
        .run_on(
            &runtime,
            "tight",
            &FakeTranscriber::hang(),
            earlier,
            never_cancelled(),
            None,
        )
        .await;
    assert!(
        started.elapsed() < Duration::from_millis(1_400),
        "the past arrival shortened the budget"
    );
    let error = output.error.as_ref().expect("timed out");
    assert_eq!(error.code, "flow_timeout");
    assert_eq!(error.stage, stage::TRANSCRIPTION);
    assert!(output.transcript.is_none());

    // A budget already spent at arrival fails before any work.
    let stt = FakeTranscriber::text(RAW);
    let output = rig
        .run_on(
            &runtime,
            "tight",
            &stt,
            ago(Duration::from_secs(2)),
            never_cancelled(),
            None,
        )
        .await;
    let error = output.error.as_ref().expect("spent budget");
    assert_eq!(error.code, "flow_timeout");
    assert_eq!(error.stage, stage::PREPARE);
    assert!(stt.deadlines().is_empty(), "never transcribed");
}

#[tokio::test]
async fn a_client_disconnect_cancels_the_transcription() {
    let rig = rig(answering(CLEANED));
    let runtime = rig.runtime();
    let (handle, guard) = cancel::context();
    let stt = FakeTranscriber::hang();
    // The client goes away once the transcription really runs, not during
    // the preparation before it.
    let started = Arc::clone(&stt.started);
    let canceller = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(10), started.notified())
            .await
            .expect("the transcription started");
        handle.cancel(CancelReason::ClientDisconnected);
    });
    let output = rig
        .run_on(&runtime, "clean", &stt, Instant::now(), guard, None)
        .await;
    canceller.await.expect("canceller");
    assert_eq!(stt.deadlines().len(), 1, "one transcription");
    let error = output.error.as_ref().expect("cancelled");
    assert_eq!(error.stage, stage::TRANSCRIPTION);
    assert_eq!(error.code, "cancelled");
    assert_eq!(error.cause_code.as_deref(), Some("client_disconnected"));
    assert_eq!(output.stages.transcription.status, StageStatus::Cancelled);
    assert_eq!(output.stages.relay.status, StageStatus::Skipped);
    assert!(output.transcript.is_none());
    assert_eq!(rig.fake_calls(), 0);
}

#[tokio::test]
async fn a_shutdown_cancels_the_provider_call_and_keeps_the_transcript() {
    let mut fake = answering(CLEANED);
    fake["sleep_ms"] = json!(30_000);
    let rig = rig(fake);
    let runtime = rig.runtime();
    let (handle, guard) = cancel::context();
    let count = rig.dir.path().join("count.txt");
    // Cancel once the provider is running (the fake counts its start);
    // reports whether it fired, giving up after a bounded wait.
    let canceller = tokio::spawn(async move {
        let running = tokio::time::timeout(Duration::from_secs(10), async {
            while !count.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok();
        if running {
            handle.cancel(CancelReason::Shutdown);
        }
        running
    });
    let mut sink = RecordingSink::default();
    let started = std::time::Instant::now();
    let output = rig
        .run_on(
            &runtime,
            "plain",
            &FakeTranscriber::text(RAW),
            Instant::now(),
            guard,
            Some(&mut sink),
        )
        .await;
    // An operation that ended before the provider started (a failure)
    // stops the canceller instead of leaving it waiting.
    canceller.abort();
    let fired = tokio::time::timeout(Duration::from_secs(5), canceller)
        .await
        .expect("the canceller stops");
    assert!(
        matches!(fired, Ok(true)),
        "the shutdown fired while the provider ran: {:?}",
        output.error
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "never waits for the provider"
    );
    let error = output.error.as_ref().expect("cancelled");
    assert_eq!(error.stage, stage::RELAY);
    assert_eq!(error.code, "cancelled");
    assert_eq!(error.cause_code.as_deref(), Some("service_shutdown"));
    assert_eq!(output.stages.relay.status, StageStatus::Cancelled);
    assert_eq!(
        output.transcript.as_ref().map(|t| t.text.as_str()),
        Some(RAW),
        "the raw transcript survives the cancelled call"
    );
    assert!(output.result.is_none());
    // The abandoned provider tree was reaped within the drain.
    assert!(
        !sink
            .events
            .iter()
            .any(|(stage, _, _)| *stage == Some(stage::CLEANUP)),
        "cleanup finished"
    );
    assert_eq!(
        sink.events.last(),
        Some(&(None, event::FINISHED, Some("cancelled")))
    );
}

#[tokio::test]
async fn events_follow_the_stages() {
    let rig = rig(answering(CLEANED));
    let runtime = rig.runtime();
    let mut sink = RecordingSink::default();
    let output = rig
        .run_on(
            &runtime,
            "clean",
            &FakeTranscriber::text(RAW),
            Instant::now(),
            never_cancelled(),
            Some(&mut sink),
        )
        .await;
    assert!(output.error.is_none());
    assert_eq!(
        sink.events,
        vec![
            (None, event::STARTED, None),
            (Some(stage::TRANSCRIPTION), event::STAGE_STARTED, None),
            (Some(stage::TRANSCRIPTION), event::STAGE_FINISHED, None),
            (Some(stage::FORMATTER), event::STAGE_STARTED, None),
            (Some(stage::FORMATTER), event::STAGE_FINISHED, None),
            (Some(stage::RELAY), event::STAGE_STARTED, None),
            (Some(stage::RELAY), event::STAGE_FINISHED, None),
            (None, event::FINISHED, None),
        ]
    );
}

#[tokio::test]
async fn one_runtime_serves_successive_operations() {
    let rig = rig(answering(CLEANED));
    let runtime = rig.runtime();
    for _ in 0..2 {
        let output = rig
            .run_on(
                &runtime,
                "clean",
                &FakeTranscriber::text(RAW),
                Instant::now(),
                never_cancelled(),
                None,
            )
            .await;
        assert_eq!(
            output.result.as_ref().map(|r| r.text.as_str()),
            Some(CLEANED)
        );
    }
    assert_eq!(rig.fake_calls(), 2);
}

/// A service runs operations on its own tasks: the stage runner's future
/// must be `Send`. Checked at compile time; the future is never polled.
#[tokio::test]
async fn the_stage_runner_future_is_send() {
    fn assert_send<T: Send>(_: &T) {}
    let rig = rig(answering(CLEANED));
    let runtime = rig.runtime();
    let mut sink = RecordingSink::default();
    let mut operation = Operation::start(
        "op".to_owned(),
        Instant::now(),
        None,
        never_cancelled(),
        Some(&mut sink),
    );
    let prepared = prepare_flow(&rig.loaded, "clean", Instant::now()).expect("prepared");
    let stt = FakeTranscriber::text(RAW);
    let audio = rig.audio();
    let future = operation.run_stages(&runtime, &prepared, &stt, &audio);
    assert_send(&future);
}
