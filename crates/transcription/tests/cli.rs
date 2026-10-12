use pumice_transcription::cli::{SuccessOutput, print_error, print_success, run_with_engine};
use pumice_transcription::{
    Engine, EngineError, ErrorCode, FakeEngine, Transcript, TranscriptionOptions,
};
use std::io::Cursor;
use std::process::Command;

fn write_wav_f32(samples: &[f32], sample_rate: u32, channels: u16) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::new(&mut buf, spec).unwrap();
    for &s in samples {
        writer.write_sample(s).unwrap();
    }
    writer.finalize().unwrap();
    buf.into_inner()
}

fn write_wav_i8(samples: &[i8], sample_rate: u32, channels: u16) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 8,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::new(&mut buf, spec).unwrap();
    for &s in samples {
        writer.write_sample(s).unwrap();
    }
    writer.finalize().unwrap();
    buf.into_inner()
}

fn write_silence_wav(duration_secs: f64, sample_rate: u32, channels: u16) -> Vec<u8> {
    let samples = vec![0.0f32; (duration_secs * sample_rate as f64) as usize * channels as usize];
    write_wav_f32(&samples, sample_rate, channels)
}

#[derive(Debug, Clone)]
struct RecordingFakeEngine {
    pcm_len: std::sync::Arc<std::sync::Mutex<usize>>,
    language: std::sync::Arc<std::sync::Mutex<Option<String>>>,
}

impl RecordingFakeEngine {
    fn new() -> Self {
        Self {
            pcm_len: std::sync::Arc::new(std::sync::Mutex::new(0)),
            language: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }

    fn pcm_len(&self) -> usize {
        *self.pcm_len.lock().unwrap()
    }

    fn language(&self) -> Option<String> {
        self.language.lock().unwrap().clone()
    }
}

impl Engine for RecordingFakeEngine {
    fn transcribe(
        &self,
        pcm_16khz_mono: &[f32],
        options: &TranscriptionOptions,
    ) -> Result<Transcript, EngineError> {
        *self.pcm_len.lock().unwrap() = pcm_16khz_mono.len();
        *self.language.lock().unwrap() = options.language.clone();
        Ok(Transcript {
            text: "hello world".to_string(),
            language: Some("pt".to_string()),
        })
    }
}

#[derive(Debug, Clone)]
struct NoLanguageFakeEngine;

impl Engine for NoLanguageFakeEngine {
    fn transcribe(
        &self,
        _pcm_16khz_mono: &[f32],
        _options: &TranscriptionOptions,
    ) -> Result<Transcript, EngineError> {
        Ok(Transcript {
            text: "hello".to_string(),
            language: None,
        })
    }
}

#[derive(Debug, Clone)]
struct FailingFakeEngine;

impl Engine for FailingFakeEngine {
    fn transcribe(
        &self,
        _pcm_16khz_mono: &[f32],
        _options: &TranscriptionOptions,
    ) -> Result<Transcript, EngineError> {
        Err(EngineError::Failed)
    }
}

#[test]
fn fake_engine_full_path() {
    let tmp = tempfile::tempdir().unwrap();
    let audio_path = tmp.path().join("audio.wav");
    std::fs::write(&audio_path, write_silence_wav(1.0, 16_000, 1)).unwrap();
    let model_path = tmp.path().join("model.gguf");
    std::fs::write(&model_path, b"").unwrap();

    let engine = RecordingFakeEngine::new();
    let result = run_with_engine(
        vec![
            "pumice-transcribe",
            "file",
            audio_path.to_str().unwrap(),
            "--model",
            model_path.to_str().unwrap(),
            "--language",
            "pt",
        ],
        Box::new(engine.clone()),
    )
    .unwrap();

    assert_eq!(result.text, "hello world");
    assert_eq!(result.language, Some("pt".to_string()));
    assert!((result.audio_seconds - 1.0).abs() < 0.01);
    assert_eq!(engine.pcm_len(), 16_000);
    assert_eq!(engine.language(), Some("pt".to_string()));
}

#[test]
fn fake_engine_failure_returns_engine_failed() {
    let tmp = tempfile::tempdir().unwrap();
    let audio_path = tmp.path().join("audio.wav");
    std::fs::write(&audio_path, write_silence_wav(1.0, 16_000, 1)).unwrap();
    let model_path = tmp.path().join("model.gguf");
    std::fs::write(&model_path, b"").unwrap();

    let err = run_with_engine(
        vec![
            "pumice-transcribe",
            "file",
            audio_path.to_str().unwrap(),
            "--model",
            model_path.to_str().unwrap(),
        ],
        Box::new(FailingFakeEngine),
    )
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::EngineFailed);
}

