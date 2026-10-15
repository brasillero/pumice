//! Integration tests for `pumice run` (p12-pumice-run): the combined flow of
//! the transcription sibling, the in-process formatter and the in-process
//! relay. Everything runs through fakes: the transcription sibling is the
//! scenario-driven `pumice-test-transcribe` stand-in and the provider is the
//! relay's portable `pumice-test-cli` behind the real Claude adapter,
//! pipeline and process runner. No real AI CLI or model is ever invoked.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};
use tempfile::TempDir;

/// The dictated text. Unicode, a newline and distinctive words, so leaks into
/// stderr or events are always detectable.
const RAW: &str = "S3NT1NEL um dictation — ünïcödé line\nsecond line with spaces ";

/// The fake provider's cleaned answer, distinct from [`RAW`].
const CLEANED: &str = "Cleaned dictation — done.";

/// A disposable install: `pumice`, the transcription stub and the fake
/// provider CLI, plus a config directory and an audio file.
struct Rig {
    keep: TempDir,
    install: PathBuf,
    cfg: PathBuf,
    audio: PathBuf,
    fake_dir: PathBuf,
    xdg: PathBuf,
}

/// The per-test knobs: the stub's scenario, the fake provider's scenario and
/// the `flows:` section of orchestrator.yml.
struct Script {
    stub: Value,
    fake: Value,
    flows: String,
    formatter_module: bool,
}

impl Default for Script {
    fn default() -> Script {
        Script {
            stub: stub_scenario(
                json!({
                    "schema_version": 1,
                    "ok": true,
                    "result": [{
                        "id": "turbo",
                        "path": "/models/turbo.gguf",
                        "exists": true,
                        "size": 100,
                        "default": true
                    }]
                }),
                json!({
                    "schema_version": 1,
                    "ok": true,
                    "result": {
                        "text": RAW,
                        "language": "en",
                        "audio_seconds": 2.1,
                        "model_load_ms": 350,
                        "transcription_ms": 120
                    }
                }),
            ),
            fake: json!({
                "stdout": claude_envelope(false, CLEANED),
                "exit_code": 0,
                "report_arg_files": ["--system-prompt-file"],
            }),
            flows: FLOW_CLEAN.to_owned(),
            formatter_module: true,
        }
    }
}

const FLOW_CLEAN: &str = "  - id: clean
    enabled: true
    transcription_model: turbo
    formatter: builtin:clean
    relay_route: claude
";

const FLOW_RAW: &str = "  - id: raw
    enabled: true
    transcription_model: turbo
    formatter: null
    relay_route: passthrough
";

/// A Claude success/error envelope for the fake provider's stdout.
fn claude_envelope(is_error: bool, result: &str) -> String {
    json!({
        "type": "result",
        "subtype": "success",
        "is_error": is_error,
        "result": result,
    })
    .to_string()
}

/// A stub scenario answering `models list` and `file` with the given
/// envelopes (both serialized with the trailing newline the real module
/// writes).
fn stub_scenario(models: Value, file: Value) -> Value {
    json!({
        "models_list": {"stdout": format!("{models}\n"), "exit_code": 0},
        "file": {"stdout": format!("{file}\n"), "exit_code": 0},
    })
}

fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// Copies an executable (hard link preferred: copying at the moment another
/// test thread forks can fail with ETXTBSY on Linux).
fn install_executable(src: &Path, dest: &Path) {
    if fs::hard_link(src, dest).is_err() {
        fs::copy(src, dest).expect("copy executable");
    }
}

fn make_rig(script: &Script) -> Rig {
    let keep = TempDir::new().expect("temp dir");
    let install = keep.path().join("bin");
    let cfg = keep.path().join("cfg");
    let fake_dir = keep.path().join("fake");
    let xdg = keep.path().join("xdg");
    for dir in [&install, &cfg, &fake_dir, &xdg] {
        fs::create_dir_all(dir).expect("create rig dir");
    }

    install_executable(
        Path::new(env!("CARGO_BIN_EXE_pumice")),
        &install.join(exe("pumice")),
    );
    let stub = install.join(exe("pumice-transcribe"));
    install_executable(
        Path::new(env!("CARGO_BIN_EXE_pumice-test-transcribe")),
        &stub,
    );
    let mut stub_scenario = script.stub.clone();
    stub_scenario["report_path"] = json!(
        keep.path()
            .join("stub-file-report.json")
            .display()
            .to_string()
    );
    stub_scenario["models_report_path"] = json!(
        keep.path()
            .join("stub-models-report.json")
            .display()
            .to_string()
    );
    write_scenario(&stub, &stub_scenario);

    let fake = fake_dir.join(exe("fake-claude"));
    install_executable(Path::new(env!("CARGO_BIN_EXE_pumice-test-cli")), &fake);
    let mut fake_scenario = script.fake.clone();
    if fake_scenario.get("report_path").is_none() {
        fake_scenario["report_path"] = json!(fake_dir.join("report.json").display().to_string());
    }
    fake_scenario["count_path"] = json!(fake_dir.join("count.txt").display().to_string());
    write_scenario(&fake, &fake_scenario);

    // relay.yml: the real Claude adapter pointed at the fake CLI.
    fs::write(
        cfg.join("relay.yml"),
        format!(
            "providers:\n  - id: claude\n    enabled: true\n    model: haiku-test\n    binary: '{}'\n",
            fake.display()
        ),
    )
    .expect("write relay.yml");
    fs::write(cfg.join("formatter.yml"), "schema_version: 1\n").expect("write formatter.yml");
    fs::write(cfg.join("transcribe.yml"), "schema_version: 1\n").expect("write transcribe.yml");

    let formatter_module = if script.formatter_module {
        "  formatter:\n    config: ./formatter.yml\n"
    } else {
        ""
    };
    fs::write(
        cfg.join("orchestrator.yml"),
        format!(
            "schema_version: 1\nmodules:\n  transcription:\n    config: ./transcribe.yml\n{formatter_module}  relay:\n    location: in_process\n    config: ./relay.yml\nflows:\n{}",
            script.flows
        ),
    )
    .expect("write orchestrator.yml");

    let audio = keep.path().join("audio.wav");
    fs::write(&audio, b"RIFF fake audio bytes").expect("write audio");

    Rig {
        keep,
        install,
        cfg,
        audio,
        fake_dir,
        xdg,
    }
}

fn write_scenario(exe_path: &Path, scenario: &Value) {
    let mut path = exe_path.as_os_str().to_owned();
    path.push(".scenario.json");
    fs::write(
        PathBuf::from(path),
        serde_json::to_vec_pretty(scenario).expect("serialize scenario"),
    )
    .expect("write scenario");
}

impl Rig {
    fn pumice(&self) -> PathBuf {
        self.install.join(exe("pumice"))
    }

    fn orchestrator_yml(&self) -> PathBuf {
        self.cfg.join("orchestrator.yml")
    }

