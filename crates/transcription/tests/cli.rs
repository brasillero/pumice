use pumice_transcription::cli::{
    SuccessOutput, print_error, print_success, run_with_engine as run_with_engine_raw,
};
use pumice_transcription::compute::{BackendMode, Compute, ComputeBackend, FallbackReason};
use pumice_transcription::{
    CancelToken, Engine, EngineError, ErrorCode, FakeEngine, Transcript, TranscriptionOptions,
    config,
};
use std::io::Cursor;
use std::process::Command;

/// Every top-level `transcribe.yml` setting, keyed by its name, with the
/// value that was the built-in default before every setting had to be
/// written. [`complete`] appends the ones a test does not write.
const SETTINGS: [(&str, &str); 8] = [
    ("schema_version", "schema_version: 1\n"),
    ("default_model", "default_model: null\n"),
    ("language", "language: null\n"),
    ("threads", "threads: auto\n"),
    ("backend", "backend: cpu\n"),
    ("server", "server:\n  port: null\n"),
    ("models_dir", "models_dir: null\n"),
    ("models", "models: []\n"),
];

/// `text`, then every top-level setting it does not write itself.
/// Appending keeps the line numbers in `text` unchanged.
fn complete(text: &str) -> String {
    let mut yaml = text.to_owned();
    for (key, line) in SETTINGS {
        let prefix = format!("{key}:");
        if !text.lines().any(|written| written.starts_with(&prefix)) {
            yaml.push_str(line);
        }
    }
    yaml
}

/// Writes `text`, completed by [`complete`], to `path`.
fn write_complete(path: impl AsRef<std::path::Path>, text: &str) -> std::io::Result<()> {
    std::fs::write(path, complete(text))
}

fn isolated_command(bin: &str) -> Command {
    let mut command = Command::new(bin);
    command
        .env_remove("HOME")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("APPDATA");
    command
}

/// [`isolated_command`] with `--config` naming a complete configuration
/// written into `dir`, for runs whose subject is not the configuration.
fn configured_command(bin: &str, dir: &std::path::Path) -> Command {
    let config = dir.join("transcribe.yml");
    write_complete(&config, "").unwrap();
    let mut command = isolated_command(bin);
    command.arg("--config").arg(config);
    command
}

fn run_with_engine(
    args: Vec<&str>,
    engine: Box<dyn Engine>,
) -> Result<SuccessOutput, pumice_transcription::Error> {
    let mut args: Vec<std::ffi::OsString> = args.into_iter().map(Into::into).collect();
    let temp = tempfile::tempdir().unwrap();
    if !args.iter().any(|arg| arg == "--config") {
        let config = temp.path().join("transcribe.yml");
        write_complete(&config, "").unwrap();
        args.insert(1, "--config".into());
        args.insert(2, config.into());
    }
    run_with_engine_raw(args, engine)
}

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

fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures")).join(name)
}

fn write_silence_wav(duration_secs: f64, sample_rate: u32, channels: u16) -> Vec<u8> {
    let samples = vec![0.0f32; (duration_secs * sample_rate as f64) as usize * channels as usize];
    write_wav_f32(&samples, sample_rate, channels)
}

/// Build a valid PCM WAV of exactly `total_size` bytes with a short silent
/// audio data chunk and a trailing JUNK chunk that pads the file to the
/// requested size. The JUNK size is chosen to be even so no pad byte is needed.
fn write_wav_with_trailing_junk(total_size: usize, audio_bytes: usize) -> Vec<u8> {
    assert!(audio_bytes.is_multiple_of(2), "audio_bytes must be even");
    let junk_size = total_size - 52 - audio_bytes;
    assert!(
        junk_size.is_multiple_of(2),
        "computed junk_size must be even"
    );

    let sample_rate = 16_000u32;
    let channels = 1u16;
    let bits_per_sample = 16u16;
    let block_align = channels * (bits_per_sample / 8);
    let byte_rate = sample_rate * block_align as u32;

    let mut out = Vec::with_capacity(total_size);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((total_size - 8) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");

    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // WAVE_FORMAT_PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits_per_sample.to_le_bytes());

    out.extend_from_slice(b"data");
    out.extend_from_slice(&(audio_bytes as u32).to_le_bytes());
    out.extend_from_slice(&vec![0u8; audio_bytes]);

    out.extend_from_slice(b"JUNK");
    out.extend_from_slice(&(junk_size as u32).to_le_bytes());
    out.extend_from_slice(&vec![0u8; junk_size]);

    assert_eq!(out.len(), total_size);
    out
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
        _cancel: &CancelToken,
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
        _cancel: &CancelToken,
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
        _cancel: &CancelToken,
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
            "--model-file",
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
            "--model-file",
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
            "--model-file",
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
            "--model-file",
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
            "--model-file",
            "/dev/null",
        ],
        Box::new(FakeEngine::new("x", None::<String>)),
    )
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::InvalidInput);
}