#[test]
fn utf8_transcript_is_preserved() {
    let tmp = tempfile::tempdir().unwrap();
    let audio_path = tmp.path().join("audio.wav");
    std::fs::write(&audio_path, write_silence_wav(1.0, 16_000, 1)).unwrap();
    let model_path = tmp.path().join("model.gguf");
    std::fs::write(&model_path, b"").unwrap();

    let accented = "Olá, como vai você?";
    let result = run_with_engine(
        vec![
            "pumice-transcribe",
            "file",
            audio_path.to_str().unwrap(),
            "--model",
            model_path.to_str().unwrap(),
        ],
        Box::new(FakeEngine::new(accented, Some("pt"))),
    )
    .unwrap();

    assert_eq!(result.text, accented);
}

#[test]
fn missing_model_returns_model_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let audio_path = tmp.path().join("audio.wav");
    std::fs::write(&audio_path, write_silence_wav(1.0, 16_000, 1)).unwrap();

    let err = run_with_engine(
        vec![
            "pumice-transcribe",
            "file",
            audio_path.to_str().unwrap(),
            "--model",
            "/does/not/exist.gguf",
        ],
        Box::new(FakeEngine::new("x", None::<String>)),
    )
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::ModelNotFound);
}

#[test]
fn missing_audio_file_returns_invalid_input() {
    let err = run_with_engine(
        vec![
            "pumice-transcribe",
            "file",
            "/does/not/exist.wav",
            "--model",
            "/dev/null",
        ],
        Box::new(FakeEngine::new("x", None::<String>)),
    )
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::InvalidInput);
}

#[test]
fn non_wav_file_returns_invalid_input() {
    let tmp = tempfile::tempdir().unwrap();
    let audio_path = tmp.path().join("audio.txt");
    std::fs::write(&audio_path, b"not a wav file").unwrap();

    let err = run_with_engine(
        vec![
            "pumice-transcribe",
            "file",
            audio_path.to_str().unwrap(),
            "--model",
            "/dev/null",
        ],
        Box::new(FakeEngine::new("x", None::<String>)),
    )
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::InvalidInput);
}

#[test]
fn non_regular_file_returns_invalid_input() {
    // /dev/null is a character device, not a regular file.
    let err = run_with_engine(
        vec![
            "pumice-transcribe",
            "file",
            "/dev/null",
            "--model",
            "/dev/null",
        ],
        Box::new(FakeEngine::new("x", None::<String>)),
    )
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::InvalidInput);
}

#[test]
#[cfg(target_os = "linux")]
fn fifo_without_writer_returns_invalid_input_with_timeout() {
    let tmp = tempfile::tempdir().unwrap();
    let fifo_path = tmp.path().join("no_writer.fifo");

    let status = std::process::Command::new("mkfifo")
        .arg(&fifo_path)
        .status()
        .unwrap();
    assert!(status.success(), "mkfifo failed");

    let bin = env!("CARGO_BIN_EXE_pumice-transcribe");
    let mut child = std::process::Command::new(bin)
        .args([
            "file",
            fifo_path.to_str().unwrap(),
            "--model",
            "/dev/null",
            "--json",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    let timeout = std::time::Duration::from_secs(5);
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            panic!("pumice-transcribe blocked on FIFO without a writer");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };

    let mut output = std::process::Output {
        status,
        stdout: Vec::new(),
        stderr: Vec::new(),
    };
    if let Some(mut stdout) = child.stdout.take() {
        use std::io::Read;
        stdout.read_to_end(&mut output.stdout).unwrap();
    }
    if let Some(mut stderr) = child.stderr.take() {
        use std::io::Read;
        stderr.read_to_end(&mut output.stderr).unwrap();
    }
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"]["code"], "invalid_input");
}

#[test]
fn too_large_file_returns_input_too_large() {
    let tmp = tempfile::tempdir().unwrap();
    let audio_path = tmp.path().join("audio.wav");
    std::fs::write(&audio_path, vec![0u8; 25_000_001]).unwrap();

    let err = run_with_engine(
        vec![
            "pumice-transcribe",
            "file",
            audio_path.to_str().unwrap(),
            "--model",
            "/dev/null",
        ],
        Box::new(FakeEngine::new("x", None::<String>)),
    )
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::InputTooLarge);
}

