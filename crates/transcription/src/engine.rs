use crate::{EngineError, Transcript, TranscriptionOptions};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A clonable cooperative-cancellation handle.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

pub trait Engine: Send + Sync {
    fn transcribe(
        &self,
        pcm_16khz_mono: &[f32],
        options: &TranscriptionOptions,
        cancel: &CancelToken,
    ) -> Result<Transcript, EngineError>;
}

#[derive(Debug, Clone)]
pub struct FakeEngine {
    text: String,
    language: Option<String>,
}

impl FakeEngine {
    pub fn new(text: impl Into<String>, language: Option<impl Into<String>>) -> Self {
        Self {
            text: text.into(),
            language: language.map(Into::into),
        }
    }
}

impl Engine for FakeEngine {
    fn transcribe(
        &self,
        _pcm_16khz_mono: &[f32],
        _options: &TranscriptionOptions,
        _cancel: &CancelToken,
    ) -> Result<Transcript, EngineError> {
        Ok(Transcript {
            text: self.text.clone(),
            language: self.language.clone(),
        })
    }
}

#[cfg(feature = "engine")]
pub mod cpp {
    use super::*;
    use ::transcribe_cpp::{
        Backend, CancelToken as CppCancelToken, Model, ModelOptions, RunExtension, RunOptions,
        SessionOptions, WhisperRunOptions,
    };
    use std::path::Path;

    pub struct TranscribeCppEngine {
        model: Model,
        threads: i32,
    }

    impl TranscribeCppEngine {
        pub fn new(model_path: &Path, threads: i32) -> Result<Self, EngineError> {
            // Native logs can contain dictated text; disable them before loading.
            transcribe_cpp::disable_logging();
            let model = Model::load_with(
                model_path,
                &ModelOptions {
                    backend: Backend::Cpu,
                    ..Default::default()
                },
            )
            .map_err(|_| EngineError::ModelLoadFailed)?;
            Ok(Self { model, threads })
        }
    }

    impl Engine for TranscribeCppEngine {
        fn transcribe(
            &self,
            pcm_16khz_mono: &[f32],
            options: &TranscriptionOptions,
            cancel: &CancelToken,
        ) -> Result<Transcript, EngineError> {
            let mut session = self
                .model
                .session_with(&SessionOptions {
                    n_threads: self.threads,
                    ..Default::default()
                })
                .map_err(|_| EngineError::Failed)?;

            if cancel.is_cancelled() {
                return Err(EngineError::Cancelled);
            }

            let cpp_token = CppCancelToken::new();
            session.set_cancel_token(&cpp_token);

            // Bridge our CancelToken into the native cancel callback on a short
            // polling thread; cooperative cancellation is the only safe way to
            // stop an in-flight native run.
            // The watcher stops as soon as the run ends and is joined below, so no
            // thread outlives the call.
            let finished = Arc::new(AtomicBool::new(false));
            let watcher = {
                let finished = Arc::clone(&finished);
                let native_token = cpp_token.clone();
                let cancel = cancel.clone();
                std::thread::spawn(move || {
                    while !finished.load(Ordering::SeqCst) {
                        if cancel.is_cancelled() {
                            native_token.cancel();
                            return;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                })
            };

            let mut whisper_ext = WhisperRunOptions::default();
            if let Some(prompt) = &options.prompt {
                whisper_ext.initial_prompt = Some(prompt.clone());
            }
            if let Some(temperature) = options.temperature {
                whisper_ext.temperature = Some(temperature);
            }

            let run_options = RunOptions {
                language: options.language.clone(),
                family: Some(RunExtension::Whisper(whisper_ext)),
                ..Default::default()
            };

            let run = session.run(pcm_16khz_mono, &run_options);
            finished.store(true, Ordering::SeqCst);
            let _ = watcher.join();

            if cancel.is_cancelled() {
                return Err(EngineError::Cancelled);
            }
            let result = run.map_err(|_| EngineError::Failed)?;

            Ok(Transcript {
                text: result.text,
                language: result.language,
            })
        }
    }
}