#[test]
fn non_audio_file_returns_unsupported_format() {
    let tmp = tempfile::tempdir().unwrap();
    let audio_path = tmp.path().join("audio.txt");
    std::fs::write(&audio_path, b"not a wav file").unwrap();

    let err = run_with_engine(
        vec![
            "pumice-transcribe",
            "file",
            audio_path.to_str().unwrap(),
            "--model-file",
            "/dev/null",
        ],
        Box::new(FakeEngine::new("x", None::<String>)),
    )
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::UnsupportedFormat);
}

/// Runs `file` on `audio` with an existing (empty) model file and returns
/// the error.
fn file_error(audio: &std::path::Path) -> pumice_transcription::Error {
    let tmp = tempfile::tempdir().unwrap();
    let model_path = tmp.path().join("model.gguf");
    std::fs::write(&model_path, b"").unwrap();
    run_with_engine(
        vec![
            "pumice-transcribe",
            "file",
            audio.to_str().unwrap(),
            "--model-file",
            model_path.to_str().unwrap(),
        ],
        Box::new(FakeEngine::new("x", None::<String>)),
    )
    .unwrap_err()
}

#[test]
fn non_regular_file_returns_invalid_input() {
    // A directory exists on every platform and is not a regular file.
    let tmp = tempfile::tempdir().unwrap();
    let err = file_error(tmp.path());

    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert!(
        err.message.contains("not a regular file"),
        "refused as a non-regular file: {}",
        err.message
    );
}

#[test]
#[cfg(unix)]
fn device_file_returns_invalid_input() {
    // /dev/null is a character device, not a regular file.
    let err = file_error(std::path::Path::new("/dev/null"));

    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert!(
        err.message.contains("not a regular file"),
        "refused as a non-regular file: {}",
        err.message
    );
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
    let mut child = configured_command(bin, tmp.path())
        .args([
            "file",
            fifo_path.to_str().unwrap(),
            "--model-file",
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
            "--model-file",
            "/dev/null",
        ],
        Box::new(FakeEngine::new("x", None::<String>)),
    )
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::InputTooLarge);
}