#[test]
fn too_long_audio_returns_audio_too_long() {
    let tmp = tempfile::tempdir().unwrap();
    let audio_path = tmp.path().join("audio.wav");
    std::fs::write(&audio_path, write_silence_wav(121.0, 16_000, 1)).unwrap();

    let err = run_with_engine(
        vec![
            "pumice-transcribe",
            "file",
            audio_path.to_str().unwrap(),
            "--model",
            "/dev/null",
        ],
        Box::new(FakeEngine::new("x", None::<String>)),
    )
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::AudioTooLong);
}

#[test]
fn long_8khz_8bit_under_size_over_duration_rejected_before_resample() {
    // 121 seconds of 8 kHz 8-bit mono = 968,000 bytes, well under 25 MB.
    // The decoder must reject it before any resampled buffer is allocated.
    let tmp = tempfile::tempdir().unwrap();
    let audio_path = tmp.path().join("audio.wav");
    let silence = vec![0i8; 8_000 * 121];
    std::fs::write(&audio_path, write_wav_i8(&silence, 8_000, 1)).unwrap();

    let err = run_with_engine(
        vec![
            "pumice-transcribe",
            "file",
            audio_path.to_str().unwrap(),
            "--model",
            "/dev/null",
        ],
        Box::new(FakeEngine::new("x", None::<String>)),
    )
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::AudioTooLong);
}

#[test]
fn missing_model_flag_returns_invalid_arguments() {
    let err = run_with_engine(
        vec!["pumice-transcribe", "file", "audio.wav"],
        Box::new(FakeEngine::new("x", None::<String>)),
    )
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::InvalidArguments);
}

#[test]
fn json_success_envelope_shape() {
    let output = SuccessOutput {
        text: "hello".to_string(),
        language: Some("pt".to_string()),
        audio_seconds: 1.5,
        model_load_ms: 100,
        transcription_ms: 200,
    };
    let mut buf = Vec::new();
    print_success(&output, true, &mut buf).unwrap();
    let s = String::from_utf8(buf).unwrap();
    let json: serde_json::Value = serde_json::from_str(s.trim()).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["ok"], true);
    assert_eq!(json["result"]["text"], "hello");
    assert_eq!(json["result"]["language"], "pt");
    assert_eq!(json["result"]["audio_seconds"], 1.5);
    assert_eq!(json["result"]["model_load_ms"], 100);
    assert_eq!(json["result"]["transcription_ms"], 200);
    assert!(json["error"].is_null());
}

#[test]
fn json_error_envelope_shape() {
    let error = pumice_transcription::Error::model_not_found(std::path::Path::new("/missing"));
    let mut out = Vec::new();
    let mut err = Vec::new();
    print_error(&error, true, &mut out, &mut err).unwrap();
    let s = String::from_utf8(out).unwrap();
    let json: serde_json::Value = serde_json::from_str(s.trim()).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["ok"], false);
    assert!(json["result"].is_null());
    assert_eq!(json["error"]["code"], "model_not_found");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("/missing")
    );
    assert!(err.is_empty());
}

#[test]
fn plain_error_goes_to_stderr() {
    let error = pumice_transcription::Error::engine_failed();
    let mut out = Vec::new();
    let mut err = Vec::new();
    print_error(&error, false, &mut out, &mut err).unwrap();
    assert!(out.is_empty());
    let s = String::from_utf8(err).unwrap();
    assert!(s.contains("transcription engine failed"));
}

#[test]
fn binary_help_and_version() {
    let bin = env!("CARGO_BIN_EXE_pumice-transcribe");
    let help = Command::new(bin).arg("--help").output().unwrap();
    assert!(
        help.status.success(),
        "--help failed: stdout={:?}, stderr={:?}",
        String::from_utf8_lossy(&help.stdout),
        String::from_utf8_lossy(&help.stderr)
    );
    let stdout = String::from_utf8(help.stdout).unwrap();
    assert!(stdout.contains("pumice-transcribe"));
    assert!(stdout.contains("file"));

    let file_help = Command::new(bin).args(["file", "--help"]).output().unwrap();
    assert!(
        file_help.status.success(),
        "file --help failed: stdout={:?}, stderr={:?}",
        String::from_utf8_lossy(&file_help.stdout),
        String::from_utf8_lossy(&file_help.stderr)
    );
    let file_stdout = String::from_utf8(file_help.stdout).unwrap();
    assert!(file_stdout.contains("--model"));

    let version = Command::new(bin).arg("--version").output().unwrap();
    assert!(
        version.status.success(),
        "--version failed: stdout={:?}, stderr={:?}",
        String::from_utf8_lossy(&version.stdout),
        String::from_utf8_lossy(&version.stderr)
    );
    let stdout = String::from_utf8(version.stdout).unwrap();
    assert!(stdout.contains("pumice-transcribe"));
}