    /// `pumice run <audio> --flow <flow> --config <orchestrator.yml> [--json]`
    /// with a hermetic home; extra args come first.
    fn run(&self, flow: &str, extra: &[&str], json_mode: bool) -> Output {
        let mut command = Command::new(self.pumice());
        command
            .arg("run")
            .arg(&self.audio)
            .arg("--flow")
            .arg(flow)
            .arg("--config")
            .arg(self.orchestrator_yml())
            .args(extra)
            .env("XDG_CONFIG_HOME", &self.xdg)
            .env("APPDATA", &self.xdg)
            .env("HOME", self.keep.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if json_mode {
            command.arg("--json");
        }
        command.output().expect("run pumice")
    }

    fn run_json(&self, flow: &str) -> (Output, Value) {
        let output = self.run(flow, &[], true);
        let envelope = one_json_document(&output);
        (output, envelope)
    }

    /// The fake provider's report, if it ran.
    fn fake_report(&self) -> Option<Value> {
        let path = self.fake_dir.join("report.json");
        fs::read(&path)
            .ok()
            .map(|bytes| serde_json::from_slice(&bytes).expect("fake report is JSON"))
    }

    /// How many times the fake provider ran.
    fn fake_calls(&self) -> usize {
        fs::read_to_string(self.fake_dir.join("count.txt"))
            .map(|text| text.lines().count())
            .unwrap_or(0)
    }

    /// The transcription config path the orchestrator hands the sibling.
    fn transcribe_yml(&self) -> PathBuf {
        self.cfg.join("transcribe.yml")
    }

    /// Rewrites one call of the stub's scenario after the rig exists (for
    /// envelopes that name rig paths).
    fn set_stub_behavior(&self, call: &str, behavior: Value) {
        let stub = self.install.join(exe("pumice-transcribe"));
        let mut path = stub.as_os_str().to_owned();
        path.push(".scenario.json");
        let mut scenario: Value =
            serde_json::from_slice(&fs::read(PathBuf::from(path)).expect("read scenario"))
                .expect("scenario is JSON");
        scenario[call] = behavior;
        write_scenario(&stub, &scenario);
    }

    fn stub_file_report(&self) -> Option<Value> {
        let path = self.keep.path().join("stub-file-report.json");
        fs::read(&path)
            .ok()
            .map(|bytes| serde_json::from_slice(&bytes).expect("stub report is JSON"))
    }
}

/// Asserts stdout holds exactly one JSON document (plus its newline) and
/// returns it.
fn one_json_document(output: &Output) -> Value {
    let stdout = String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8");
    let envelope: Value = serde_json::from_str(stdout.trim_end())
        .unwrap_or_else(|error| panic!("stdout is one JSON document: {error}\nstdout: {stdout}"));
    assert!(
        stdout.trim_end_matches('\n').lines().count() == 1 || !stdout.trim_end().contains('\n'),
        "exactly one JSON document on stdout: {stdout:?}"
    );
    envelope
}

fn stdout_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Every stderr event line parses, carries the envelope's operation id and is
/// metadata only: the dictated text never appears.
fn assert_events_metadata_only(output: &Output, envelope: &Value) {
    let stderr = stderr_text(output);
    assert!(
        !stderr.contains("S3NT1NEL"),
        "stderr must never carry the dictation: {stderr}"
    );
    if stderr.is_empty() {
        return;
    }
    let operation_id = envelope["operation_id"].as_str().expect("operation id");
    let mut seqs = Vec::new();
    for line in stderr.lines() {
        let event: Value = serde_json::from_str(line)
            .unwrap_or_else(|error| panic!("every stderr line is a JSON event: {error}: {line}"));
        assert_eq!(event["schema_version"], 1, "{line}");
        assert_eq!(event["module"], "orchestrator", "{line}");
        assert_eq!(event["operation_id"], operation_id, "{line}");
        assert!(event["event"].is_string(), "{line}");
        seqs.push(event["seq"].as_u64().expect("seq"));
    }
    assert!(
        seqs.windows(2).all(|pair| pair[0] < pair[1]),
        "events are ordered: {seqs:?}"
    );
}

#[test]
fn happy_path_runs_the_whole_flow_once() {
    let rig = make_rig(&Script::default());
    let (output, envelope) = rig.run_json("clean");
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        stderr_text(&output)
    );

    assert_eq!(envelope["schema_version"], 1);
    assert_eq!(envelope["module"], "orchestrator");
    assert_eq!(envelope["ok"], true);
    assert_eq!(envelope["flow"], "clean");
    assert!(
        envelope["operation_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    // Raw and final are distinct fields; the raw transcript is byte for byte.
    assert_eq!(envelope["transcript"]["text"], RAW);
    assert_eq!(envelope["transcript"]["language"], "en");
    assert_eq!(envelope["transcript"]["audio_seconds"], 2.1);
    assert_eq!(envelope["result"]["text"], CLEANED);
    assert!(envelope.get("error").is_none(), "{envelope}");
    for stage in ["prepare", "transcription", "formatter", "relay"] {
        assert_eq!(envelope["stages"][stage]["status"], "ok", "{stage}");
        assert!(
            envelope["stages"][stage]["elapsed_ms"].is_number(),
            "{stage}"
        );
    }
    assert_eq!(envelope["stages"]["transcription"]["model_load_ms"], 350);
    assert_eq!(envelope["stages"]["transcription"]["inference_ms"], 120);
    assert!(envelope["timings_ms"]["total"].is_number());
    assert_events_metadata_only(&output, &envelope);

    // The provider ran exactly once, in an empty private workspace, with
    // tools disabled and the formatter's exact system text; the raw bytes
    // are the stdin and appear nowhere in argv.
    assert_eq!(rig.fake_calls(), 1, "exactly one provider call");
    let report = rig.fake_report().expect("the fake ran");
    assert_eq!(
        report["stdin"],
        json!(RAW),
        "the raw transcript is the stdin"
    );
    assert_eq!(
        report["arg_files"]["--system-prompt-file"]["contents"],
        json!(pumice_formatter::builtin_clean().system),
        "the formatter's exact system text"
    );
    let argv: Vec<&str> = report["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|arg| arg.as_str().unwrap())
        .collect();
    assert!(
        argv.windows(2)
            .any(|pair| pair == ["--model", "haiku-test"]),
        "{argv:?}"
    );
    assert!(
        argv.windows(2).any(|pair| pair == ["--tools", ""]),
        "{argv:?}"
    );
    assert!(!argv.iter().any(|arg| arg.contains("S3NT1NEL")), "{argv:?}");
    assert_eq!(report["cwd_entries"], json!([]), "private empty workspace");

    // The provider's temporary workspace is removed afterwards.
    let workspace = PathBuf::from(report["cwd"].as_str().unwrap());
    assert!(
        !workspace.parent().unwrap().exists(),
        "the temporary root is gone"
    );

    // The sibling was called with the absolute config and audio paths.
    let stub_report = rig.stub_file_report().expect("the sibling ran");
    assert_eq!(
        stub_report["argv"],
        json!([
            "--config",
            rig.cfg.join("transcribe.yml").display().to_string(),
            "file",
            rig.audio.display().to_string(),
            "--model",
            "turbo",
            "--json"
        ]),
        "the sibling argv"
    );
}

#[test]
fn null_formatter_passthrough_returns_the_raw_bytes() {
    let rig = make_rig(&Script {
        flows: FLOW_RAW.to_owned(),
        formatter_module: false,
        ..Script::default()
    });
    let (output, envelope) = rig.run_json("raw");
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        stderr_text(&output)
    );
    assert_eq!(envelope["ok"], true);
    assert_eq!(envelope["result"]["text"], RAW, "raw byte for byte");
    assert_eq!(envelope["stages"]["formatter"]["status"], "skipped");
    assert_eq!(envelope["stages"]["relay"]["status"], "ok");
    assert_eq!(rig.fake_calls(), 0, "passthrough never calls a provider");
    assert_events_metadata_only(&output, &envelope);
}

#[test]
fn null_formatter_with_a_provider_route_adds_no_instructions() {
    let rig = make_rig(&Script {
        flows: "  - id: plain
    enabled: true
    transcription_model: turbo
    formatter: null
    relay_route: claude
"
        .to_owned(),
        formatter_module: false,
        ..Script::default()
    });
    let (output, envelope) = rig.run_json("plain");
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        stderr_text(&output)
    );
    assert_eq!(envelope["result"]["text"], CLEANED);
    assert_eq!(envelope["stages"]["formatter"]["status"], "skipped");
    let report = rig.fake_report().expect("the fake ran");
    assert_eq!(
        report["arg_files"]["--system-prompt-file"]["contents"],
        json!(""),
        "no system message is added"
    );
    assert_eq!(report["stdin"], json!(RAW));
}

#[test]
fn whitespace_only_transcript_succeeds_without_a_provider_call() {
    let script = Script {
        stub: stub_scenario(
            json!({"schema_version":1,"ok":true,"result":[{"id":"turbo","path":"/m/turbo.gguf","exists":true,"default":true}]}),
            json!({"schema_version":1,"ok":true,"result":{"text":"  \n\t ","language":null,"audio_seconds":1.4,"model_load_ms":10,"transcription_ms":5}}),
        ),
        ..Script::default()
    };
    let rig = make_rig(&script);
    let (output, envelope) = rig.run_json("clean");
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        stderr_text(&output)
    );
    assert_eq!(envelope["ok"], true);
    assert_eq!(envelope["transcript"]["text"], "  \n\t ");
    assert_eq!(
        envelope["result"]["text"], "  \n\t ",
        "identical final text"
    );
    assert_eq!(envelope["stages"]["transcription"]["status"], "ok");
    assert_eq!(envelope["stages"]["formatter"]["status"], "skipped");
    assert_eq!(envelope["stages"]["relay"]["status"], "skipped");
    assert_eq!(rig.fake_calls(), 0, "no provider call for whitespace");
}

