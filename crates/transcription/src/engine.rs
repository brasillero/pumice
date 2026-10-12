use crate::{EngineError, Transcript, TranscriptionOptions};

pub trait Engine: Send + Sync {
    fn transcribe(
        &self,
        pcm_16khz_mono: &[f32],
        options: &TranscriptionOptions,
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
    use ::transcribe_cpp::{Backend, Model, ModelOptions, RunOptions, SessionOptions};
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
        ) -> Result<Transcript, EngineError> {
            let mut session = self
                .model
                .session_with(&SessionOptions {
                    n_threads: self.threads,
                    ..Default::default()
                })
                .map_err(|_| EngineError::Failed)?;

            let run_options = RunOptions {
                language: options.language.clone(),
                ..Default::default()
            };

            let result = session
                .run(pcm_16khz_mono, &run_options)
                .map_err(|_| EngineError::Failed)?;

            Ok(Transcript {
                text: result.text,
                language: result.language,
            })
        }
    }
}
