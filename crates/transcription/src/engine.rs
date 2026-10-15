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
    /// from /proc/self/maps. The safe bindings expose no direct "which
    /// variant" query; the mapped file is the ground truth. `None` outside
    /// Linux or when no variant module is mapped.
    #[cfg(target_os = "linux")]
    fn selected_cpu_variant() -> Option<String> {
        let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
        maps.lines().find_map(variant_from_maps_line)
    }

    /// Extracts the variant name from one /proc/self/maps line. The pathname
    /// is everything after the five metadata columns (address, perms, offset,
    /// dev, inode) and may itself contain spaces and end in " (deleted)" for
    /// removed files, so it cannot be split on whitespace.
    #[cfg(target_os = "linux")]
    fn variant_from_maps_line(line: &str) -> Option<String> {
        let path = maps_pathname(line)?;
        let file = path.rsplit('/').next()?;
        let file = file.strip_suffix(" (deleted)").unwrap_or(file);
        file.strip_prefix("libggml-cpu-")
            .and_then(|rest| rest.strip_suffix(".so"))
            .map(str::to_string)
    }

    /// The pathname of one /proc/self/maps line: the remainder after the five
    /// whitespace-separated metadata columns. Returns `None` for anonymous
    /// mappings (no pathname).
    #[cfg(target_os = "linux")]
    fn maps_pathname(line: &str) -> Option<&str> {
        let mut rest = line;
        for _ in 0..5 {
            rest = rest.trim_start();
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            rest = &rest[end..];
        }
        let path = rest.trim_start();
        (!path.is_empty()).then_some(path)
    }

    #[cfg(all(test, target_os = "linux"))]
    mod maps_tests {
        use super::*;

        #[test]
        fn variant_is_found_in_a_plain_mapping() {
            let line = "7f8a1c000000-7f8a1d200000 r-xp 00000000 103:05 1234567 \
                        /opt/pumice/lib/libggml-cpu-haswell.so";
            assert_eq!(variant_from_maps_line(line).as_deref(), Some("haswell"));
        }

        #[test]
        fn pathname_with_spaces_is_preserved() {
            let line = "7f8a1c000000-7f8a1d200000 r-xp 00000000 103:05 1234567 \
                        /tmp/stage with spaces/libggml-cpu-alderlake.so";
            assert_eq!(variant_from_maps_line(line).as_deref(), Some("alderlake"));
        }

        #[test]
        fn deleted_mapping_is_still_recognized() {
            let line = "7f8a1c000000-7f8a1d200000 r-xp 00000000 103:05 1234567 \
                        /opt/pumice/lib/libggml-cpu-x64.so (deleted)";
            assert_eq!(variant_from_maps_line(line).as_deref(), Some("x64"));
        }

        #[test]
        fn anonymous_mapping_has_no_variant() {
            let line = "7f8a1c000000-7f8a1d200000 rw-p 00000000 00:00 0";
            assert_eq!(variant_from_maps_line(line), None);
        }

        #[test]
        fn non_module_path_has_no_variant() {
            let line = "55d0f1200000-55d0f1221000 r-xp 00000000 103:05 7654321 \
                        /opt/pumice/bin/pumice-transcribe";
            assert_eq!(variant_from_maps_line(line), None);
        }

        #[test]
        fn core_library_path_has_no_variant() {
            let line = "7f8a1c000000-7f8a1d200000 r-xp 00000000 103:05 1234567 \
                        /opt/pumice/lib/libggml-base.so.0";
            assert_eq!(variant_from_maps_line(line), None);
        }
    }

    /// Runtime CPU-variant backend initialization (`cpu-variants` feature,
    /// Linux x86-64 only). The variant is chosen here, not by ggml: ggml's pinned
    /// scoring reads CPUID without checking OS-enabled state (the FIXME in
    /// transcribe-cpp-sys ggml/src/ggml-cpu/arch/x86/cpu-feats.cpp), so a CPU
    /// or guest advertising AVX/AVX-512 with the state disabled could win
    /// ggml's probe and fault later. `choose_variant` instead detects through
    /// `std::arch::is_x86_feature_detected!`, which verifies OS support via
    /// OSXSAVE/XGETBV for the vector features, and maps ggml's variant names
    /// to the exact feature sets its CMake table compiles in.
    ///
    /// The safe bindings expose only directory-based loading, and ggml scores
    /// everything it finds in that directory. The staging script therefore
    /// places each CPU module alone in `<exe dir>/cpu/<variant>/` at build
    /// time, and this module initializes backends from exactly that one
    /// directory — never cwd, PATH or a temp dir, and nothing is copied or
    /// written at runtime, so a read-only install directory works. The plain
    /// `x64` module is the portable floor when nothing better is safe.
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
        /// first. Equal feature counts resolve to the later table entry.
        /// The table is not a strict nesting hierarchy — `alderlake`, for
        /// example, follows `zen4` but swaps AVX-512 for AVX-VNNI — so
        /// never treat table position alone as proof that another entry is
        /// safe on a given host.
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

        /// Variants whose features are all usable on this host, weakest
        /// first, in the preference order `choose_variant` uses: feature
        /// count ascending, table order on ties (the later entry wins).
        /// `x64` has no requirements, so the list is never empty.
        fn compatible_variants(
            detect: impl Fn(CpuFeature) -> bool,
        ) -> Vec<(&'static str, &'static [CpuFeature])> {
            let mut variants: Vec<_> = VARIANTS
                .iter()
                .filter(|(_, required)| required.iter().all(|&f| detect(f)))
                .copied()
                .collect();
            variants.sort_by_key(|(_, required)| required.len());
            variants
        }

        /// The best variant whose features are all usable on this host, or
        /// the `x64` portable floor. The detector is injected so the choice
        /// is unit-testable independent of the host.
        fn choose_variant(detect: impl Fn(CpuFeature) -> bool) -> &'static str {
            compatible_variants(detect)
                .last()
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
        /// Each variant names its category (modules missing / no usable CPU
        /// module) and carries the same recovery hint.
        #[derive(Debug)]
        pub enum InitError {
            ExeDir,
            ModulesMissing {
                exe_dir: PathBuf,
                variant: &'static str,
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
                    InitError::ModulesMissing { exe_dir, variant } => write!(
                        f,
                        "no staged ggml CPU module for libggml-cpu-{variant}.so or any lower \
                         variant under {} (modules missing); the staged build is incomplete; \
                         {hint}",
                        exe_dir.join("cpu").display()
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

        /// The staged module file for one variant:
        /// `<exe dir>/cpu/<variant>/libggml-cpu-<variant>.so`.
        fn variant_module_file(exe_dir: &Path, variant: &str) -> PathBuf {
            exe_dir
                .join("cpu")
                .join(variant)
                .join(format!("libggml-cpu-{variant}.so"))
        }

        /// The staged module directory to initialize backends from: the
        /// chosen variant's, or the next less-preferred compatible variant
        /// whose module was staged. Candidates are exactly the variants
        /// whose required features pass the same OS-aware detector, in
        /// preference order — the table is not a nesting hierarchy, so a
        /// module earlier in the table is not automatically safe here.
        /// `x64` is always the last candidate. `None` when no candidate's
        /// module was staged.
        fn select_module_dir(
            exe_dir: &Path,
            chosen: &'static str,
            detect: impl Fn(CpuFeature) -> bool,
        ) -> Option<(&'static str, PathBuf)> {
            let candidates = compatible_variants(detect);
            let chosen_idx = candidates.iter().position(|(name, _)| *name == chosen)?;
            candidates[..=chosen_idx]
                .iter()
                .rev()
                .find(|(name, _)| variant_module_file(exe_dir, name).is_file())
                .map(|(name, _)| (*name, exe_dir.join("cpu").join(name)))
        }

        /// Initialize backends, returning a fixed-text reason on failure.
        pub fn init_from_exe_dir() -> Result<(), InitError> {
            let exe_dir = std::env::current_exe()
                .ok()
                .and_then(|path| path.parent().map(Path::to_path_buf))
                .ok_or(InitError::ExeDir)?;
            let chosen = choose_variant(detect);
            let (variant, module_dir) =
                select_module_dir(&exe_dir, chosen, detect).ok_or_else(|| {
                    InitError::ModulesMissing {
                        exe_dir: exe_dir.clone(),
                        variant: chosen,
                    }
                })?;
            ::transcribe_cpp::init_backends_with(
                Some(&module_dir),
                ::transcribe_cpp::BackendMask::CPU,
            )
            .map_err(|_| InitError::NoUsableCpuModule {
                variant,
                module_dir: module_dir.clone(),
            })?;
            if variant == chosen {
                eprintln!("cpu-variants: selected ggml CPU module libggml-cpu-{variant}.so");
            } else {
                eprintln!(
                    "cpu-variants: selected ggml CPU module libggml-cpu-{variant}.so (fallback: \
                     libggml-cpu-{chosen}.so is not staged)"
                );
            }
            Ok(())
        }

        #[cfg(test)]
        mod tests {
            use super::*;
            use CpuFeature::*;

            fn detector(have: &[CpuFeature]) -> impl Fn(CpuFeature) -> bool + '_ {
                move |feature| have.contains(&feature)
            }

            /// Every feature the host detector can ever report (AMX is
            /// never reported on stable Rust), so all non-AMX variants are
            /// compatible unless a test injects a narrower set.
            const HOST_FEATURES: &[CpuFeature] = &[
                Sse42, Avx, F16c, Fma, Bmi2, Avx2, AvxVnni, Avx512, Avx512Vbmi, Avx512Vnni,
                Avx512Bf16,
            ];

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
            fn init_errors_name_their_category() {
                let temp = tempfile::tempdir().unwrap();
                let cases = [
                    (
                        InitError::ModulesMissing {
                            exe_dir: temp.path().to_path_buf(),
                            variant: "haswell",
                        },
                        "modules missing",
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

            fn stage_module(exe_dir: &Path, variant: &str) {
                let dir = exe_dir.join("cpu").join(variant);
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join(format!("libggml-cpu-{variant}.so")), b"").unwrap();
            }

            #[test]
            fn chosen_variant_directory_is_selected() {
                let temp = tempfile::tempdir().unwrap();
                stage_module(temp.path(), "x64");
                stage_module(temp.path(), "haswell");
                let (variant, dir) =
                    select_module_dir(temp.path(), "haswell", detector(HOST_FEATURES)).unwrap();
                assert_eq!(variant, "haswell");
                assert_eq!(dir, temp.path().join("cpu").join("haswell"));
            }

            #[test]
            fn missing_chosen_variant_falls_back_to_a_lower_one() {
                let temp = tempfile::tempdir().unwrap();
                stage_module(temp.path(), "sse42");
                stage_module(temp.path(), "x64");
                let (variant, dir) =
                    select_module_dir(temp.path(), "haswell", detector(HOST_FEATURES)).unwrap();
                assert_eq!(variant, "sse42");
                assert_eq!(dir, temp.path().join("cpu").join("sse42"));
            }

            #[test]
            fn missing_best_variant_skips_incompatible_higher_tiers() {
                // Host: up to alderlake, no AVX-512. `zen4` sits right below
                // alderlake in the table but requires AVX-512, so with
                // `cpu/alderlake/` missing the fallback must skip it and
                // land on haswell, not fail on an unusable module.
                let alderlake_host = [Sse42, Avx, F16c, Fma, Avx2, Bmi2, AvxVnni];
                let temp = tempfile::tempdir().unwrap();
                stage_module(temp.path(), "zen4");
                stage_module(temp.path(), "haswell");
                stage_module(temp.path(), "x64");
                let (variant, dir) =
                    select_module_dir(temp.path(), "alderlake", detector(&alderlake_host)).unwrap();
                assert_eq!(variant, "haswell");
                assert_eq!(dir, temp.path().join("cpu").join("haswell"));
            }

            #[test]
            fn missing_cascadelake_skips_predecessors_it_cannot_run() {
                // Same shape one tier up: cascadelake's module is missing,
                // and cooperlake/zen4 — staged, and earlier in the table —
                // require AVX512BF16/VBMI this host lacks. The next
                // compatible staged variant is skylakex.
                let cascadelake_host = [Sse42, Avx, F16c, Fma, Avx2, Bmi2, Avx512, Avx512Vnni];
                let temp = tempfile::tempdir().unwrap();
                stage_module(temp.path(), "zen4");
                stage_module(temp.path(), "cooperlake");
                stage_module(temp.path(), "skylakex");
                stage_module(temp.path(), "x64");
                let (variant, dir) =
                    select_module_dir(temp.path(), "cascadelake", detector(&cascadelake_host))
                        .unwrap();
                assert_eq!(variant, "skylakex");
                assert_eq!(dir, temp.path().join("cpu").join("skylakex"));
            }

            #[test]
            fn only_x64_staged_still_runs() {
                let temp = tempfile::tempdir().unwrap();
                stage_module(temp.path(), "x64");
                let (variant, dir) =
                    select_module_dir(temp.path(), "alderlake", detector(HOST_FEATURES)).unwrap();
                assert_eq!(variant, "x64");
                assert_eq!(dir, temp.path().join("cpu").join("x64"));
            }

            #[test]
            fn empty_variant_directory_does_not_count() {
                let temp = tempfile::tempdir().unwrap();
                std::fs::create_dir_all(temp.path().join("cpu").join("haswell")).unwrap();
                stage_module(temp.path(), "x64");
                let (variant, _) =
                    select_module_dir(temp.path(), "haswell", detector(HOST_FEATURES)).unwrap();
                assert_eq!(variant, "x64");
            }

            #[test]
            fn no_staged_variant_is_an_error() {
                let temp = tempfile::tempdir().unwrap();
                assert_eq!(
                    select_module_dir(temp.path(), "haswell", detector(HOST_FEATURES)),
                    None
                );
            }

            #[test]
            fn flat_or_versioned_module_files_do_not_count() {
                let temp = tempfile::tempdir().unwrap();
                // Old flat layout or stray files must not be mistaken for
                // staged per-variant directories.
                std::fs::write(temp.path().join("libggml-cpu-haswell.so"), b"").unwrap();
                std::fs::write(temp.path().join("libggml-base.so"), b"").unwrap();
                std::fs::write(temp.path().join("notes.txt"), b"").unwrap();
                assert_eq!(
                    select_module_dir(temp.path(), "haswell", detector(HOST_FEATURES)),
                    None
                );
            }

            #[test]
            fn read_only_install_dir_needs_no_writes() {
                use std::os::unix::fs::PermissionsExt;
                let temp = tempfile::tempdir().unwrap();
                stage_module(temp.path(), "x64");
                let cpu = temp.path().join("cpu");
                let x64 = cpu.join("x64");
                for dir in [&cpu, &x64] {
                    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
                }
                let (variant, dir) =
                    select_module_dir(temp.path(), "alderlake", detector(HOST_FEATURES)).unwrap();
                assert_eq!(variant, "x64");
                assert_eq!(dir, x64);
                assert!(variant_module_file(temp.path(), "x64").is_file());
                for dir in [&cpu, &x64] {
                    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
                }
            }
        }
    }
}