#[test]
fn literal_transcript_tags_are_data() {
    let tagged = "read <transcript>this</transcript> aloud, and <transcript> stays open";
    let script = Script {
        stub: stub_scenario(
            json!({"schema_version":1,"ok":true,"result":[{"id":"turbo","path":"/m/turbo.gguf","exists":true,"default":true}]}),
            json!({"schema_version":1,"ok":true,"result":{"text":tagged,"language":"en","audio_seconds":1.0,"model_load_ms":1,"transcription_ms":1}}),
        ),
        ..Script::default()
    };
    let rig = make_rig(&script);
    let (output, envelope) = rig.run_json("clean");
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        stderr_text(&output)
    );
    assert_eq!(envelope["transcript"]["text"], tagged);
    let report = rig.fake_report().expect("the fake ran");
    assert_eq!(
        report["stdin"],
        json!(tagged),
        "tags are never reinterpreted as an envelope"
    );
}

// --- Configuration and argument failures (exit 2, one JSON document) -------

#[test]
fn invalid_orchestrator_config_reports_the_location() {
    let rig = make_rig(&Script::default());
    fs::write(rig.orchestrator_yml(), "schema_version: 1\nbogus: 5\n").unwrap();
    let (output, envelope) = rig.run_json("clean");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["error"]["code"], "invalid_config");
    assert_eq!(envelope["error"]["retryable"], false);
    assert_eq!(
        envelope["error"]["location"]["file"],
        json!(rig.orchestrator_yml().display().to_string())
    );
    assert_eq!(envelope["error"]["location"]["line"], 2);
    assert!(envelope["stages"]["prepare"]["status"] == "error");
    assert!(
        envelope.get("transcript").is_none(),
        "no transcript: {envelope}"
    );
    assert_eq!(rig.fake_calls(), 0, "no provider call after invalid config");
    assert!(
        rig.stub_file_report().is_none(),
        "no transcription after invalid config"
    );
}

#[test]
fn invalid_relay_config_points_into_relay_yml() {
    let rig = make_rig(&Script::default());
    fs::write(rig.cfg.join("relay.yml"), "port: [unclosed\n").unwrap();
    let (output, envelope) = rig.run_json("clean");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(envelope["error"]["code"], "invalid_config");
    assert_eq!(
        envelope["error"]["location"]["file"],
        json!(rig.cfg.join("relay.yml").display().to_string()),
        "the module file is named: {envelope}"
    );
    assert!(envelope["error"]["location"]["line"].is_number());
    assert_eq!(rig.fake_calls(), 0);
}

#[test]
fn unknown_and_disabled_flows_are_distinct_errors() {
    let rig = make_rig(&Script::default());

    let (output, envelope) = rig.run_json("missing");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(envelope["error"]["code"], "flow_not_found");

    let rig = make_rig(&Script {
        flows: "  - id: off
    enabled: false
    transcription_model: turbo
    formatter: null
    relay_route: passthrough
"
        .to_owned(),
        formatter_module: false,
        ..Script::default()
    });
    let (output, envelope) = rig.run_json("off");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(envelope["error"]["code"], "flow_disabled");
    assert_eq!(rig.fake_calls(), 0);
}

#[test]
fn forbidden_flow_combinations_are_rejected_at_load() {
    // formatter + passthrough (the rules would never execute).
    let rig = make_rig(&Script {
        flows: "  - id: bad
    enabled: true
    transcription_model: turbo
    formatter: builtin:clean
    relay_route: passthrough
"
        .to_owned(),
        ..Script::default()
    });
    let (output, envelope) = rig.run_json("bad");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(envelope["error"]["code"], "invalid_config");
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("passthrough"),
        "{envelope}"
    );

    // The inspect route is never allowed in a flow.
    let rig = make_rig(&Script {
        flows: "  - id: bad
    enabled: true
    transcription_model: turbo
    formatter: null
    relay_route: inspect
"
        .to_owned(),
        formatter_module: false,
        ..Script::default()
    });
    let (output, envelope) = rig.run_json("bad");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(envelope["error"]["code"], "invalid_config");
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("inspect"),
        "{envelope}"
    );
}

#[test]
fn a_route_that_is_not_enabled_is_a_config_error() {
    // The flow routes to kimi, but relay.yml only enables claude.
    let rig = make_rig(&Script {
        flows: "  - id: clean
    enabled: true
    transcription_model: turbo
    formatter: builtin:clean
    relay_route: kimi
"
        .to_owned(),
        ..Script::default()
    });
    let (output, envelope) = rig.run_json("clean");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(envelope["error"]["code"], "relay_route_invalid");
    // Points at the flow's `relay_route` value in orchestrator.yml.
    assert_orchestrator_location(&rig, &envelope, 15);
    assert_eq!(rig.fake_calls(), 0);
    assert!(rig.stub_file_report().is_none());

    // Plain mode names file:line:column on stderr.
    let output = rig.run("clean", &[], false);
    assert_eq!(output.status.code(), Some(2));
    let expected = format!("{}:15:", rig.orchestrator_yml().display());
    assert!(
        stderr_text(&output).contains(&expected),
        "{}",
        stderr_text(&output)
    );
}

#[test]
fn an_unregistered_formatter_is_a_config_error() {
    let rig = make_rig(&Script {
        flows: "  - id: clean
    enabled: true
    transcription_model: turbo
    formatter: no-such-formatter
    relay_route: claude
"
        .to_owned(),
        ..Script::default()
    });
    let (output, envelope) = rig.run_json("clean");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(envelope["error"]["code"], "formatter_not_found");
    // Points at the flow's `formatter` value in orchestrator.yml.
    assert_orchestrator_location(&rig, &envelope, 14);
    assert_eq!(rig.fake_calls(), 0);
}

