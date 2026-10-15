use crate::audio::{
    MAX_FILE_BYTES, decode_audio_with_cancel, normalize_and_validate, resample_to_target,
};
use crate::config::{Config, validate_language};
use crate::engine::{CancelToken, Engine};
use crate::{Error, ErrorCode, Transcript, TranscriptionOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Result of the shared decode → resample → engine pipeline.
pub struct TranscriptionResult {
    pub transcript: Transcript,
    pub audio_seconds: f64,
}

/// Decode and resample audio from a reader, returning 16 kHz mono PCM and its
/// duration in seconds. Used by the `file` command to validate audio before
/// loading the model, and by the HTTP server.
pub fn decode_audio_from_reader<R: Read + Send + Sync>(
    reader: R,
) -> Result<(Vec<f32>, f64), Error> {
    decode_audio_from_reader_with_cancel(reader, &CancelToken::new())
}

/// [`decode_audio_from_reader`] that stops between decoder packets and between
/// stages once `cancel` is set.
pub fn decode_audio_from_reader_with_cancel<R: Read + Send + Sync>(
    reader: R,
    cancel: &CancelToken,
) -> Result<(Vec<f32>, f64), Error> {
    let budgeted = BudgetReader::new(reader, MAX_FILE_BYTES);
    let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        decode_audio_with_cancel(budgeted, cancel)
    }))
    .map_err(|_| Error::invalid_input("audio decoding panicked"))?
    .map_err(map_budget_error)?;
    check_cancel(cancel)?;
    let resampled = resample_to_target(&decoded.samples, decoded.sample_rate)?;
    check_cancel(cancel)?;
    let pcm = normalize_and_validate(resampled)?;
    let audio_seconds = pcm.len() as f64 / 16_000.0;
    Ok((pcm, audio_seconds))
}

/// Run inference on already-decoded 16 kHz mono PCM.
pub fn transcribe_pcm(
    engine: &dyn Engine,
    pcm: &[f32],
    options: &TranscriptionOptions,
    cancel: &CancelToken,
) -> Result<Transcript, Error> {
    engine
        .transcribe(pcm, options, cancel)
        .map_err(|e| e.to_error())
}

/// Shared decode → resample → engine pipeline used by the `file` command and
/// the HTTP server. Reading and decoding run on the caller's thread; the
/// engine call itself is sync and may be run on a dedicated blocking worker.
pub fn transcribe_pcm_from_reader<R: Read + Send + Sync>(
    reader: R,
    engine: &dyn Engine,
    options: &TranscriptionOptions,
    cancel: &CancelToken,
) -> Result<TranscriptionResult, Error> {
    let (pcm, audio_seconds) = decode_audio_from_reader_with_cancel(reader, cancel)?;
    check_cancel(cancel)?;
    let transcript = transcribe_pcm(engine, &pcm, options, cancel)?;
    // A run that finished after cancellation is discarded, never published.
    check_cancel(cancel)?;
    Ok(TranscriptionResult {
        transcript,
        audio_seconds,
    })
}

/// Resolve a model selection against the configuration. Returns the configured
/// model id and its absolute path.
pub fn resolve_model_id(
    config: &Config,
    model_id: Option<&str>,
    model_file: Option<&Path>,
) -> Result<(String, PathBuf), Error> {
    match (model_id, model_file) {
        (Some(_), Some(_)) => Err(Error::invalid_arguments(
            "--model and --model-file are mutually exclusive",
        )),
        (None, Some(path)) => Ok((
            path.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "model".to_string()),
            path.to_path_buf(),
        )),
        (Some(id), None) => {
            let entry = config
                .models
                .iter()
                .find(|m| m.id == *id)
                .ok_or_else(|| Error::invalid_arguments(unknown_model_message(id, config)))?;
            Ok((id.to_string(), entry.path.clone()))
        }
        (None, None) => match &config.default_model {
            Some(id) => {
                let entry = config
                    .models
                    .iter()
                    .find(|m| m.id == *id)
                    .expect("default_model was validated against models");
                Ok((id.clone(), entry.path.clone()))
            }
            None => Err(Error::invalid_arguments(
                "no model selected; use --model ID, --model-file PATH, or set default_model in transcribe.yml",
            )),
        },
    }
}

fn unknown_model_message(id: &str, config: &Config) -> String {
    let ids: Vec<&str> = config.models.iter().map(|m| m.id.as_str()).collect();
    let list = if ids.is_empty() {
        "(none configured)".to_string()
    } else {
        ids.join(", ")
    };
    format!("unknown model id `{id}`; configured ids: {list}")
}

/// Normalize a language hint and run it through the same validation as the
/// configuration file.
pub fn normalize_language(language: Option<String>) -> Result<Option<String>, Error> {
    let Some(lang) = language else {
        return Ok(None);
    };
    validate_language(&lang)
        .map(Some)
        .map_err(Error::invalid_arguments)
}