#[test]
fn exact_size_limit_file_is_accepted() {
    // A valid 1-second 16 kHz 16-bit mono WAV padded with a trailing JUNK
    // chunk to exactly 25 MB must pass the byte-budget check and decode.
    let tmp = tempfile::tempdir().unwrap();
    let audio_path = tmp.path().join("audio.wav");
    std::fs::write(
        &audio_path,
        write_wav_with_trailing_junk(25_000_000, 32_000),
    )
    .unwrap();
    let model_path = tmp.path().join("model.gguf");
    std::fs::write(&model_path, b"").unwrap();

    let result = run_with_engine(
        vec![
            "pumice-transcribe",
            "file",
            audio_path.to_str().unwrap(),
            "--model-file",
            model_path.to_str().unwrap(),
        ],
        Box::new(FakeEngine::new("x", None::<String>)),
    )
    .unwrap();

    assert!((result.audio_seconds - 1.0).abs() < 0.01);
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
            "--model-file",
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
            "--model-file",
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
        compute: Some(Compute::new(
            BackendMode::Auto,
            ComputeBackend::Cpu,
            "CPU",
            "Test CPU  ",
            Some("haswell"),
            Some(FallbackReason::GpuUnavailable),
        )),
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
    assert_eq!(
        json["result"]["compute"],
        serde_json::json!({
            "requested": "auto",
            "backend": "cpu",
            "device": {"name": "CPU", "description": "Test CPU"},
            "cpu_variant": "haswell",
            "fallback_reason": "gpu_unavailable",
        })
    );
    assert!(json["result"].get("backend").is_none());
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
    let help = isolated_command(bin).arg("--help").output().unwrap();
    assert!(
        help.status.success(),
        "--help failed: stdout={:?}, stderr={:?}",
        String::from_utf8_lossy(&help.stdout),
        String::from_utf8_lossy(&help.stderr)
    );
    let stdout = String::from_utf8(help.stdout).unwrap();
    assert!(stdout.contains("pumice-transcribe"));
    assert!(stdout.contains("file"));

    let file_help = isolated_command(bin)
        .args(["file", "--help"])
        .output()
        .unwrap();
    assert!(
        file_help.status.success(),
        "file --help failed: stdout={:?}, stderr={:?}",
        String::from_utf8_lossy(&file_help.stdout),
        String::from_utf8_lossy(&file_help.stderr)
    );
    let file_stdout = String::from_utf8(file_help.stdout).unwrap();
    assert!(file_stdout.contains("--model"));

    let version = isolated_command(bin).arg("--version").output().unwrap();
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
            "--model-file",
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
            "--model-file",
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
            "--model-file",
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
            "--model-file",
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
            "--model-file",
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
            "--model-file",
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
            "--model-file",
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
    let tmp = tempfile::tempdir().unwrap();
    let bin = env!("CARGO_BIN_EXE_pumice-transcribe");
    let out = configured_command(bin, tmp.path())
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
    let out = isolated_command(bin)
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
    let out = isolated_command(bin)
        .args([
            "file",
            "audio.wav",
            "--model-file",
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
    let out = isolated_command(bin)
        .args([
            "file",
            "audio.wav",
            "--model-file",
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
    let out = isolated_command(bin)
        .args(["file", "audio.wav", "--model-file", "/dev/null", "--json"])
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
    let out = isolated_command(bin)
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
    let out = isolated_command(bin)
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

#[test]
fn each_compressed_format_decodes_to_plausible_duration() {
    let tmp = tempfile::tempdir().unwrap();
    let model_path = tmp.path().join("model.gguf");
    std::fs::write(&model_path, b"").unwrap();

    for (name, expected) in [
        ("sample-pt.mp3", 14.19),
        ("sample-pt.flac", 14.19),
        ("sample-pt.ogg", 14.19),
    ] {
        let engine = RecordingFakeEngine::new();
        let result = run_with_engine(
            vec![
                "pumice-transcribe",
                "file",
                fixture_path(name).to_str().unwrap(),
                "--model-file",
                model_path.to_str().unwrap(),
            ],
            Box::new(engine.clone()),
        )
        .unwrap();

        assert!(
            (result.audio_seconds - expected).abs() < 1.5,
            "{name}: audio_seconds {} too far from {expected}",
            result.audio_seconds
        );
        let expected_samples = (result.audio_seconds * 16_000.0) as usize;
        assert!(
            (engine.pcm_len() as isize - expected_samples as isize).abs() <= 2,
            "{name}: resampled length {} too far from {expected_samples}",
            engine.pcm_len()
        );
    }
}

#[test]
fn detection_ignores_file_extension() {
    let tmp = tempfile::tempdir().unwrap();
    let model_path = tmp.path().join("model.gguf");
    std::fs::write(&model_path, b"").unwrap();

    // MP3 bytes stored with a .wav extension.
    let mp3_as_wav = tmp.path().join("mp3-as-wav.wav");
    std::fs::copy(fixture_path("sample-pt.mp3"), &mp3_as_wav).unwrap();

    // WAV bytes stored with a .mp3 extension.
    let wav_as_mp3 = tmp.path().join("wav-as-mp3.mp3");
    std::fs::copy(fixture_path("sample-pt.wav"), &wav_as_mp3).unwrap();

    for path in [&mp3_as_wav, &wav_as_mp3] {
        let engine = RecordingFakeEngine::new();
        let result = run_with_engine(
            vec![
                "pumice-transcribe",
                "file",
                path.to_str().unwrap(),
                "--model-file",
                model_path.to_str().unwrap(),
            ],
            Box::new(engine),
        )
        .unwrap();
        assert!((result.audio_seconds - 14.19).abs() < 1.5);
    }
}

#[test]
fn truncated_compressed_files_return_typed_errors() {
    let model_tmp = tempfile::tempdir().unwrap();
    let model_path = model_tmp.path().join("model.gguf");
    std::fs::write(&model_path, b"").unwrap();

    for name in ["sample-pt.mp3", "sample-pt.flac", "sample-pt.ogg"] {
        let tmp = tempfile::tempdir().unwrap();
        let bytes = std::fs::read(fixture_path(name)).unwrap();
        let truncated = &bytes[..64.min(bytes.len())];
        let path = tmp.path().join(name);
        std::fs::write(&path, truncated).unwrap();

        let err = run_with_engine(
            vec![
                "pumice-transcribe",
                "file",
                path.to_str().unwrap(),
                "--model-file",
                model_path.to_str().unwrap(),
            ],
            Box::new(FakeEngine::new("x", None::<String>)),
        )
        .unwrap_err();

        assert!(
            matches!(
                err.code,
                ErrorCode::InvalidInput | ErrorCode::UnsupportedFormat
            ),
            "{name}: expected typed error, got {:?}",
            err.code
        );
    }
}

#[test]
fn empty_audio_file_returns_unsupported_format() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("empty.wav");
    std::fs::write(&path, b"").unwrap();

    let err = run_with_engine(
        vec![
            "pumice-transcribe",
            "file",
            path.to_str().unwrap(),
            "--model-file",
            "/dev/null",
        ],
        Box::new(FakeEngine::new("x", None::<String>)),
    )
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::UnsupportedFormat);
}

#[cfg(not(feature = "opus"))]
#[test]
fn ogg_opus_returns_unsupported_format() {
    let err = run_with_engine(
        vec![
            "pumice-transcribe",
            "file",
            fixture_path("sample-pt.opus").to_str().unwrap(),
            "--model-file",
            "/dev/null",
        ],
        Box::new(FakeEngine::new("x", None::<String>)),
    )
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::UnsupportedFormat);
}

#[test]
fn chained_ogg_returns_unsupported_format_json() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("chained.ogg");
    let mut bytes = std::fs::read(fixture_path("sample-pt.ogg")).unwrap();
    bytes.extend(std::fs::read(fixture_path("sample-pt.ogg")).unwrap());
    std::fs::write(&path, bytes).unwrap();

    let out = configured_command(env!("CARGO_BIN_EXE_pumice-transcribe"), tmp.path())
        .args([
            "file",
            path.to_str().unwrap(),
            "--model-file",
            "/dev/null",
            "--json",
        ])
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(2));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["error"]["code"], "unsupported_format");
}