#[test]
fn a_formatter_flow_requires_the_formatter_module() {
    let rig = make_rig(&Script {
        formatter_module: false,
        ..Script::default()
    });
    let (output, envelope) = rig.run_json("clean");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(envelope["error"]["code"], "invalid_config");
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("modules.formatter"),
        "{envelope}"
    );
}

#[test]
fn argument_errors_are_envelopes_too() {
    let rig = make_rig(&Script::default());

    // No --flow.
    let mut command = Command::new(rig.pumice());
    command
        .arg("run")
        .arg(&rig.audio)
        .arg("--json")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command.output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let envelope = one_json_document(&output);
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["error"]["code"], "invalid_arguments");
    assert!(envelope.get("flow").is_none(), "no flow field: {envelope}");

    // Without --json: empty stdout, usage on stderr.
    let output = Command::new(rig.pumice())
        .arg("run")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "no stdout without --json");
    assert!(stderr_text(&output).contains("usage:"));

    // --help prints usage and exits 0.
    let output = Command::new(rig.pumice())
        .args(["run", "--help"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout_text(&output).contains("usage: pumice run"));
}

#[test]
fn missing_config_file_is_an_error() {
    let rig = make_rig(&Script::default());
    let gone = rig.keep.path().join("gone.yml");
    let output = Command::new(rig.pumice())
        .arg("run")
        .arg(&rig.audio)
        .args(["--flow", "clean", "--config"])
        .arg(&gone)
        .arg("--json")
        .env("XDG_CONFIG_HOME", &rig.xdg)
        .env("APPDATA", &rig.xdg)
        .env("HOME", rig.keep.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let envelope = one_json_document(&output);
    assert_eq!(envelope["error"]["code"], "invalid_config");
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("gone.yml"),
        "{envelope}"
    );
}

#[test]
fn per_user_orchestrator_yml_is_the_default_config() {
    let rig = make_rig(&Script::default());
    // Move the orchestrator file into the hermetic per-user directory.
    let per_user = rig.xdg.join("pumice");
    fs::create_dir_all(&per_user).unwrap();
    // The module paths are relative to the orchestrator file: write a
    // self-contained copy there.
    for name in ["relay.yml", "formatter.yml", "transcribe.yml"] {
        fs::copy(rig.cfg.join(name), per_user.join(name)).unwrap();
    }
    fs::copy(rig.orchestrator_yml(), per_user.join("orchestrator.yml")).unwrap();

    let output = Command::new(rig.pumice())
        .arg("run")
        .arg(&rig.audio)
        .args(["--flow", "clean", "--json"])
        .env("XDG_CONFIG_HOME", &rig.xdg)
        .env("APPDATA", &rig.xdg)
        .env("HOME", rig.keep.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        stderr_text(&output)
    );

    // And its absence is an error naming the expected path.
    let rig = make_rig(&Script::default());
    let output = Command::new(rig.pumice())
        .arg("run")
        .arg(&rig.audio)
        .args(["--flow", "clean", "--json"])
        .env("XDG_CONFIG_HOME", &rig.xdg)
        .env("APPDATA", &rig.xdg)
        .env("HOME", rig.keep.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let envelope = one_json_document(&output);
    assert_eq!(envelope["error"]["code"], "invalid_config");
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("orchestrator.yml"),
        "{envelope}"
    );
}

// --- The sibling lookup -----------------------------------------------------

#[test]
fn a_missing_sibling_is_not_installed_and_never_uses_path() {
    let rig = make_rig(&Script::default());
    // Remove the sibling and plant a decoy on PATH and in the cwd that would
    // happily answer if either were searched.
    fs::remove_file(rig.install.join(exe("pumice-transcribe"))).unwrap();
    fs::remove_file(
        rig.install
            .join(format!("{}.scenario.json", exe("pumice-transcribe"))),
    )
    .unwrap();
    let decoy_dir = rig.keep.path().join("decoys");
    fs::create_dir_all(&decoy_dir).unwrap();
    install_executable(
        Path::new(env!("CARGO_BIN_EXE_pumice-test-transcribe")),
        &decoy_dir.join(exe("pumice-transcribe")),
    );

    let output = Command::new(rig.pumice())
        .arg("run")
        .arg(&rig.audio)
        .args(["--flow", "clean", "--config"])
        .arg(rig.orchestrator_yml())
        .arg("--json")
        .env("PATH", &decoy_dir)
        .env("XDG_CONFIG_HOME", &rig.xdg)
        .env("APPDATA", &rig.xdg)
        .env("HOME", rig.keep.path())
        .current_dir(&decoy_dir)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let envelope = one_json_document(&output);
    assert_eq!(envelope["error"]["code"], "transcription_not_installed");
    assert!(envelope.get("transcript").is_none());
    assert_eq!(rig.fake_calls(), 0);
}

#[cfg(unix)]
#[test]
fn a_symlinked_pumice_finds_the_sibling_next_to_the_real_executable() {
    use std::os::unix::ffi::OsStrExt;

    let rig = make_rig(&Script::default());
    // A launcher directory with only a symlink: current_exe canonicalizes
    // through it, so the sibling resolves next to the real pumice.
    let launcher = rig.keep.path().join("launcher");
    fs::create_dir_all(&launcher).unwrap();
    std::os::unix::fs::symlink(rig.pumice(), launcher.join(exe("pumice"))).unwrap();

    // A non-UTF-8 audio path survives the whole way to the sibling.
    let weird = rig
        .keep
        .path()
        .join(std::ffi::OsStr::from_bytes(b"audio-\xff.wav"));
    fs::write(&weird, b"RIFF fake").unwrap();
    let audio_arg = rig.keep.path().join("audio-arg.bin");
    let mut scenario: Value = serde_json::from_slice(
        &fs::read(
            rig.install
                .join(format!("{}.scenario.json", exe("pumice-transcribe"))),
        )
        .unwrap(),
    )
    .unwrap();
    scenario["audio_arg_path"] = json!(audio_arg.display().to_string());
    write_scenario(&rig.install.join(exe("pumice-transcribe")), &scenario);

    let mut command = Command::new(launcher.join(exe("pumice")));
    command
        .arg("run")
        .arg(std::ffi::OsStr::from_bytes(weird.as_os_str().as_bytes()))
        .args(["--flow", "clean", "--config"])
        .arg(rig.orchestrator_yml())
        .arg("--json")
        .env("XDG_CONFIG_HOME", &rig.xdg)
        .env("APPDATA", &rig.xdg)
        .env("HOME", rig.keep.path())
        .stdin(Stdio::null());
    let output = command.output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        stderr_text(&output)
    );
    assert_eq!(
        fs::read(&audio_arg).unwrap(),
        weird.as_os_str().as_bytes(),
        "the audio path arrives byte for byte"
    );
}

#[test]
fn an_install_directory_with_spaces_and_unicode_works() {
    let script = Script::default();
    let spaced_root = TempDir::new().unwrap();
    let spaced = spaced_root.path().join("my install ünï");
    fs::create_dir_all(&spaced).unwrap();
    // Rebuild a rig inside the spaced directory by moving a fresh rig's
    // install dir; simpler: construct the rig then copy install over.
    let rig = make_rig(&script);
    let moved = spaced.join("bin");
    fs::create_dir_all(&moved).unwrap();
    // The whole install directory, scenario file included.
    for entry in fs::read_dir(&rig.install).unwrap() {
        let entry = entry.unwrap();
        install_executable(&entry.path(), &moved.join(entry.file_name()));
    }
    let output = Command::new(moved.join(exe("pumice")))
        .arg("run")
        .arg(&rig.audio)
        .args(["--flow", "clean", "--config"])
        .arg(rig.orchestrator_yml())
        .arg("--json")
        .env("XDG_CONFIG_HOME", &rig.xdg)
        .env("APPDATA", &rig.xdg)
        .env("HOME", rig.keep.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        stderr_text(&output)
    );
}