#[test]
fn language_validation_accepts_pt() {
    let tmp = tempfile::tempdir().unwrap();
    let audio_path = tmp.path().join("audio.wav");
    std::fs::write(&audio_path, write_silence_wav(1.0, 16_000, 1)).unwrap();
    let model_path = tmp.path().join("model.gguf");
    std::fs::write(&model_path, b"").unwrap();

    let engine = RecordingFakeEngine::new();
    run_with_engine(
        vec![
            "pumice-transcribe",
            "file",
            audio_path.to_str().unwrap(),
            "--model",
            model_path.to_str().unwrap(),
            "--language",
            "pt",
        ],
        Box::new(engine.clone()),
    )
    .unwrap();

    assert_eq!(engine.language(), Some("pt".to_string()));
}

#[test]
fn language_validation_normalizes_pt_uppercase() {
    let tmp = tempfile::tempdir().unwrap();
    let audio_path = tmp.path().join("audio.wav");
    std::fs::write(&audio_path, write_silence_wav(1.0, 16_000, 1)).unwrap();
    let model_path = tmp.path().join("model.gguf");
    std::fs::write(&model_path, b"").unwrap();

    let engine = RecordingFakeEngine::new();
    run_with_engine(
        vec![
            "pumice-transcribe",
            "file",
            audio_path.to_str().unwrap(),
            "--model",
            model_path.to_str().unwrap(),
            "--language",
            "PT",
        ],
        Box::new(engine.clone()),
    )
    .unwrap();

    assert_eq!(engine.language(), Some("pt".to_string()));
}

#[test]
fn language_validation_rejects_portuguese() {
    let tmp = tempfile::tempdir().unwrap();
    let audio_path = tmp.path().join("audio.wav");
    std::fs::write(&audio_path, write_silence_wav(1.0, 16_000, 1)).unwrap();
    let model_path = tmp.path().join("model.gguf");
    std::fs::write(&model_path, b"").unwrap();

    let err = run_with_engine(
        vec![
            "pumice-transcribe",
            "file",
            audio_path.to_str().unwrap(),
            "--model",
            model_path.to_str().unwrap(),
            "--language",
            "portuguese",
        ],
        Box::new(FakeEngine::new("x", None::<String>)),
    )
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::InvalidArguments);
}

#[test]
fn language_validation_rejects_zz() {
    let tmp = tempfile::tempdir().unwrap();
    let audio_path = tmp.path().join("audio.wav");
    std::fs::write(&audio_path, write_silence_wav(1.0, 16_000, 1)).unwrap();
    let model_path = tmp.path().join("model.gguf");
    std::fs::write(&model_path, b"").unwrap();

    let err = run_with_engine(
        vec![
            "pumice-transcribe",
            "file",
            audio_path.to_str().unwrap(),
            "--model",
            model_path.to_str().unwrap(),
            "--language",
            "zz",
        ],
        Box::new(FakeEngine::new("x", None::<String>)),
    )
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::InvalidArguments);
}

#[test]
fn threads_validation_rejects_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let audio_path = tmp.path().join("audio.wav");
    std::fs::write(&audio_path, write_silence_wav(1.0, 16_000, 1)).unwrap();
    let model_path = tmp.path().join("model.gguf");
    std::fs::write(&model_path, b"").unwrap();

    let err = run_with_engine(
        vec![
            "pumice-transcribe",
            "file",
            audio_path.to_str().unwrap(),
            "--model",
            model_path.to_str().unwrap(),
            "--threads",
            "0",
        ],
        Box::new(FakeEngine::new("x", None::<String>)),
    )
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::InvalidArguments);
}