#[test]
fn long_compressed_audio_returns_audio_too_long() {
    let err = run_with_engine(
        vec![
            "pumice-transcribe",
            "file",
            fixture_path("long-silence.ogg").to_str().unwrap(),
            "--model-file",
            "/dev/null",
        ],
        Box::new(FakeEngine::new("x", None::<String>)),
    )
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::AudioTooLong);
}

#[cfg(not(feature = "opus"))]
#[test]
fn json_error_envelope_for_unsupported_format() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = env!("CARGO_BIN_EXE_pumice-transcribe");
    let out = configured_command(bin, tmp.path())
        .args([
            "file",
            fixture_path("sample-pt.opus").to_str().unwrap(),
            "--model-file",
            "/dev/null",
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
    assert_eq!(json["error"]["code"], "unsupported_format");
}

#[test]
fn json_error_envelope_for_audio_too_long() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = env!("CARGO_BIN_EXE_pumice-transcribe");
    let out = configured_command(bin, tmp.path())
        .args([
            "file",
            fixture_path("long-silence.mp3").to_str().unwrap(),
            "--model-file",
            "/dev/null",
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
    assert_eq!(json["error"]["code"], "audio_too_long");
}

/// Writes `text`, completed by [`complete`], as `transcribe.yml`.
fn write_config(tmp: &tempfile::TempDir, text: &str) -> std::path::PathBuf {
    let path = tmp.path().join("transcribe.yml");
    write_complete(&path, text).unwrap();
    path
}

#[test]
fn model_id_resolves_from_config() {
    let tmp = tempfile::tempdir().unwrap();
    let audio_path = tmp.path().join("audio.wav");
    std::fs::write(&audio_path, write_silence_wav(1.0, 16_000, 1)).unwrap();
    let model_path = tmp.path().join("tiny.gguf");
    std::fs::write(&model_path, b"").unwrap();
    let config_path = write_config(
        &tmp,
        &format!(
            r#"schema_version: 1
models:
  - id: tiny
    path: {}
"#,
            model_path.display()
        ),
    );

    run_with_engine(
        vec![
            "pumice-transcribe",
            "--config",
            config_path.to_str().unwrap(),
            "file",
            audio_path.to_str().unwrap(),
            "--model",
            "tiny",
        ],
        Box::new(FakeEngine::new("hello", None::<String>)),
    )
    .unwrap();
}

#[test]
fn model_file_flag_needs_no_configured_model() {
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
            "--model-file",
            model_path.to_str().unwrap(),
        ],
        Box::new(FakeEngine::new("hello", None::<String>)),
    )
    .unwrap();

    assert_eq!(result.text, "hello");
}

#[test]
fn model_and_model_file_are_mutually_exclusive() {
    let tmp = tempfile::tempdir().unwrap();
    let audio_path = tmp.path().join("audio.wav");
    std::fs::write(&audio_path, write_silence_wav(1.0, 16_000, 1)).unwrap();
    let model_path = tmp.path().join("model.gguf");
    std::fs::write(&model_path, b"").unwrap();
    let config_path = write_config(
        &tmp,
        r#"schema_version: 1
models:
  - id: tiny
    path: model.gguf
"#,
    );

    let err = run_with_engine(
        vec![
            "pumice-transcribe",
            "--config",
            config_path.to_str().unwrap(),
            "file",
            audio_path.to_str().unwrap(),
            "--model",
            "tiny",
            "--model-file",
            model_path.to_str().unwrap(),
        ],
        Box::new(FakeEngine::new("x", None::<String>)),
    )
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::InvalidArguments);
}