// --- Transcription failures --------------------------------------------------

/// Asserts a failure envelope: one JSON document, stable code, no transcript,
/// no sentinel anywhere but stdout's transcript field.
fn assert_failure(output: &Output, envelope: &Value, exit: i32, code: &str, stage: &str) {
    assert_eq!(
        output.status.code(),
        Some(exit),
        "exit {exit} for {code}: {}",
        stderr_text(output)
    );
    assert_eq!(envelope["ok"], false, "{envelope}");
    assert_eq!(envelope["error"]["code"], code, "{envelope}");
    assert_eq!(envelope["error"]["stage"], stage, "{envelope}");
    assert_eq!(envelope["error"]["retryable"], false, "{envelope}");
    assert!(envelope.get("result").is_none(), "no result: {envelope}");
    assert_events_metadata_only(output, envelope);
}

/// Asserts the error points at `file:line` of the orchestrator file, with a
/// column.
fn assert_orchestrator_location(rig: &Rig, envelope: &Value, line: u64) {
    assert_eq!(
        envelope["error"]["location"]["file"],
        json!(rig.orchestrator_yml().display().to_string()),
        "{envelope}"
    );
    assert_eq!(envelope["error"]["location"]["line"], line, "{envelope}");
    assert!(
        envelope["error"]["location"]["column"].is_number(),
        "{envelope}"
    );
}

fn stub_with_file_behavior(file: Value) -> Script {
    let mut script = Script::default();
    script.stub["file"] = file;
    script
}

#[test]
fn stt_crash_and_malformed_envelopes_are_protocol_errors() {
    for (name, file) in [
        ("crash", json!({"crash": true})),
        ("bad json", json!({"stdout": "not json at all\n"})),
        (
            "wrong version",
            json!({"stdout": "{\"schema_version\":2,\"ok\":true,\"result\":{}}\n"}),
        ),
        (
            "two documents",
            json!({"stdout": "{\"schema_version\":1,\"ok\":true,\"result\":{\"text\":\"x\",\"language\":null,\"audio_seconds\":1.0}}\n{\"ok\":true}\n"}),
        ),
        (
            "ok but nonzero exit",
            json!({"stdout": "{\"schema_version\":1,\"ok\":true,\"result\":{\"text\":\"x\",\"language\":null,\"audio_seconds\":1.0}}\n", "exit_code": 1}),
        ),
        (
            "error but clean exit",
            json!({"stdout": "{\"schema_version\":1,\"ok\":false,\"error\":{\"code\":\"engine_failed\",\"message\":\"boom\"}}\n", "exit_code": 0}),
        ),
    ] {
        let rig = make_rig(&stub_with_file_behavior(file));
        let (output, envelope) = rig.run_json("clean");
        assert_failure(
            &output,
            &envelope,
            1,
            "transcription_protocol_error",
            "transcription",
        );
        assert!(
            envelope.get("transcript").is_none(),
            "{name}: partial output is not a transcript: {envelope}"
        );
        assert_eq!(rig.fake_calls(), 0, "{name}: no provider call");
    }
}

#[test]
fn stt_oversized_stdout_is_a_protocol_error() {
    let rig = make_rig(&stub_with_file_behavior(json!({
        "stdout": "{\"schema_version\":1,\"ok\":true,\"result\":",
        "stdout_repeat_bytes": 2 * 1024 * 1024,
    })));
    let (output, envelope) = rig.run_json("clean");
    assert_failure(
        &output,
        &envelope,
        1,
        "transcription_protocol_error",
        "transcription",
    );
    assert_eq!(rig.fake_calls(), 0);
}

#[test]
fn stt_module_errors_map_to_stable_codes() {
    // An engine failure: runtime error, exit 1, a fixed message per code —
    // the child's own message is never forwarded.
    let rig = make_rig(&stub_with_file_behavior(json!({
        "stdout": "{\"schema_version\":1,\"ok\":false,\"error\":{\"code\":\"engine_failed\",\"message\":\"the engine exploded\"}}\n",
        "exit_code": 1,
    })));
    let (output, envelope) = rig.run_json("clean");
    assert_failure(
        &output,
        &envelope,
        1,
        "transcription_engine_failed",
        "transcription",
    );
    assert_eq!(envelope["error"]["cause_code"], "engine_failed");
    assert_eq!(
        envelope["error"]["message"], "the transcription engine failed",
        "the fixed message, not the child's: {envelope}"
    );
    assert!(envelope.get("transcript").is_none());

    // A missing model file: input/config class, exit 2.
    let rig = make_rig(&stub_with_file_behavior(json!({
        "stdout": "{\"schema_version\":1,\"ok\":false,\"error\":{\"code\":\"model_not_found\",\"message\":\"model file not found: /m/turbo.gguf\"}}\n",
        "exit_code": 2,
    })));
    let (output, envelope) = rig.run_json("clean");
    assert_failure(
        &output,
        &envelope,
        2,
        "transcription_model_not_found",
        "transcription",
    );

    // An unknown child code is a protocol error, never inferred from text.
    let rig = make_rig(&stub_with_file_behavior(json!({
        "stdout": "{\"schema_version\":1,\"ok\":false,\"error\":{\"code\":\"surprise\",\"message\":\"S3NT1NEL\"}}\n",
        "exit_code": 1,
    })));
    let (output, envelope) = rig.run_json("clean");
    assert_failure(
        &output,
        &envelope,
        1,
        "transcription_protocol_error",
        "transcription",
    );
    assert!(
        !stdout_text(&output).contains("S3NT1NEL"),
        "the child's message is not forwarded: {envelope}"
    );
}

#[test]
fn hostile_child_messages_never_reach_stderr_or_the_envelope() {
    // A misbehaving build puts dictated text into its error message for
    // every recognized code: the orchestrator reports a fixed message per
    // code, in both modes, and never the child's.
    const SENTINEL: &str = "PRIVATE_TRANSCRIPT_SENTINEL";
    for (code, exit) in [
        ("engine_failed", 1),
        ("model_load_failed", 1),
        ("engine_not_available", 1),
        ("internal", 1),
        ("invalid_arguments", 2),
        ("invalid_input", 2),
        ("input_too_large", 2),
        ("audio_too_long", 2),
        ("unsupported_format", 2),
        ("invalid_config", 2),
        ("model_not_found", 2),
    ] {
        let child = json!({"schema_version": 1, "ok": false, "error": {
            "code": code,
            "message": format!("failed on: {SENTINEL} {RAW}"),
        }});
        let rig = make_rig(&stub_with_file_behavior(json!({
            "stdout": format!("{child}\n"),
            "exit_code": exit,
        })));

        let (output, envelope) = rig.run_json("clean");
        assert_eq!(output.status.code(), Some(exit), "{code}: {envelope}");
        assert_eq!(envelope["error"]["cause_code"], code, "{envelope}");
        assert!(
            !stdout_text(&output).contains(SENTINEL),
            "{code}: the child's message is not in the envelope: {envelope}"
        );
        assert!(
            !stderr_text(&output).contains(SENTINEL),
            "{code}: nor in the events: {}",
            stderr_text(&output)
        );
        assert_events_metadata_only(&output, &envelope);

        let output = rig.run("clean", &[], false);
        assert_eq!(output.status.code(), Some(exit), "{code}");
        let stderr = stderr_text(&output);
        assert!(!stderr.is_empty(), "{code}: plain mode reports on stderr");
        assert!(
            !stderr.contains(SENTINEL) && !stderr.contains("S3NT1NEL"),
            "{code}: plain stderr never carries the child's message: {stderr}"
        );
        assert!(stdout_text(&output).is_empty(), "{code}: no result");
    }

    // The same holds for the preflight `models list` call.
    let mut script = Script::default();
    script.stub["models_list"] = json!({
        "stdout": format!("{}\n", json!({"schema_version": 1, "ok": false, "error": {
            "code": "invalid_config",
            "message": SENTINEL,
        }})),
        "exit_code": 2,
    });
    let rig = make_rig(&script);
    let (output, envelope) = rig.run_json("clean");
    assert_failure(&output, &envelope, 2, "invalid_config", "prepare");
    assert!(!stdout_text(&output).contains(SENTINEL), "{envelope}");
    assert!(!stderr_text(&output).contains(SENTINEL));
    let output = rig.run("clean", &[], false);
    assert!(!stderr_text(&output).contains(SENTINEL));
}