/// Normalize a thread count request to an i32.
pub fn normalize_threads(threads: Option<usize>) -> Result<i32, Error> {
    const MAX_THREADS: i32 = 32;
    match threads {
        Some(0) => Err(Error::invalid_arguments("--threads must be at least 1")),
        Some(n) if n > MAX_THREADS as usize => {
            Err(Error::invalid_arguments("--threads must be at most 32"))
        }
        Some(n) => Ok(n as i32),
        None => Ok(std::thread::available_parallelism()
            .map(|p| (p.get() as i32).min(MAX_THREADS))
            .unwrap_or(1)),
    }
}

fn check_cancel(cancel: &CancelToken) -> Result<(), Error> {
    if cancel.is_cancelled() {
        Err(Error::cancelled())
    } else {
        Ok(())
    }
}

fn map_budget_error(err: Error) -> Error {
    if err.code == ErrorCode::InvalidInput && err.message.contains("byte budget exceeded") {
        Error::input_too_large(MAX_FILE_BYTES)
    } else {
        err
    }
}

/// A reader that caps the total bytes read without buffering more than the
/// provided budget.
pub struct BudgetReader<R> {
    inner: R,
    budget: u64,
    read: u64,
    overshot: bool,
}

impl<R> BudgetReader<R> {
    pub fn new(inner: R, budget: u64) -> Self {
        Self {
            inner,
            budget,
            read: 0,
            overshot: false,
        }
    }
}

impl<R: Read> Read for BudgetReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.overshot {
            return Err(std::io::Error::other("byte budget exceeded"));
        }
        if self.read >= self.budget {
            let mut probe = [0u8; 1];
            let n = self.inner.read(&mut probe)?;
            if n == 0 {
                return Ok(0);
            }
            self.overshot = true;
            return Err(std::io::Error::other("byte budget exceeded"));
        }
        let max = ((self.budget - self.read) as usize).min(buf.len());
        let n = self.inner.read(&mut buf[..max])?;
        self.read += n as u64;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorCode;
    use std::io::Cursor;

    #[test]
    fn normalize_language_accepts_supported_code() {
        assert_eq!(
            normalize_language(Some("pt".to_string())).unwrap(),
            Some("pt".to_string())
        );
    }

    #[test]
    fn normalize_language_uppercase_is_normalized() {
        assert_eq!(
            normalize_language(Some("PT".to_string())).unwrap(),
            Some("pt".to_string())
        );
    }

    #[test]
    fn normalize_language_rejects_non_iso_code() {
        assert_eq!(
            normalize_language(Some("portuguese".to_string()))
                .unwrap_err()
                .code,
            ErrorCode::InvalidArguments
        );
    }

    #[test]
    fn normalize_language_rejects_unknown_code() {
        assert_eq!(
            normalize_language(Some("zz".to_string())).unwrap_err().code,
            ErrorCode::InvalidArguments
        );
    }

    #[test]
    fn normalize_threads_caps_default() {
        let threads = normalize_threads(None).unwrap();
        assert!((1..=32).contains(&threads));
    }

    #[test]
    fn normalize_threads_rejects_zero() {
        assert_eq!(
            normalize_threads(Some(0)).unwrap_err().code,
            ErrorCode::InvalidArguments
        );
    }

    #[test]
    fn normalize_threads_rejects_too_large() {
        assert_eq!(
            normalize_threads(Some(33)).unwrap_err().code,
            ErrorCode::InvalidArguments
        );
    }

    #[test]
    fn budget_reader_one_under_limit_reads_all() {
        let data = vec![0u8; MAX_FILE_BYTES as usize - 1];
        let mut reader = BudgetReader::new(Cursor::new(&data), MAX_FILE_BYTES);
        let mut out = Vec::new();
        reader.read_to_end(&mut out).unwrap();
        assert_eq!(out.len(), data.len());
    }

    #[test]
    fn budget_reader_exact_limit_reads_all() {
        let data = vec![0u8; MAX_FILE_BYTES as usize];
        let mut reader = BudgetReader::new(Cursor::new(&data), MAX_FILE_BYTES);
        let mut out = Vec::new();
        reader.read_to_end(&mut out).unwrap();
        assert_eq!(out.len(), data.len());
    }

    #[test]
    fn budget_reader_one_over_limit_rejects() {
        let data = vec![0u8; MAX_FILE_BYTES as usize + 1];
        let mut reader = BudgetReader::new(Cursor::new(&data), MAX_FILE_BYTES);
        let mut out = Vec::new();
        let err = reader.read_to_end(&mut out).unwrap_err();
        assert!(err.to_string().contains("byte budget exceeded"));
    }
}