#[test]
fn default_model_is_used_when_no_flag_is_given() {
    let tmp = tempfile::tempdir().unwrap();
    let audio_path = tmp.path().join("audio.wav");
    std::fs::write(&audio_path, write_silence_wav(1.0, 16_000, 1)).unwrap();
    let model_path = tmp.path().join("tiny.gguf");
    std::fs::write(&model_path, b"").unwrap();
    let config_path = write_config(
        &tmp,
        &format!(
            r#"schema_version: 1
default_model: tiny
models:
  - id: tiny
    path: {}
"#,
            model_path.display()
        ),
    );

    let result = run_with_engine(
        vec![
            "pumice-transcribe",
            "--config",
            config_path.to_str().unwrap(),
            "file",
            audio_path.to_str().unwrap(),
        ],
        Box::new(FakeEngine::new("from default", None::<String>)),
    )
    .unwrap();

    assert_eq!(result.text, "from default");
}

#[test]
fn no_model_flag_and_no_default_model_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let audio_path = tmp.path().join("audio.wav");
    std::fs::write(&audio_path, write_silence_wav(1.0, 16_000, 1)).unwrap();
    let config_path = write_config(&tmp, "schema_version: 1\nmodels: []\n");

    let err = run_with_engine(
        vec![
            "pumice-transcribe",
            "--config",
            config_path.to_str().unwrap(),
            "file",
            audio_path.to_str().unwrap(),
        ],
        Box::new(FakeEngine::new("x", None::<String>)),
    )
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::InvalidArguments);
}

#[test]
fn unknown_model_id_lists_configured_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let audio_path = tmp.path().join("audio.wav");
    std::fs::write(&audio_path, write_silence_wav(1.0, 16_000, 1)).unwrap();
    let model_path = tmp.path().join("tiny.gguf");
    std::fs::write(&model_path, b"").unwrap();
    let config_path = write_config(
        &tmp,
        &format!(
            r#"schema_version: 1
models:
  - id: tiny
    path: {}
"#,
            model_path.display()
        ),
    );

    let err = run_with_engine(
        vec![
            "pumice-transcribe",
            "--config",
            config_path.to_str().unwrap(),
            "file",
            audio_path.to_str().unwrap(),
            "--model",
            "huge",
        ],
        Box::new(FakeEngine::new("x", None::<String>)),
    )
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::InvalidArguments);
    assert!(err.to_string().contains("huge"));
    assert!(err.to_string().contains("tiny"));
}

#[test]
fn config_language_applies_without_cli_language() {
    let tmp = tempfile::tempdir().unwrap();
    let audio_path = tmp.path().join("audio.wav");
    std::fs::write(&audio_path, write_silence_wav(1.0, 16_000, 1)).unwrap();
    let model_path = tmp.path().join("tiny.gguf");
    std::fs::write(&model_path, b"").unwrap();
    let config_path = write_config(
        &tmp,
        &format!(
            r#"schema_version: 1
language: pt
models:
  - id: tiny
    path: {}
"#,
            model_path.display()
        ),
    );

    let engine = RecordingFakeEngine::new();
    run_with_engine(
        vec![
            "pumice-transcribe",
            "--config",
            config_path.to_str().unwrap(),
            "file",
            audio_path.to_str().unwrap(),
            "--model",
            "tiny",
        ],
        Box::new(engine.clone()),
    )
    .unwrap();

    assert_eq!(engine.language(), Some("pt".to_string()));
}

#[test]
fn cli_language_overrides_config() {
    let tmp = tempfile::tempdir().unwrap();
    let audio_path = tmp.path().join("audio.wav");
    std::fs::write(&audio_path, write_silence_wav(1.0, 16_000, 1)).unwrap();
    let model_path = tmp.path().join("tiny.gguf");
    std::fs::write(&model_path, b"").unwrap();
    let config_path = write_config(
        &tmp,
        &format!(
            r#"schema_version: 1
language: pt
models:
  - id: tiny
    path: {}
"#,
            model_path.display()
        ),
    );

    let engine = RecordingFakeEngine::new();
    run_with_engine(
        vec![
            "pumice-transcribe",
            "--config",
            config_path.to_str().unwrap(),
            "file",
            audio_path.to_str().unwrap(),
            "--model",
            "tiny",
            "--language",
            "en",
        ],
        Box::new(engine.clone()),
    )
    .unwrap();

    assert_eq!(engine.language(), Some("en".to_string()));
}