#[test]
fn stt_extra_stderr_is_discarded() {
    let rig = make_rig(&stub_with_file_behavior(json!({
        "stdout": format!("{}\n", json!({"schema_version":1,"ok":true,"result":{"text":"hi there","language":"en","audio_seconds":1.0}})),
        "stderr": "noisy S3NT1NEL diagnostics\n",
        "exit_code": 0,
    })));
    let (output, envelope) = rig.run_json("clean");
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        stderr_text(&output)
    );
    assert_eq!(envelope["transcript"]["text"], "hi there");
    assert!(
        !stderr_text(&output).contains("noisy"),
        "child stderr never reaches our stderr"
    );
}

#[test]
fn models_list_failures_happen_in_preflight() {
    // The transcription config is invalid: the sibling says so and the flow
    // stops before any inference.
    let rig = make_rig(&Script::default());
    rig.set_stub_behavior(
        "models_list",
        json!({
            "stdout": format!(
                "{}\n",
                json!({"schema_version": 1, "ok": false, "error": {
                    "code": "invalid_config",
                    "message": "bad model entry",
                    "location": {"file": rig.transcribe_yml(), "line": 4, "column": 2},
                }})
            ),
            "exit_code": 2,
        }),
    );
    let (output, envelope) = rig.run_json("clean");
    assert_failure(&output, &envelope, 2, "invalid_config", "prepare");
    assert_eq!(envelope["error"]["cause_code"], "invalid_config");
    assert_eq!(
        envelope["error"]["location"]["file"],
        json!(rig.transcribe_yml().display().to_string())
    );
    assert_eq!(envelope["error"]["location"]["line"], 4);
    assert_eq!(envelope["error"]["location"]["column"], 2);
    assert!(rig.stub_file_report().is_none(), "no file call");
    assert_eq!(rig.fake_calls(), 0);

    // A location naming any other file is not the child's config: dropped,
    // since its path could carry anything.
    let mut script = Script::default();
    script.stub["models_list"] = json!({
        "stdout": "{\"schema_version\":1,\"ok\":false,\"error\":{\"code\":\"invalid_config\",\"message\":\"bad\",\"location\":{\"file\":\"/S3NT1NEL/transcribe.yml\",\"line\":4,\"column\":2}}}\n",
        "exit_code": 2,
    });
    let rig = make_rig(&script);
    let (output, envelope) = rig.run_json("clean");
    assert_failure(&output, &envelope, 2, "invalid_config", "prepare");
    assert!(envelope["error"].get("location").is_none(), "{envelope}");
    assert!(!stdout_text(&output).contains("S3NT1NEL"), "{envelope}");

    // The flow names a model the transcription config does not have.
    let rig2 = make_rig(&Script {
        flows: "  - id: clean
    enabled: true
    transcription_model: no-such-model
    formatter: builtin:clean
    relay_route: claude
"
        .to_owned(),
        ..Script::default()
    });
    let (output, envelope) = rig2.run_json("clean");
    assert_failure(
        &output,
        &envelope,
        2,
        "transcription_model_not_found",
        "prepare",
    );
    // Points at the flow's `transcription_model` value in orchestrator.yml.
    assert_orchestrator_location(&rig2, &envelope, 13);

    // The model exists in the config but its file is missing.
    let mut script = Script::default();
    script.stub["models_list"] = json!({
        "stdout": "{\"schema_version\":1,\"ok\":true,\"result\":[{\"id\":\"turbo\",\"path\":\"/m/turbo.gguf\",\"exists\":false,\"default\":true}]}\n",
        "exit_code": 0,
    });
    let rig = make_rig(&script);
    let (output, envelope) = rig.run_json("clean");
    assert_failure(
        &output,
        &envelope,
        2,
        "transcription_model_not_found",
        "prepare",
    );
    assert_orchestrator_location(&rig, &envelope, 13);
    assert!(rig.stub_file_report().is_none());
}

// --- Formatter and relay failures after transcription ------------------------

#[test]
fn relay_failures_keep_the_raw_transcript() {
    for (name, stdout, exit_code, code) in [
        (
            "not logged in",
            claude_envelope(true, "Not logged in · Please run /login"),
            0,
            "relay_not_logged_in",
        ),
        (
            "quota",
            claude_envelope(true, "You've hit your weekly limit · resets Oct 8"),
            0,
            "relay_quota_exceeded",
        ),
        (
            "rate limited",
            claude_envelope(true, "API Error: 429 rate_limit_error"),
            0,
            "relay_rate_limited",
        ),
        (
            "empty reply",
            claude_envelope(false, "   \n"),
            0,
            "relay_invalid_output",
        ),
        ("crash", "garbage".to_owned(), 3, "relay_failed"),
    ] {
        let rig = make_rig(&Script {
            fake: json!({"stdout": stdout, "exit_code": exit_code}),
            ..Script::default()
        });
        let (output, envelope) = rig.run_json("clean");
        assert_failure(&output, &envelope, 1, code, "relay");
        // The raw transcript survives the failed paid call.
        assert_eq!(envelope["transcript"]["text"], RAW, "{name}: {envelope}");
        assert_eq!(envelope["stages"]["transcription"]["status"], "ok");
        assert_eq!(envelope["stages"]["formatter"]["status"], "ok");
        assert_eq!(envelope["stages"]["relay"]["status"], "error");
        assert_eq!(rig.fake_calls(), 1, "{name}: one call, no retry");
    }
}

#[test]
fn failure_never_prints_the_raw_transcript_as_the_result() {
    let rig = make_rig(&Script {
        fake: json!({"stdout": claude_envelope(true, "Not logged in"), "exit_code": 0}),
        ..Script::default()
    });
    // Plain mode: stdout stays empty on failure.
    let output = rig.run("clean", &[], false);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty(), "no raw text as final output");
    let stderr = stderr_text(&output);
    assert!(stderr.contains("relay_not_logged_in"), "{stderr}");
    assert!(
        stderr.contains("--json") && stderr.contains("repeats the whole flow"),
        "says where the transcript is and what a rerun costs: {stderr}"
    );
    assert!(
        !stderr.contains("preserved"),
        "claims no stored transcript: {stderr}"
    );
    assert!(!stderr.contains("S3NT1NEL"), "{stderr}");
}

#[test]
fn plain_mode_success_prints_only_the_final_text() {
    let rig = make_rig(&Script::default());
    let output = rig.run("clean", &[], false);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        stderr_text(&output)
    );
    assert_eq!(stdout_text(&output), format!("{CLEANED}\n"));
    assert!(
        stderr_text(&output).is_empty(),
        "no events without --json: {}",
        stderr_text(&output)
    );
}

