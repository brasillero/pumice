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

    /// Short label for the compute backend in use, e.g. `CPU [haswell]`, when
    /// the engine can know it. Used for status output only; never contains
    /// dictated text.
    fn backend_label(&self) -> Option<String> {
        None
    }
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
        backend_label: Option<String>,
    }

    impl TranscribeCppEngine {
        pub fn new(model_path: &Path, threads: i32) -> Result<Self, EngineError> {
            // Native logs can contain dictated text; disable them before loading.
            transcribe_cpp::disable_logging();
            #[cfg(all(feature = "cpu-variants", target_os = "linux"))]
            dynamic_backends::init_from_exe_dir().map_err(|message| {
                eprintln!("cpu-variants: {message}");
                EngineError::Failed
            })?;
            let model = Model::load_with(
                model_path,
                &ModelOptions {
                    backend: Backend::Cpu,
                    ..Default::default()
                },
            )
            .map_err(|_| EngineError::ModelLoadFailed)?;
            let backend_label = backend_label(&model);
            Ok(Self {
                model,
                threads,
                backend_label,
            })
        }
    }

    impl Engine for TranscribeCppEngine {
        fn backend_label(&self) -> Option<String> {
            self.backend_label.clone()
        }

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

    /// Human-readable backend/device label for status output, e.g.
    /// `CPU (Intel ...) [haswell]`. Fixed text plus hardware names only.
    fn backend_label(model: &Model) -> Option<String> {
        let label = match model.device() {
            Ok(device) => {
                if device.description.is_empty() {
                    device.name
                } else {
                    format!("{} ({})", device.name, device.description)
                }
            }
            Err(_) => model.backend(),
        };
        #[cfg(target_os = "linux")]
        if let Some(variant) = selected_cpu_variant() {
            return Some(format!("{label} [{variant}]"));
        }
        Some(label)
    }

    /// Name of the dlopen'd ggml CPU variant module (e.g. `haswell`), read
    /// from /proc/self/maps. ggml picks the module by runtime CPU feature
    /// scoring, and the safe bindings expose no direct "which variant" query;
    /// the mapped file is the ground truth. `None` outside Linux or when no
    /// variant module is mapped.
    #[cfg(target_os = "linux")]
    fn selected_cpu_variant() -> Option<String> {
        let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
        maps.lines().find_map(|line| {
            let path = line.split_whitespace().nth(5)?;
            let file = path.rsplit('/').next()?;
            file.strip_prefix("libggml-cpu-")
                .and_then(|rest| rest.strip_suffix(".so"))
                .map(str::to_string)
        })
    }

    /// Runtime CPU-variant backend initialization (`cpu-variants` feature,
    /// Linux only). Loads the ggml backend modules from a trusted directory
    /// next to the executable — never from the current directory, PATH or
    /// ggml's default search — and restricts registration to CPU backends.
    /// ggml scores every module against the running CPU and picks the best;
    /// modules the CPU cannot run score 0 and are skipped, with the plain x64
    /// module always scoring as the portable floor, so a missing feature
    /// degrades instead of raising SIGILL.
    #[cfg(all(feature = "cpu-variants", target_os = "linux"))]
    mod dynamic_backends {
        use std::path::{Path, PathBuf};

        /// Initialize backends, returning a fixed-text reason on failure.
        pub fn init_from_exe_dir() -> Result<(), String> {
            let exe_dir = std::env::current_exe()
                .ok()
                .and_then(|path| path.parent().map(Path::to_path_buf))
                .ok_or_else(|| "cannot locate the executable directory".to_string())?;
            let module_dir = backend_module_dir(&exe_dir).ok_or_else(|| {
                format!(
                    "no ggml backend modules found next to the executable ({}) or in its lib/ \
                     subdirectory; stage the shared libraries with \
                     scripts/build-cpu-variants.sh",
                    exe_dir.display()
                )
            })?;
            ::transcribe_cpp::init_backends_with(
                Some(&module_dir),
                ::transcribe_cpp::BackendMask::CPU,
            )
            .map_err(|_| {
                format!(
                    "failed to initialize CPU backend modules from {}",
                    module_dir.display()
                )
            })?;
            eprintln!(
                "cpu-variants: loaded ggml backend modules from {}",
                module_dir.display()
            );
            Ok(())
        }

        /// The directory to load modules from: the executable's directory, or
        /// its `lib/` subdirectory when the modules live there. Only these
        /// installation-relative locations are trusted.
        fn backend_module_dir(exe_dir: &Path) -> Option<PathBuf> {
            [exe_dir.to_path_buf(), exe_dir.join("lib")]
                .into_iter()
                .find(|dir| contains_backend_modules(dir))
        }

        /// Whether `name` is a real ggml CPU backend module filename, matching
        /// the pattern ggml's own loader probes (`libggml-cpu-*.so`, plus the
        /// unadorned `libggml-cpu.so`). Core libraries such as libggml-base.so
        /// must not count: in the documented layout they sit beside the
        /// executable, and counting them would stop the `lib/` fallback from
        /// ever being considered.
        fn is_cpu_module_name(name: &str) -> bool {
            (name.starts_with("libggml-cpu-") && name.ends_with(".so")) || name == "libggml-cpu.so"
        }

        fn contains_backend_modules(dir: &Path) -> bool {
            std::fs::read_dir(dir).is_ok_and(|entries| {
                entries.flatten().any(|entry| {
                    entry.path().is_file()
                        && is_cpu_module_name(&entry.file_name().to_string_lossy())
                })
            })
        }

        #[cfg(test)]
        mod tests {
            use super::*;

            #[test]
            fn no_modules_next_to_executable_is_an_error() {
                let temp = tempfile::tempdir().unwrap();
                assert_eq!(backend_module_dir(temp.path()), None);
            }

            #[test]
            fn modules_next_to_executable_are_preferred() {
                let temp = tempfile::tempdir().unwrap();
                std::fs::write(temp.path().join("libggml-cpu-x64.so"), b"").unwrap();
                assert_eq!(
                    backend_module_dir(temp.path()),
                    Some(temp.path().to_path_buf())
                );
            }

            #[test]
            fn lib_subdirectory_is_a_falllback() {
                let temp = tempfile::tempdir().unwrap();
                let lib = temp.path().join("lib");
                std::fs::create_dir(&lib).unwrap();
                std::fs::write(lib.join("libggml-cpu-haswell.so"), b"").unwrap();
                assert_eq!(backend_module_dir(temp.path()), Some(lib));
            }

            #[test]
            fn unrelated_files_do_not_count_as_modules() {
                let temp = tempfile::tempdir().unwrap();
                std::fs::write(temp.path().join("libggml-base.so"), b"").unwrap();
                std::fs::write(temp.path().join("libggml.so"), b"").unwrap();
                std::fs::write(temp.path().join("libtranscribe.so.0.3"), b"").unwrap();
                std::fs::write(temp.path().join("notes.txt"), b"").unwrap();
                assert_eq!(backend_module_dir(temp.path()), None);
            }

            #[test]
            fn versioned_module_names_do_not_count() {
                let temp = tempfile::tempdir().unwrap();
                std::fs::write(temp.path().join("libggml-cpu-x64.so.0.3.1"), b"").unwrap();
                assert_eq!(backend_module_dir(temp.path()), None);
            }

            #[test]
            fn core_libs_beside_executable_do_not_hide_lib_modules() {
                let temp = tempfile::tempdir().unwrap();
                // Documented layout: core libraries beside the executable,
                // CPU modules in lib/.
                for core in [
                    "libtranscribe.so.0.3",
                    "libggml.so.0",
                    "libggml-base.so.0",
                    "libggml-cpu-alderlake.so.0.3.1",
                ] {
                    std::fs::write(temp.path().join(core), b"").unwrap();
                }
                let lib = temp.path().join("lib");
                std::fs::create_dir(&lib).unwrap();
                std::fs::write(lib.join("libggml-cpu-haswell.so"), b"").unwrap();
                assert_eq!(backend_module_dir(temp.path()), Some(lib));
            }
        }
    }
}