#[test]
fn cli_threads_override_config() {
    let tmp = tempfile::tempdir().unwrap();
    let audio_path = tmp.path().join("audio.wav");
    std::fs::write(&audio_path, write_silence_wav(1.0, 16_000, 1)).unwrap();
    let model_path = tmp.path().join("tiny.gguf");
    std::fs::write(&model_path, b"").unwrap();
    let config_path = write_config(
        &tmp,
        &format!(
            r#"schema_version: 1
threads: 8
models:
  - id: tiny
    path: {}
"#,
            model_path.display()
        ),
    );

    // A config-valid thread count combined with an out-of-bounds CLI value
    // proves the CLI value wins (and is validated).
    let err = run_with_engine(
        vec![
            "pumice-transcribe",
            "--config",
            config_path.to_str().unwrap(),
            "file",
            audio_path.to_str().unwrap(),
            "--model",
            "tiny",
            "--threads",
            "33",
        ],
        Box::new(FakeEngine::new("x", None::<String>)),
    )
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::InvalidArguments);
}

#[test]
fn check_config_plain_success() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = write_config(
        &tmp,
        r#"schema_version: 1
models:
  - id: tiny
    path: /tmp/tiny.gguf
"#,
    );
    let bin = env!("CARGO_BIN_EXE_pumice-transcribe");
    let out = isolated_command(bin)
        .args(["check-config", "--config", config_path.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("config ok"));
    assert!(stdout.contains(config_path.to_str().unwrap()));
}

/// Runs the binary with `args` and no configuration file anywhere: an
/// empty temporary home, and models stored under it.
fn run_without_config(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    isolated_command(env!("CARGO_BIN_EXE_pumice-transcribe"))
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("APPDATA", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("LOCALAPPDATA", home.join("data"))
        .output()
        .unwrap()
}

#[test]
fn commands_that_read_settings_fail_without_a_config_file() {
    let tmp = tempfile::tempdir().unwrap();
    let wav = tmp.path().join("clip.wav");
    std::fs::write(&wav, write_silence_wav(0.05, 16_000, 1)).unwrap();
    let model = tmp.path().join("model.gguf");
    std::fs::write(&model, b"").unwrap();
    let expected = tmp
        .path()
        .join("config")
        .join("pumice")
        .join("transcribe.yml");
    for args in [
        &["check-config", "--json"][..],
        &["models", "list", "--json"],
        &[
            "file",
            wav.to_str().unwrap(),
            "--model-file",
            model.to_str().unwrap(),
            "--json",
        ],
        &["serve", "--model", "tiny", "--port", "0"],
    ] {
        let out = run_without_config(tmp.path(), args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            text.contains("the transcription configuration file does not exist"),
            "{args:?}: {text}"
        );
    }
    let out = run_without_config(tmp.path(), &["check-config", "--json"]);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"]["code"], "invalid_config");
    assert_eq!(
        json["error"]["location"]["file"],
        expected.to_str().unwrap()
    );
}

#[test]
fn catalog_and_backends_run_without_a_config_file() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_without_config(tmp.path(), &["models", "catalog", "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let entries = json["result"].as_array().unwrap();
    assert!(!entries.is_empty());
    assert!(entries.iter().all(|entry| entry["installed"] == false));

    // `models verify` looks in the per-user models directory: nothing is
    // installed there, so the failure is about the file, not the config.
    let out = run_without_config(
        tmp.path(),
        &["models", "verify", "whisper-tiny-q8_0", "--json"],
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_ne!(json["error"]["code"], "invalid_config", "{json}");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains(&tmp.path().join("data").display().to_string()),
        "{json}"
    );

    // `backends` needs a mode: from --backend, or from a configuration.
    let out = run_without_config(tmp.path(), &["backends", "--json"]);
    assert_eq!(out.status.code(), Some(2));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no backend mode: pass --backend MODE"),
        "{json}"
    );
    let out = run_without_config(tmp.path(), &["backends", "--backend", "cpu", "--json"]);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_ne!(json["error"]["code"], "invalid_arguments", "{json}");
    assert_ne!(json["error"]["code"], "invalid_config", "{json}");
}

#[test]
fn an_incomplete_config_file_fails_even_for_commands_that_need_none() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("transcribe.yml");
    std::fs::write(&config, "schema_version: 1\nmodels: []\n").unwrap();
    for args in [
        &["models", "catalog", "--json"][..],
        &["backends", "--backend", "cpu", "--json"],
    ] {
        let mut argv = vec!["--config", config.to_str().unwrap()];
        argv.extend(args);
        let out = run_without_config(tmp.path(), &argv);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(json["error"]["code"], "invalid_config", "{args:?}");
        assert!(
            json["error"]["message"]
                .as_str()
                .unwrap()
                .contains("\"default_model\" is missing"),
            "{json}"
        );
    }
}