// --- Deadlines (D2): one flow clock that stages can only shorten ------------

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    libc_kill(pid, 0) == 0
}

#[cfg(unix)]
fn libc_kill(pid: u32, signal: libc::c_int) -> i32 {
    // Wrapped so the unsafe block is an ordinary call expression.
    unsafe { libc::kill(pid as i32, signal) }
}

/// Waits until a JSON file with `pid` (and optionally a grandchild) exists.
#[cfg(unix)]
fn wait_for_pids(path: &Path, grandchild: bool) -> (u32, Option<u32>) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if let Ok(text) = fs::read_to_string(path)
            && let Ok(value) = serde_json::from_str::<Value>(&text)
            && let Some(pid) = value["pid"].as_u64()
        {
            let grandchild_pid = value["grandchild_pid"].as_u64();
            if !grandchild || grandchild_pid.is_some() {
                return (pid as u32, grandchild_pid.map(|pid| pid as u32));
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no pid file at {}",
            path.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Waits until every listed pid is gone (zombie counts as gone here: its
/// parent is being tested for reaping separately through the exit code).
#[cfg(unix)]
fn wait_until_all_gone(pids: &[u32]) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if pids.iter().all(|&pid| {
            !pid_alive(pid) || {
                // A zombie is reaped by definition once its parent exits; the
                // ps check treats zombies as gone, like the relay tests do.
                let out = Command::new("ps")
                    .args(["-o", "stat=", "-p", &pid.to_string()])
                    .output()
                    .expect("ps");
                String::from_utf8_lossy(&out.stdout).trim().starts_with('Z')
            }
        }) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "pids {pids:?} survive"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn the_transcription_stage_cap_shortens_the_flow() {
    let mut script = Script::default();
    script.stub["file"] = json!({"sleep_ms": 30_000, "exit_code": 0});
    script.flows = "  - id: clean
    enabled: true
    transcription_model: turbo
    formatter: builtin:clean
    relay_route: claude
    budgets_ms:
      total: 60000
      transcription: 800
"
    .to_owned();
    let rig = make_rig(&script);
    let started = std::time::Instant::now();
    let (output, envelope) = rig.run_json("clean");
    assert_failure(
        &output,
        &envelope,
        1,
        "transcription_timeout",
        "transcription",
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(15),
        "the sleeping stub was cut short"
    );
    assert!(
        envelope.get("transcript").is_none(),
        "no transcript on timeout"
    );
    assert_eq!(rig.fake_calls(), 0, "no provider call");
}

#[test]
fn a_stalled_models_list_is_cut_by_the_flow_budget() {
    // The preflight subprocess has its own 5 s cap inside the remaining
    // budget; here the 3.3 s total budget (0.8 s of work) fires first.
    let mut script = Script::default();
    script.stub["models_list"] = json!({"sleep_ms": 30_000, "exit_code": 0});
    script.flows = "  - id: clean
    enabled: true
    transcription_model: turbo
    formatter: builtin:clean
    relay_route: claude
    budgets_ms:
      total: 3300
"
    .to_owned();
    let rig = make_rig(&script);
    let started = std::time::Instant::now();
    let (output, envelope) = rig.run_json("clean");
    assert_failure(&output, &envelope, 1, "flow_timeout", "prepare");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(15),
        "the stalled preflight was cut short"
    );
    assert!(rig.stub_file_report().is_none(), "no inference ran");
    assert_eq!(rig.fake_calls(), 0);
}

#[test]
fn the_flow_budget_never_resets_between_stages() {
    // Total 3.3 s → the work budget ends 0.8 s after start; the generous
    // transcription cap must not reset that clock.
    let mut script = Script::default();
    script.stub["file"] = json!({"sleep_ms": 30_000, "exit_code": 0});
    script.flows = "  - id: clean
    enabled: true
    transcription_model: turbo
    formatter: builtin:clean
    relay_route: claude
    budgets_ms:
      total: 3300
"
    .to_owned();
    let rig = make_rig(&script);
    let started = std::time::Instant::now();
    let (output, envelope) = rig.run_json("clean");
    assert_failure(&output, &envelope, 1, "flow_timeout", "transcription");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(15),
        "the overall budget won over the stage cap"
    );
}

#[test]
fn the_relay_stage_cap_shortens_the_provider_call() {
    let rig = make_rig(&Script {
        fake: json!({
            "stdout": claude_envelope(false, "too late"),
            "exit_code": 0,
            "sleep_ms": 30_000,
        }),
        flows: "  - id: clean
    enabled: true
    transcription_model: turbo
    formatter: builtin:clean
    relay_route: claude
    budgets_ms:
      total: 60000
      relay: 800
"
        .to_owned(),
        ..Script::default()
    });
    let started = std::time::Instant::now();
    let (output, envelope) = rig.run_json("clean");
    assert_failure(&output, &envelope, 1, "relay_timeout", "relay");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(15),
        "the sleeping provider was cut short"
    );
    // The raw transcript survives a provider timeout.
    assert_eq!(envelope["transcript"]["text"], RAW);
    assert!(envelope.get("result").is_none());
}

#[test]
fn a_relay_timeout_at_the_overall_budget_is_a_flow_timeout() {
    // Total 3.3 s with the default stage caps: the overall budget (0.8 s of
    // work after the 2.5 s reserve) is what limits the stalled provider,
    // even though the pipeline's response reserve fires it shortly before
    // the work deadline.
    let rig = make_rig(&Script {
        fake: json!({
            "stdout": claude_envelope(false, "too late"),
            "exit_code": 0,
            "sleep_ms": 30_000,
        }),
        flows: "  - id: clean
    enabled: true
    transcription_model: turbo
    formatter: builtin:clean
    relay_route: claude
    budgets_ms:
      total: 3300
"
        .to_owned(),
        ..Script::default()
    });
    let started = std::time::Instant::now();
    let (output, envelope) = rig.run_json("clean");
    assert_failure(&output, &envelope, 1, "flow_timeout", "relay");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(15),
        "the sleeping provider was cut short"
    );
    assert_eq!(rig.fake_calls(), 1, "the provider did start");
    assert_eq!(envelope["transcript"]["text"], RAW);
}

/// A pipe whose buffer is already full, so the next blocking write to its
/// write end blocks until someone reads (nobody will). `std::io::pipe`
/// creates both ends close-on-exec, so a test spawning concurrently never
/// inherits them.
#[cfg(unix)]
fn full_pipe() -> (std::io::PipeReader, std::io::PipeWriter) {
    use std::os::fd::AsRawFd;
    let (read, write) = std::io::pipe().expect("pipe");
    let fd = write.as_raw_fd();
    // SAFETY: fcntl and write on a descriptor we own. O_NONBLOCK lives on the
    // open file description, so it is cleared again before the child
    // inherits it.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        assert!(flags >= 0);
        assert_eq!(libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK), 0);
        let chunk = [b'x'; 4096];
        while libc::write(fd, chunk.as_ptr().cast(), chunk.len()) > 0 {}
        while libc::write(fd, chunk.as_ptr().cast(), 1) > 0 {}
        assert_eq!(libc::fcntl(fd, libc::F_SETFL, flags), 0);
    }
    (read, write)
}

