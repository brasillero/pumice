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
            dynamic_backends::init_from_exe_dir().map_err(|error| {
                eprintln!("cpu-variants: {error}");
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
    /// Linux only). The variant is chosen here, not by ggml: ggml's pinned
    /// scoring reads CPUID without checking OS-enabled state (the FIXME in
    /// transcribe-cpp-sys ggml/src/ggml-cpu/arch/x86/cpu-feats.cpp), so a CPU
    /// or guest advertising AVX/AVX-512 with the state disabled could win
    /// ggml's probe and fault later. `choose_variant` instead detects through
    /// `std::arch::is_x86_feature_detected!`, which verifies OS support via
    /// OSXSAVE/XGETBV for the vector features, and maps ggml's variant names
    /// to the exact feature sets its CMake table compiles in.
    ///
    /// The safe bindings expose only directory-based loading, and ggml scores
    /// everything it finds in that directory. To make ggml load only the
    /// chosen module, it is staged alone in a private per-process directory
    /// inside the trusted install dir (never cwd, PATH or a temp dir) and
    /// removed right after registration; the plain `x64` module is the
    /// portable floor when nothing better is safe.
    #[cfg(all(feature = "cpu-variants", target_os = "linux"))]
    mod dynamic_backends {
        use std::fmt;
        use std::path::{Path, PathBuf};

        /// One x86 ISA feature a ggml CPU variant can require, mirroring the
        /// GGML_* compile definitions in ggml's pinned CMake variant table
        /// (transcribe-cpp-sys ggml/src/CMakeLists.txt,
        /// ggml_add_cpu_backend_variant). `Avx512` stands for the five base
        /// flags of ggml's AVX512 tier (-mavx512f/cd/vl/dq/bw).
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        enum CpuFeature {
            Sse42,
            Avx,
            F16c,
            Fma,
            Bmi2,
            Avx2,
            AvxVnni,
            Avx512,
            Avx512Vbmi,
            Avx512Vnni,
            Avx512Bf16,
            AmxTile,
            AmxInt8,
        }

        /// ggml's CPU variant table for a non-MSVC x86-64 build, weakest
        /// first. Equal feature counts resolve to the later (higher-tier)
        /// entry; the tiers nest, so ties only arise for theoretical feature
        /// combinations no shipping CPU has.
        const VARIANTS: &[(&str, &[CpuFeature])] = &[
            ("x64", &[]),
            ("sse42", &[CpuFeature::Sse42]),
            ("sandybridge", &[CpuFeature::Sse42, CpuFeature::Avx]),
            (
                "ivybridge",
                &[CpuFeature::Sse42, CpuFeature::Avx, CpuFeature::F16c],
            ),
            (
                "piledriver",
                &[
                    CpuFeature::Sse42,
                    CpuFeature::Avx,
                    CpuFeature::F16c,
                    CpuFeature::Fma,
                ],
            ),
            (
                "haswell",
                &[
                    CpuFeature::Sse42,
                    CpuFeature::Avx,
                    CpuFeature::F16c,
                    CpuFeature::Fma,
                    CpuFeature::Avx2,
                    CpuFeature::Bmi2,
                ],
            ),
            (
                "skylakex",
                &[
                    CpuFeature::Sse42,
                    CpuFeature::Avx,
                    CpuFeature::F16c,
                    CpuFeature::Fma,
                    CpuFeature::Avx2,
                    CpuFeature::Bmi2,
                    CpuFeature::Avx512,
                ],
            ),
            (
                "cannonlake",
                &[
                    CpuFeature::Sse42,
                    CpuFeature::Avx,
                    CpuFeature::F16c,
                    CpuFeature::Fma,
                    CpuFeature::Avx2,
                    CpuFeature::Bmi2,
                    CpuFeature::Avx512,
                    CpuFeature::Avx512Vbmi,
                ],
            ),
            (
                "cascadelake",
                &[
                    CpuFeature::Sse42,
                    CpuFeature::Avx,
                    CpuFeature::F16c,
                    CpuFeature::Fma,
                    CpuFeature::Avx2,
                    CpuFeature::Bmi2,
                    CpuFeature::Avx512,
                    CpuFeature::Avx512Vnni,
                ],
            ),
            (
                "icelake",
                &[
                    CpuFeature::Sse42,
                    CpuFeature::Avx,
                    CpuFeature::F16c,
                    CpuFeature::Fma,
                    CpuFeature::Avx2,
                    CpuFeature::Bmi2,
                    CpuFeature::Avx512,
                    CpuFeature::Avx512Vbmi,
                    CpuFeature::Avx512Vnni,
                ],
            ),
            (
                "cooperlake",
                &[
                    CpuFeature::Sse42,
                    CpuFeature::Avx,
                    CpuFeature::F16c,
                    CpuFeature::Fma,
                    CpuFeature::Avx2,
                    CpuFeature::Bmi2,
                    CpuFeature::Avx512,
                    CpuFeature::Avx512Vnni,
                    CpuFeature::Avx512Bf16,
                ],
            ),
            (
                "zen4",
                &[
                    CpuFeature::Sse42,
                    CpuFeature::Avx,
                    CpuFeature::F16c,
                    CpuFeature::Fma,
                    CpuFeature::Avx2,
                    CpuFeature::Bmi2,
                    CpuFeature::Avx512,
                    CpuFeature::Avx512Vbmi,
                    CpuFeature::Avx512Vnni,
                    CpuFeature::Avx512Bf16,
                ],
            ),
            (
                "alderlake",
                &[
                    CpuFeature::Sse42,
                    CpuFeature::Avx,
                    CpuFeature::F16c,
                    CpuFeature::Fma,
                    CpuFeature::Avx2,
                    CpuFeature::Bmi2,
                    CpuFeature::AvxVnni,
                ],
            ),
            (
                "sapphirerapids",
                &[
                    CpuFeature::Sse42,
                    CpuFeature::Avx,
                    CpuFeature::F16c,
                    CpuFeature::Fma,
                    CpuFeature::Avx2,
                    CpuFeature::Bmi2,
                    CpuFeature::Avx512,
                    CpuFeature::Avx512Vbmi,
                    CpuFeature::Avx512Vnni,
                    CpuFeature::Avx512Bf16,
                    CpuFeature::AmxTile,
                    CpuFeature::AmxInt8,
                ],
            ),
        ];

        /// The best variant whose features are all usable on this host, or
        /// the `x64` portable floor. The detector is injected so the choice
        /// is unit-testable independent of the host.
        fn choose_variant(detect: impl Fn(CpuFeature) -> bool) -> &'static str {
            VARIANTS
                .iter()
                .filter(|(_, required)| required.iter().all(|&f| detect(f)))
                .max_by_key(|(_, required)| required.len())
                .map(|(name, _)| *name)
                .unwrap_or("x64")
        }

        /// Host feature detection. `std::arch::is_x86_feature_detected!`
        /// checks OSXSAVE and the XCR0 state bits (via XGETBV) for `avx`,
        /// `avx2`, the `avx512*` features and `avxvnni`, so a CPU or guest
        /// that advertises instructions whose OS state is disabled reports as
        /// not having them — unlike ggml's CPUID-only scoring. Every variant
        /// with a VEX-encoded feature (F16C, AVX-VNNI) also requires `avx`,
        /// so such features are only reachable with OS AVX state enabled.
        #[cfg(target_arch = "x86_64")]
        fn detect(feature: CpuFeature) -> bool {
            use CpuFeature::*;
            match feature {
                Sse42 => std::arch::is_x86_feature_detected!("sse4.2"),
                Avx => std::arch::is_x86_feature_detected!("avx"),
                F16c => std::arch::is_x86_feature_detected!("f16c"),
                Fma => std::arch::is_x86_feature_detected!("fma"),
                Bmi2 => std::arch::is_x86_feature_detected!("bmi2"),
                Avx2 => std::arch::is_x86_feature_detected!("avx2"),
                AvxVnni => std::arch::is_x86_feature_detected!("avxvnni"),
                Avx512 => {
                    std::arch::is_x86_feature_detected!("avx512f")
                        && std::arch::is_x86_feature_detected!("avx512cd")
                        && std::arch::is_x86_feature_detected!("avx512vl")
                        && std::arch::is_x86_feature_detected!("avx512dq")
                        && std::arch::is_x86_feature_detected!("avx512bw")
                }
                Avx512Vbmi => std::arch::is_x86_feature_detected!("avx512vbmi"),
                Avx512Vnni => std::arch::is_x86_feature_detected!("avx512vnni"),
                Avx512Bf16 => std::arch::is_x86_feature_detected!("avx512bf16"),
                // AMX needs std intrinsics that are still unstable
                // (x86_amx_intrinsics) plus a per-process OS permission grant
                // beyond XCR0 state; never select it rather than risk a
                // fault. Sapphire Rapids CPUs still match the zen4 tier, the
                // same ISA without AMX.
                AmxTile | AmxInt8 => false,
            }
        }

        #[cfg(not(target_arch = "x86_64"))]
        fn detect(feature: CpuFeature) -> bool {
            let _ = feature;
            false
        }

        /// Why backend initialization failed, in fixed text safe to show on
        /// stderr: paths and variant names only, never native error strings.
        /// Each variant names its category (modules missing / staging failed
        /// / no usable CPU module) and carries the same recovery hint.
        #[derive(Debug)]
        pub enum InitError {
            ExeDir,
            NoModuleDir {
                exe_dir: PathBuf,
            },
            ChosenModuleMissing {
                variant: &'static str,
                module_dir: PathBuf,
            },
            StageFailed {
                module_dir: PathBuf,
            },
            NoUsableCpuModule {
                variant: &'static str,
                module_dir: PathBuf,
            },
        }

        impl fmt::Display for InitError {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let hint = "rebuild the staged folder with \
                            crates/transcription/scripts/build-cpu-variants.sh";
                match self {
                    InitError::ExeDir => {
                        write!(f, "cannot locate the executable directory; {hint}")
                    }
                    InitError::NoModuleDir { exe_dir } => write!(
                        f,
                        "no ggml CPU modules found next to the executable ({}) or in its lib/ \
                         subdirectory (modules missing); {hint}",
                        exe_dir.display()
                    ),
                    InitError::ChosenModuleMissing {
                        variant,
                        module_dir,
                    } => write!(
                        f,
                        "the selected ggml CPU module libggml-cpu-{variant}.so is missing from \
                         {} (modules missing); the staged build is incomplete; {hint}",
                        module_dir.display()
                    ),
                    InitError::StageFailed { module_dir } => write!(
                        f,
                        "cannot prepare a private module directory in {} (staging failed); \
                         check that the directory is writable; {hint}",
                        module_dir.display()
                    ),
                    InitError::NoUsableCpuModule {
                        variant,
                        module_dir,
                    } => write!(
                        f,
                        "ggml could not load the selected CPU module libggml-cpu-{variant}.so \
                         from {} (no usable CPU module); the staged libraries may not match \
                         this executable; {hint}",
                        module_dir.display()
                    ),
                }
            }
        }

        /// A private per-process directory inside the trusted install dir
        /// holding exactly one module — a copy of the chosen one — so ggml's
        /// directory probe can only see and load that module. Removed on
        /// drop; after `dlopen` the loaded module stays mapped, so cleanup
        /// never unloads it.
        struct PrivateModuleDir(PathBuf);

        impl PrivateModuleDir {
            fn stage(module_dir: &Path, variant: &str) -> Result<Self, InitError> {
                let fail = || InitError::StageFailed {
                    module_dir: module_dir.to_path_buf(),
                };
                let dir = module_dir.join(format!(".pumice-cpu-variant-{}", std::process::id()));
                std::fs::create_dir(&dir).map_err(|_| fail())?;
                let copied = std::fs::copy(
                    module_dir.join(format!("libggml-cpu-{variant}.so")),
                    dir.join(format!("libggml-cpu-{variant}.so")),
                );
                if let Err(_error) = copied {
                    let _ = std::fs::remove_dir_all(&dir);
                    return Err(fail());
                }
                Ok(Self(dir))
            }
        }

        impl Drop for PrivateModuleDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        /// Initialize backends, returning a fixed-text reason on failure.
        pub fn init_from_exe_dir() -> Result<(), InitError> {
            let exe_dir = std::env::current_exe()
                .ok()
                .and_then(|path| path.parent().map(Path::to_path_buf))
                .ok_or(InitError::ExeDir)?;
            let module_dir =
                backend_module_dir(&exe_dir).ok_or_else(|| InitError::NoModuleDir {
                    exe_dir: exe_dir.clone(),
                })?;
            let variant = choose_variant(detect);
            if !module_dir
                .join(format!("libggml-cpu-{variant}.so"))
                .is_file()
            {
                return Err(InitError::ChosenModuleMissing {
                    variant,
                    module_dir,
                });
            }
            // transcribe-cpp 0.3.1 exposes no per-path backend loader, so
            // stage the single chosen module alone; ggml's CPUID-only scoring
            // then has nothing else it could pick instead.
            let private = PrivateModuleDir::stage(&module_dir, variant)?;
            let result = ::transcribe_cpp::init_backends_with(
                Some(&private.0),
                ::transcribe_cpp::BackendMask::CPU,
            );
            drop(private);
            result.map_err(|_| InitError::NoUsableCpuModule {
                variant,
                module_dir,
            })?;
            eprintln!("cpu-variants: selected ggml CPU module libggml-cpu-{variant}.so");
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
            use CpuFeature::*;

            fn detector(have: &[CpuFeature]) -> impl Fn(CpuFeature) -> bool + '_ {
                move |feature| have.contains(&feature)
            }

            #[test]
            fn x64_is_the_portable_floor() {
                assert_eq!(choose_variant(detector(&[])), "x64");
            }

            #[test]
            fn sse42_only_cpu_gets_sse42() {
                assert_eq!(choose_variant(detector(&[Sse42])), "sse42");
            }

            #[test]
            fn avx_tiers_upgrade_stepwise() {
                let sandybridge = [Sse42, Avx];
                assert_eq!(choose_variant(detector(&sandybridge)), "sandybridge");
                let ivybridge = [Sse42, Avx, F16c];
                assert_eq!(choose_variant(detector(&ivybridge)), "ivybridge");
                let piledriver = [Sse42, Avx, F16c, Fma];
                assert_eq!(choose_variant(detector(&piledriver)), "piledriver");
            }

            #[test]
            fn haswell_set_chooses_haswell() {
                let haswell = [Sse42, Avx, F16c, Fma, Avx2, Bmi2];
                assert_eq!(choose_variant(detector(&haswell)), "haswell");
            }

            #[test]
            fn avx_vnni_upgrades_haswell_to_alderlake() {
                let alderlake = [Sse42, Avx, F16c, Fma, Avx2, Bmi2, AvxVnni];
                assert_eq!(choose_variant(detector(&alderlake)), "alderlake");
            }

            #[test]
            fn os_enabled_avx512_reaches_the_avx512_tiers() {
                let base = [Sse42, Avx, F16c, Fma, Avx2, Bmi2];
                assert_eq!(
                    choose_variant(detector(&[base.as_slice(), &[Avx512]].concat())),
                    "skylakex"
                );
                assert_eq!(
                    choose_variant(detector(&[base.as_slice(), &[Avx512, Avx512Vbmi]].concat())),
                    "cannonlake"
                );
                assert_eq!(
                    choose_variant(detector(&[base.as_slice(), &[Avx512, Avx512Vnni]].concat())),
                    "cascadelake"
                );
                assert_eq!(
                    choose_variant(detector(
                        &[base.as_slice(), &[Avx512, Avx512Vbmi, Avx512Vnni]].concat()
                    )),
                    "icelake"
                );
                assert_eq!(
                    choose_variant(detector(
                        &[base.as_slice(), &[Avx512, Avx512Vnni, Avx512Bf16]].concat()
                    )),
                    "cooperlake"
                );
                assert_eq!(
                    choose_variant(detector(
                        &[
                            base.as_slice(),
                            &[Avx512, Avx512Vbmi, Avx512Vnni, Avx512Bf16]
                        ]
                        .concat()
                    )),
                    "zen4"
                );
            }

            #[test]
            fn amx_completes_sapphirerapids() {
                let sapphire = [
                    Sse42, Avx, F16c, Fma, Avx2, Bmi2, Avx512, Avx512Vbmi, Avx512Vnni, Avx512Bf16,
                    AmxTile, AmxInt8,
                ];
                assert_eq!(choose_variant(detector(&sapphire)), "sapphirerapids");
            }

            #[test]
            fn avx512_advertised_but_os_disabled_is_not_chosen() {
                // std's detector reports the avx512 features as absent when
                // the OS has not enabled their XSAVE state, even though CPUID
                // advertises them; a detector reporting exactly that must
                // keep every AVX-512 (and AMX) tier out, falling back to the
                // best non-AVX-512 variant.
                let avx512_os_disabled = [
                    Sse42, Avx, F16c, Fma, Avx2, Bmi2, Avx512Vbmi, Avx512Vnni, Avx512Bf16, AmxTile,
                    AmxInt8,
                ];
                assert_eq!(choose_variant(detector(&avx512_os_disabled)), "haswell");
            }

            #[test]
            fn avx_os_disabled_keeps_out_vex_tiers() {
                // Same shape for plain AVX: with the OS AVX state disabled,
                // only the non-AVX tiers are selectable.
                let avx_os_disabled = [Sse42];
                assert_eq!(choose_variant(detector(&avx_os_disabled)), "sse42");
            }

            #[test]
            fn private_module_dir_holds_one_module_and_cleans_up() {
                let temp = tempfile::tempdir().unwrap();
                std::fs::write(temp.path().join("libggml-cpu-haswell.so"), b"module").unwrap();
                let staged_path;
                {
                    let staged = PrivateModuleDir::stage(temp.path(), "haswell").unwrap();
                    staged_path = staged.0.clone();
                    assert_eq!(
                        std::fs::read(staged_path.join("libggml-cpu-haswell.so")).unwrap(),
                        b"module"
                    );
                }
                assert!(!staged_path.exists());
                assert!(temp.path().exists());
            }

            #[test]
            fn init_errors_name_their_category() {
                let temp = tempfile::tempdir().unwrap();
                let cases = [
                    (
                        InitError::NoModuleDir {
                            exe_dir: temp.path().to_path_buf(),
                        },
                        "modules missing",
                    ),
                    (
                        InitError::ChosenModuleMissing {
                            variant: "haswell",
                            module_dir: temp.path().to_path_buf(),
                        },
                        "modules missing",
                    ),
                    (
                        InitError::StageFailed {
                            module_dir: temp.path().to_path_buf(),
                        },
                        "staging failed",
                    ),
                    (
                        InitError::NoUsableCpuModule {
                            variant: "haswell",
                            module_dir: temp.path().to_path_buf(),
                        },
                        "no usable CPU module",
                    ),
                ];
                for (error, category) in cases {
                    let message = error.to_string();
                    assert!(message.contains(category), "{message}");
                    assert!(message.contains("build-cpu-variants.sh"), "{message}");
                }
            }

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