#[test]
fn check_config_fails_with_location() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = write_config(
        &tmp,
        r#"schema_version: 1
models:
  - id: tiny
    path: tiny.gguf
  - id: tiny
    path: other.gguf
"#,
    );
    let bin = env!("CARGO_BIN_EXE_pumice-transcribe");
    let out = isolated_command(bin)
        .args([
            "check-config",
            "--config",
            config_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();

    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(2));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(json["error"]["code"], "invalid_config");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("duplicate model id")
    );
    assert!(
        json["error"]["location"]["file"]
            .as_str()
            .unwrap()
            .contains("transcribe.yml")
    );
    assert!(json["error"]["location"]["line"].is_number());
}

#[test]
fn check_config_explicit_missing_file_fails() {
    let bin = env!("CARGO_BIN_EXE_pumice-transcribe");
    let out = isolated_command(bin)
        .args(["check-config", "--config", "/tmp/does-not-exist-p4b.yml"])
        .output()
        .unwrap();

    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn models_list_plain_and_json() {
    let tmp = tempfile::tempdir().unwrap();
    let existing_model = tmp.path().join("tiny.gguf");
    std::fs::write(&existing_model, b"tiny").unwrap();
    let missing_model = tmp.path().join("huge.gguf");
    let config_path = write_config(
        &tmp,
        &format!(
            r#"schema_version: 1
default_model: tiny
models:
  - id: tiny
    path: {}
  - id: huge
    path: {}
"#,
            existing_model.display(),
            missing_model.display()
        ),
    );
    let bin = env!("CARGO_BIN_EXE_pumice-transcribe");

    let plain = isolated_command(bin)
        .args(["models", "list", "--config", config_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(plain.status.success());
    let plain_stdout = String::from_utf8(plain.stdout).unwrap();
    assert!(plain_stdout.contains("tiny"));
    assert!(plain_stdout.contains("huge"));
    assert!(plain_stdout.contains("yes"));
    assert!(plain_stdout.contains("no"));

    let json = isolated_command(bin)
        .args([
            "models",
            "list",
            "--config",
            config_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(json.status.success());
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["ok"], true);
    let models = value["result"].as_array().unwrap();
    assert_eq!(models.len(), 2);
    let tiny = models.iter().find(|m| m["id"] == "tiny").unwrap();
    assert_eq!(tiny["exists"], true);
    assert_eq!(tiny["size"], 4);
    assert_eq!(tiny["default"], true);
    let huge = models.iter().find(|m| m["id"] == "huge").unwrap();
    assert_eq!(huge["exists"], false);
    assert!(huge["size"].is_null());
}

#[test]
fn models_list_does_not_report_directory_size() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = write_config(
        &tmp,
        "schema_version: 1\nmodels:\n  - id: directory\n    path: .\n",
    );
    let out = isolated_command(env!("CARGO_BIN_EXE_pumice-transcribe"))
        .args([
            "models",
            "list",
            "--config",
            config_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let entry = &value["result"][0];
    assert_eq!(entry["exists"], false);
    assert!(entry.get("size").is_none());
}

#[test]
fn relative_config_path_resolves_model_against_config_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let config_dir = tmp.path().join("config");
    std::fs::create_dir(&config_dir).unwrap();
    write_complete(
        config_dir.join("transcribe.yml"),
        "schema_version: 1\nmodels:\n  - id: tiny\n    path: models/tiny.gguf\n",
    )
    .unwrap();
    // The model file exists so both sides can be canonicalized: the OS may
    // report the working directory in a different but equivalent form
    // (e.g. /private/var on macOS).
    std::fs::create_dir(config_dir.join("models")).unwrap();
    std::fs::write(config_dir.join("models/tiny.gguf"), b"").unwrap();
    let out = isolated_command(env!("CARGO_BIN_EXE_pumice-transcribe"))
        .current_dir(tmp.path())
        .args([
            "models",
            "list",
            "--config",
            "config/transcribe.yml",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let reported = std::path::PathBuf::from(value["result"][0]["path"].as_str().unwrap());
    assert!(reported.is_absolute());
    assert_eq!(
        reported.canonicalize().unwrap(),
        config_dir.join("models/tiny.gguf").canonicalize().unwrap()
    );
}

#[test]
fn example_config_file_parses() {
    let example =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../transcribe.example.yml");
    let config = config::load(Some(&example)).unwrap().config;
    // The example writes every setting the way it documented them before
    // they had to be written.
    assert_eq!(config.default_model.as_deref(), Some("tiny"));
    assert_eq!(config.language.as_deref(), Some("pt"));
    assert_eq!(config.threads, config::Threads::Count(8));
    assert_eq!(config.backend, BackendMode::Cpu);
    assert_eq!(config.server_port, Some(17568));
    assert_eq!(config.models_dir, None);
    let ids: Vec<&str> = config.models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["turbo", "tiny"]);
}

#[test]
fn per_user_config_lookup_via_env() {
    let tmp = tempfile::tempdir().unwrap();
    // Windows looks under %APPDATA%\pumice; Linux and macOS under $HOME/.config/pumice.
    let base = if cfg!(windows) {
        tmp.path().join("AppData")
    } else {
        tmp.path().join(".config")
    };
    let config_dir = base.join("pumice");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("transcribe.yml");
    write_complete(
        &config_path,
        r#"schema_version: 1
models:
  - id: tiny
    path: tiny.gguf
"#,
    )
    .unwrap();

    let bin = env!("CARGO_BIN_EXE_pumice-transcribe");
    let mut command = isolated_command(bin);
    command
        .args(["check-config"])
        .env("HOME", tmp.path())
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("APPDATA");
    if cfg!(windows) {
        command.env("APPDATA", &base);
    }
    let out = command.output().unwrap();

    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains(config_path.to_str().unwrap()),
        "stdout: {stdout}"
    );
}

#[cfg(unix)]
#[test]
fn broken_per_user_config_symlink_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let config_dir = tmp.path().join(".config").join("pumice");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("transcribe.yml");
    std::os::unix::fs::symlink(config_dir.join("missing.yml"), &config_path).unwrap();

    let out = isolated_command(env!("CARGO_BIN_EXE_pumice-transcribe"))
        .args(["check-config", "--json"])
        .env("HOME", tmp.path())
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["error"]["code"], "invalid_config");
    assert_eq!(
        value["error"]["location"]["file"],
        config_path.to_str().unwrap()
    );
}

#[test]
fn help_mentions_config_and_model_flags() {
    let bin = env!("CARGO_BIN_EXE_pumice-transcribe");
    let top = isolated_command(bin).arg("--help").output().unwrap();
    let top_stdout = String::from_utf8(top.stdout).unwrap();
    assert!(top_stdout.contains("--config"));

    let file_help = isolated_command(bin)
        .args(["file", "--help"])
        .output()
        .unwrap();
    let file_stdout = String::from_utf8(file_help.stdout).unwrap();
    assert!(file_stdout.contains("--model"));
    assert!(file_stdout.contains("--model-file"));

    let models_help = isolated_command(bin)
        .args(["models", "--help"])
        .output()
        .unwrap();
    assert!(models_help.status.success());
}

#[test]
fn serve_missing_catalog_model_names_the_download_command() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("transcribe.yml");
    write_complete(
        &config,
        "schema_version: 1\nmodels_dir: models\nmodels:\n  - id: tiny\n    catalog: whisper-tiny-q8_0\n",
    )
    .unwrap();

    let out = isolated_command(env!("CARGO_BIN_EXE_pumice-transcribe"))
        .args([
            "--config",
            config.to_str().unwrap(),
            "serve",
            "--port",
            "0",
            "--model",
            "tiny",
        ])
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("models download whisper-tiny-q8_0"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains(&format!("--config {}", config.display())),
        "stderr must preserve --config: {stderr}"
    );
}

#[test]
fn file_missing_catalog_model_names_the_download_command() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("transcribe.yml");
    write_complete(
        &config,
        "schema_version: 1\nmodels_dir: models\nmodels:\n  - id: tiny\n    catalog: whisper-tiny-q8_0\n",
    )
    .unwrap();
    let wav = tmp.path().join("clip.wav");
    std::fs::write(&wav, write_silence_wav(0.05, 16_000, 1)).unwrap();

    let out = isolated_command(env!("CARGO_BIN_EXE_pumice-transcribe"))
        .args([
            "--config",
            config.to_str().unwrap(),
            "file",
            wav.to_str().unwrap(),
            "--model",
            "tiny",
        ])
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("models download whisper-tiny-q8_0"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains(&format!("--config {}", config.display())),
        "stderr must preserve --config: {stderr}"
    );
}

#[test]
fn file_missing_path_model_keeps_the_plain_error() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("transcribe.yml");
    write_complete(
        &config,
        "schema_version: 1\nmodels:\n  - id: local\n    path: absent.gguf\n",
    )
    .unwrap();
    let wav = tmp.path().join("clip.wav");
    std::fs::write(&wav, write_silence_wav(0.05, 16_000, 1)).unwrap();

    let out = isolated_command(env!("CARGO_BIN_EXE_pumice-transcribe"))
        .args([
            "--config",
            config.to_str().unwrap(),
            "file",
            wav.to_str().unwrap(),
            "--model",
            "local",
        ])
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("model file not found"), "stderr: {stderr}");
    assert!(
        !stderr.contains("models download"),
        "local path entries get no download hint: {stderr}"
    );
}