/// Waits for our own child with a bound; on timeout kills it (still
/// unreaped, so the PID is ours) and returns `None`.
#[cfg(unix)]
fn wait_bounded(
    child: &mut std::process::Child,
    limit: std::time::Duration,
) -> Option<std::process::ExitStatus> {
    let deadline = std::time::Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().expect("poll child") {
            return Some(status);
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[cfg(unix)]
#[test]
fn a_broken_stdout_with_a_blocked_stderr_still_exits() {
    // stdout's reader is gone (EPIPE) and stderr is full and never read:
    // the run must still exit 1 promptly instead of blocking on stderr, in
    // both modes.
    let rig = make_rig(&Script {
        flows: FLOW_RAW.to_owned(),
        ..Script::default()
    });
    for json_mode in [true, false] {
        let (stderr_read, stderr_write) = full_pipe();
        let (stdout_read, stdout_write) = std::io::pipe().expect("pipe");
        drop(stdout_read);
        let mut command = Command::new(rig.pumice());
        command
            .arg("run")
            .arg(&rig.audio)
            .args(["--flow", "raw", "--config"])
            .arg(rig.orchestrator_yml())
            .env("XDG_CONFIG_HOME", &rig.xdg)
            .env("HOME", rig.keep.path())
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout_write))
            .stderr(Stdio::from(stderr_write));
        if json_mode {
            command.arg("--json");
        }
        let mut child = command.spawn().expect("spawn pumice run");
        // The Command still holds copies of the write ends until dropped.
        drop(command);

        let status = wait_bounded(&mut child, std::time::Duration::from_secs(15))
            .unwrap_or_else(|| panic!("pumice run hung on a blocked stderr (json: {json_mode})"));
        assert_eq!(status.code(), Some(1), "json: {json_mode}");
        drop(stderr_read);
    }
}

// --- Cancellation: Ctrl-C and SIGTERM reap every child -----------------------

/// Spawns `pumice run --json` for the rig's flow and returns the child.
#[cfg(unix)]
fn spawn_run(rig: &Rig, flow: &str) -> std::process::Child {
    Command::new(rig.pumice())
        .arg("run")
        .arg(&rig.audio)
        .args(["--flow", flow, "--config"])
        .arg(rig.orchestrator_yml())
        .arg("--json")
        .env("XDG_CONFIG_HOME", &rig.xdg)
        .env("APPDATA", &rig.xdg)
        .env("HOME", rig.keep.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pumice run")
}

#[cfg(unix)]
fn send_signal(pid: u32, signal: libc::c_int) {
    assert_eq!(libc_kill(pid, signal), 0);
}

#[cfg(unix)]
#[test]
fn ctrl_c_during_transcription_kills_and_reaps_the_tree() {
    let pid_file;
    let mut script = Script::default();
    {
        let probe = TempDir::new().unwrap();
        pid_file = probe.keep().join("stub-pids.json");
    }
    script.stub["file"] = json!({"sleep_ms": 30_000, "spawn_grandchild_sleep_ms": 30_000});
    script.stub["pid_path"] = json!(pid_file.display().to_string());
    let rig = make_rig(&script);

    let child = spawn_run(&rig, "clean");
    let (stub_pid, grandchild) = wait_for_pids(&pid_file, true);
    send_signal(child.id(), libc::SIGINT);
    let output = child.wait_with_output().expect("reap pumice run");

    assert_eq!(output.status.code(), Some(130), "Ctrl-C exits 130");
    let envelope = one_json_document(&output);
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["error"]["code"], "cancelled");
    assert_eq!(envelope["error"]["stage"], "transcription");
    assert!(envelope.get("transcript").is_none(), "STT never completed");
    assert_eq!(envelope["stages"]["transcription"]["status"], "cancelled");
    assert_eq!(envelope["stages"]["formatter"]["status"], "skipped");
    assert_eq!(envelope["stages"]["relay"]["status"], "skipped");
    assert_eq!(rig.fake_calls(), 0, "no provider call after cancellation");
    assert_events_metadata_only(&output, &envelope);

    wait_until_all_gone(&[stub_pid, grandchild.expect("grandchild pid")]);
}

#[cfg(unix)]
#[test]
fn sigterm_during_transcription_exits_143() {
    let pid_file;
    let mut script = Script::default();
    {
        let probe = TempDir::new().unwrap();
        pid_file = probe.keep().join("stub-pids.json");
    }
    script.stub["file"] = json!({"sleep_ms": 30_000});
    script.stub["pid_path"] = json!(pid_file.display().to_string());
    let rig = make_rig(&script);

    let child = spawn_run(&rig, "clean");
    let (stub_pid, _) = wait_for_pids(&pid_file, false);
    send_signal(child.id(), libc::SIGTERM);
    let output = child.wait_with_output().expect("reap pumice run");

    assert_eq!(output.status.code(), Some(143), "SIGTERM exits 143");
    let envelope = one_json_document(&output);
    assert_eq!(envelope["error"]["code"], "cancelled");
    wait_until_all_gone(&[stub_pid]);
}

#[cfg(unix)]
#[test]
fn a_signal_right_after_the_started_event_is_handled() {
    // The handlers are registered before the `started` event: a SIGTERM the
    // moment it appears gets the envelope and exit 143, never the default
    // signal death.
    use std::io::BufRead;

    let mut script = Script::default();
    script.stub["file"] = json!({"sleep_ms": 30_000, "exit_code": 0});
    let rig = make_rig(&script);
    let mut child = spawn_run(&rig, "clean");
    let mut stderr = std::io::BufReader::new(child.stderr.take().expect("stderr"));
    let mut first = String::new();
    stderr.read_line(&mut first).expect("read the first event");
    let event: Value = serde_json::from_str(&first).expect("an event");
    assert_eq!(event["event"], "started", "{first}");
    send_signal(child.id(), libc::SIGTERM);

    let mut rest = String::new();
    std::io::Read::read_to_string(&mut stderr, &mut rest).expect("drain stderr");
    let output = child.wait_with_output().expect("reap pumice run");
    assert_eq!(output.status.code(), Some(143), "{first}{rest}");
    let envelope = one_json_document(&output);
    assert_eq!(envelope["error"]["code"], "cancelled");
    assert!(!rest.contains("S3NT1NEL"));
}

#[cfg(unix)]
#[test]
fn ctrl_c_during_the_provider_call_kills_and_reaps_the_tree() {
    let rig = make_rig(&Script {
        fake: json!({
            "stdout": claude_envelope(false, "too late"),
            "exit_code": 0,
            "sleep_ms": 30_000,
            "spawn_grandchild_sleep_ms": 30_000,
        }),
        ..Script::default()
    });

    let child = spawn_run(&rig, "clean");
    // The fake writes its report (with both pids) before sleeping.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while rig.fake_report().is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "the provider never ran"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let report = rig.fake_report().unwrap();
    let provider_pid = report["pid"].as_u64().unwrap() as u32;
    let grandchild_pid = report["grandchild_pid"].as_u64().unwrap() as u32;

    send_signal(child.id(), libc::SIGINT);
    let output = child.wait_with_output().expect("reap pumice run");

    assert_eq!(output.status.code(), Some(130));
    let envelope = one_json_document(&output);
    assert_eq!(envelope["error"]["code"], "cancelled");
    assert_eq!(envelope["error"]["stage"], "relay");
    // The completed transcript survives the cancellation.
    assert_eq!(envelope["transcript"]["text"], RAW);
    assert!(envelope.get("result").is_none());
    assert_eq!(envelope["stages"]["relay"]["status"], "cancelled");
    assert_events_metadata_only(&output, &envelope);

    // The provider tree — parent and grandchild holding the pipes — is gone.
    wait_until_all_gone(&[provider_pid, grandchild_pid]);
}