#[test]
fn explicit_language_overrides_engine_none() {
    // Regression: when --language is given and the engine returns no
    // detected language, the envelope must still report the explicit hint.
    let tmp = tempfile::tempdir().unwrap();
    let audio_path = tmp.path().join("audio.wav");
    std::fs::write(&audio_path, write_silence_wav(1.0, 16_000, 1)).unwrap();
    let model_path = tmp.path().join("model.gguf");
    std::fs::write(&model_path, b"").unwrap();

    let result = run_with_engine(
        vec![
            "pumice-transcribe",
            "file",
            audio_path.to_str().unwrap(),
            "--model",
            model_path.to_str().unwrap(),
            "--language",
            "pt",
        ],
        Box::new(NoLanguageFakeEngine),
    )
    .unwrap();

    assert_eq!(result.language, Some("pt".to_string()));
}

#[test]
fn threads_validation_rejects_too_large() {
    let tmp = tempfile::tempdir().unwrap();
    let audio_path = tmp.path().join("audio.wav");
    std::fs::write(&audio_path, write_silence_wav(1.0, 16_000, 1)).unwrap();
    let model_path = tmp.path().join("model.gguf");
    std::fs::write(&model_path, b"").unwrap();

    let err = run_with_engine(
        vec![
            "pumice-transcribe",
            "file",
            audio_path.to_str().unwrap(),
            "--model",
            model_path.to_str().unwrap(),
            "--threads",
            "33",
        ],
        Box::new(FakeEngine::new("x", None::<String>)),
    )
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::InvalidArguments);
}

#[test]
fn json_parse_error_missing_model_with_json() {
    let bin = env!("CARGO_BIN_EXE_pumice-transcribe");
    let out = Command::new(bin)
        .args(["file", "audio.wav", "--json"])
        .output()
        .unwrap();

    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(2));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"]["code"], "invalid_arguments");
    assert!(
        out.stderr.is_empty(),
        "stderr should be empty: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn json_parse_error_bad_threads_with_json() {
    let bin = env!("CARGO_BIN_EXE_pumice-transcribe");
    let out = Command::new(bin)
        .args(["file", "audio.wav", "--threads", "abc", "--json"])
        .output()
        .unwrap();

    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(2));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"]["code"], "invalid_arguments");
    assert!(out.stderr.is_empty());
}

#[test]
fn json_parse_error_unknown_flag_with_json() {
    let bin = env!("CARGO_BIN_EXE_pumice-transcribe");
    let out = Command::new(bin)
        .args([
            "file",
            "audio.wav",
            "--model",
            "/dev/null",
            "--unknown-flag",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(2));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"]["code"], "invalid_arguments");
    assert!(out.stderr.is_empty());
}

#[test]
fn parse_error_unknown_flag_without_json_goes_to_stderr() {
    let bin = env!("CARGO_BIN_EXE_pumice-transcribe");
    let out = Command::new(bin)
        .args([
            "file",
            "audio.wav",
            "--model",
            "/dev/null",
            "--unknown-flag",
        ])
        .output()
        .unwrap();

    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(2));
    assert!(
        out.stdout.is_empty(),
        "stdout should be empty for human errors"
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("--unknown-flag"));
}

#[test]
#[cfg(target_os = "linux")]
fn output_failure_on_dev_full_reports_exit_1() {
    let bin = env!("CARGO_BIN_EXE_pumice-transcribe");
    let out = Command::new(bin)
        .args(["file", "audio.wav", "--model", "/dev/null", "--json"])
        .stdout(std::process::Stdio::from(
            std::fs::OpenOptions::new()
                .write(true)
                .open("/dev/full")
                .unwrap(),
        ))
        .output()
        .unwrap();

    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("failed to write output"));
}

#[test]
#[cfg(target_os = "linux")]
fn help_output_failure_on_dev_full_reports_exit_1() {
    let bin = env!("CARGO_BIN_EXE_pumice-transcribe");
    let out = Command::new(bin)
        .arg("--help")
        .stdout(std::process::Stdio::from(
            std::fs::OpenOptions::new()
                .write(true)
                .open("/dev/full")
                .unwrap(),
        ))
        .output()
        .unwrap();

    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("failed to write output"));
}

#[test]
#[cfg(target_os = "linux")]
fn version_output_failure_on_dev_full_reports_exit_1() {
    let bin = env!("CARGO_BIN_EXE_pumice-transcribe");
    let out = Command::new(bin)
        .arg("--version")
        .stdout(std::process::Stdio::from(
            std::fs::OpenOptions::new()
                .write(true)
                .open("/dev/full")
                .unwrap(),
        ))
        .output()
        .unwrap();

    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("failed to write output"));
}
