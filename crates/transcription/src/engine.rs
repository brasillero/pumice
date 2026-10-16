use crate::compute::{BackendMode, Compute};
use crate::{EngineError, Transcript, TranscriptionOptions};
use std::path::Path;
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

/// What the CLI asks an engine loader for.
#[derive(Debug, Clone, Copy)]
pub struct EngineRequest<'a> {
    pub model_path: &'a Path,
    pub threads: i32,
    /// From `--backend`, else `backend:` in transcribe.yml, else `cpu`.
    pub backend: BackendMode,
}

pub trait Engine: Send + Sync {
    fn transcribe(
        &self,
        pcm_16khz_mono: &[f32],
        options: &TranscriptionOptions,
        cancel: &CancelToken,
    ) -> Result<Transcript, EngineError>;

    /// The compute snapshot taken when the model loaded (see
    /// [`crate::compute`]), when the engine can know it. Fixed text plus
    /// bounded hardware names only; never dictated text.
    fn compute(&self) -> Option<Compute> {
        None
    }
}

#[derive(Debug, Clone)]
pub struct FakeEngine {
    text: String,
    language: Option<String>,
    compute: Option<Compute>,
}

impl FakeEngine {
    pub fn new(text: impl Into<String>, language: Option<impl Into<String>>) -> Self {
        Self {
            text: text.into(),
            language: language.map(Into::into),
            compute: None,
        }
    }

    /// Reports `compute` as its snapshot.
    pub fn with_compute(mut self, compute: Compute) -> Self {
        self.compute = Some(compute);
        self
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

    fn compute(&self) -> Option<Compute> {
        self.compute.clone()
    }
}

#[cfg(feature = "engine")]
pub mod cpp {
    use super::*;
    use crate::Error;
    use crate::compute::{self, GpuAvailability, GpuClass, LoadRequest};
    use ::transcribe_cpp::{
        Backend, CancelToken as CppCancelToken, Model, ModelOptions, RunExtension, RunOptions,
        SessionOptions, WhisperRunOptions,
    };

    /// The backend a `gpu` request loads with: the bundle's GPU module.
    const GPU_BACKEND: Backend = Backend::Vulkan;

    pub struct TranscribeCppEngine {
        model: Model,
        threads: i32,
        compute: Compute,
    }

    impl TranscribeCppEngine {
        /// Registers the backends `request.backend` allows, loads the model
        /// as the policy in [`crate::compute`] says and takes the compute
        /// snapshot from the device the model actually loaded on. A failed
        /// load is never retried on another backend (p14.1-windows-vulkan/D4).
        pub fn new(request: EngineRequest<'_>) -> Result<Self, Error> {
            let mode = request.backend;
            // Native logs can contain dictated text; disable them before loading.
            transcribe_cpp::disable_logging();
            let gpu = init_backends(mode)?;
            let load = compute::load_request(mode, gpu)?;
            let options = match load {
                LoadRequest::Cpu => ModelOptions {
                    backend: Backend::Cpu,
                    device: None,
                },
                // Exactly the discrete GPU the policy picks: the engine's
                // own automatic choice would take any `gpu`-typed device,
                // software ones included, and fall back to the next.
                LoadRequest::Discrete => {
                    let (devices, classes): (Vec<_>, Vec<_>) =
                        registered_gpus().into_iter().unzip();
                    let device = compute::auto_device(&classes)
                        .map(|index| devices[index].clone())
                        .ok_or_else(|| {
                            Error::internal("backend auto lost the discrete GPU it registered")
                        })?;
                    ModelOptions {
                        backend: GPU_BACKEND,
                        device: Some(device),
                    }
                }
                LoadRequest::Gpu => ModelOptions {
                    backend: GPU_BACKEND,
                    device: None,
                },
            };
            let model = Model::load_with(request.model_path, &options).map_err(|error| {
                compute::load_failed(
                    mode,
                    load,
                    matches!(error, transcribe_cpp::Error::Backend(_)),
                )
            })?;
            let device = model.device().map_err(|_| {
                Error::new(
                    crate::ErrorCode::ModelLoadFailed,
                    "the engine did not report the device the model loaded on",
                )
            })?;
            let compute = Compute::from_loaded(
                mode,
                gpu,
                &device.kind,
                &device.name,
                &device.description,
                loaded_cpu_variant().as_deref(),
            )?;
            Ok(Self {
                model,
                threads: request.threads,
                compute,
            })
        }
    }

    /// The registered GPU devices of this build's GPU backend, in registry
    /// order, with the class the backend policy gives each.
    fn registered_gpus() -> Vec<(transcribe_cpp::Device, GpuClass)> {
        transcribe_cpp::devices()
            .into_iter()
            .filter(|device| device.kind == "vulkan")
            .map(|device| {
                let class = GpuClass::of(&device_type(&device), device.device_id.as_deref());
                (device, class)
            })
            .collect()
    }

    /// The engine's device type as `backends --json` reports it: `cpu`,
    /// `gpu`, `igpu`, `accel` or `unknown`.
    fn device_type(device: &transcribe_cpp::Device) -> String {
        format!("{:?}", device.device_type).to_lowercase()
    }

    /// Registers the compute backends `mode` allows and reports what the GPU
    /// side found. Builds without runtime modules have the CPU compiled in
    /// and no GPU.
    fn init_backends(mode: BackendMode) -> Result<GpuAvailability, Error> {
        #[cfg(pumice_dynamic_backends)]
        {
            dynamic_backends::init_from_exe_dir(mode.wants_gpu())
                .map(|selection| selection.gpu_availability())
                .map_err(|error| {
                    eprintln!("cpu-variants: {error}");
                    Error::engine_failed()
                })
        }
        #[cfg(not(pumice_dynamic_backends))]
        {
            let _ = mode;
            Ok(GpuAvailability::NotBuilt)
        }
    }

    impl Engine for TranscribeCppEngine {
        fn compute(&self) -> Option<Compute> {
            Some(self.compute.clone())
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
                .map_err(|error| run_error(&error))?;

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

            // A failure that may leave the engine unusable is reported even
            // when the run was also cancelled, so it still retires a GPU.
            let run = run.map_err(|error| run_error(&error));
            if cancel.is_cancelled() && run.as_ref().err() != Some(&EngineError::Failed) {
                return Err(EngineError::Cancelled);
            }
            let result = run?;

            Ok(Transcript {
                text: result.text,
                language: result.language,
            })
        }
    }

    /// Sorts a native error per p14.2-backend-policy/D1: a request the engine
    /// refused for its content or input leaves the engine usable; every other
    /// failure, including kinds a later transcribe-cpp adds, may not.
    fn run_error(error: &transcribe_cpp::Error) -> EngineError {
        use transcribe_cpp::Error as Native;
        match error {
            Native::InvalidArgument(_)
            | Native::NotImplemented(_)
            | Native::Unsupported(_)
            | Native::InputTooLong(_)
            | Native::Aborted { .. }
            | Native::OutputTruncated { .. }
            | Native::OutputRepetition { .. }
            | Native::Nul(_)
            | Native::Busy(_) => EngineError::Rejected,
            Native::ModelFileNotFound(_)
            | Native::ModelLoad(_)
            | Native::OutOfMemory(_)
            | Native::Backend(_)
            | Native::BadStructSize(_)
            | Native::VersionMismatch(_)
            | Native::Other(_) => EngineError::Failed,
            // `transcribe_cpp::Error` is non-exhaustive. A kind added by a
            // later version is not known to be safe to reuse after, so it
            // retires a GPU process until it is classified here.
            _ => EngineError::Failed,
        }
    }

    #[cfg(test)]
    mod run_error_tests {
        use super::*;
        use transcribe_cpp::Error as Native;

        #[test]
        fn content_and_input_errors_leave_the_engine_usable() {
            let nul = std::ffi::CString::new("a\0b").unwrap_err();
            for error in [
                Native::InvalidArgument("x".into()),
                Native::NotImplemented("x".into()),
                Native::Unsupported("x".into()),
                Native::InputTooLong("x".into()),
                Native::Aborted {
                    message: "x".into(),
                    partial: None,
                },
                Native::OutputTruncated {
                    message: "x".into(),
                    partial: None,
                },
                Native::OutputRepetition {
                    message: "x".into(),
                    partial: None,
                },
                Native::Nul(nul),
                Native::Busy("x".into()),
            ] {
                assert_eq!(run_error(&error), EngineError::Rejected, "{error:?}");
            }
        }

        #[test]
        fn other_native_failures_make_the_engine_unusable() {
            for error in [
                Native::ModelFileNotFound("x".into()),
                Native::ModelLoad("x".into()),
                Native::OutOfMemory("x".into()),
                Native::Backend("x".into()),
                Native::BadStructSize("x".into()),
                Native::VersionMismatch("x".into()),
                Native::Other("x".into()),
            ] {
                assert_eq!(run_error(&error), EngineError::Failed, "{error:?}");
            }
        }
    }

    /// The loaded ggml CPU module's variant name, e.g. `haswell`; `None` in
    /// builds without runtime CPU modules.
    fn loaded_cpu_variant() -> Option<String> {
        #[cfg(target_os = "linux")]
        if let Some(variant) = selected_cpu_variant() {
            return Some(variant);
        }
        // No /proc/self/maps here: report the module startup initialized
        // from. Its directory holds that one CPU module only, and
        // initialization fails unless it registered.
        #[cfg(all(pumice_dynamic_backends, windows))]
        if let Some(selection) = dynamic_backends::selection() {
            return Some(selection.variant.to_string());
        }
        None
    }

    /// `pumice-transcribe backends`: registers the compute backends exactly
    /// as a model load in `mode` does, without a model, and lists what the
    /// bundle ships and what registered. Fixed field names plus bounded
    /// hardware names only.
    pub fn backends_report(mode: BackendMode) -> Result<serde_json::Value, Error> {
        transcribe_cpp::disable_logging();
        let gpu = init_backends(mode)?;
        #[cfg(pumice_dynamic_backends)]
        let bundle = dynamic_backends::selection().map(|selection| selection.bundle.as_str());
        #[cfg(not(pumice_dynamic_backends))]
        let bundle: Option<&str> = None;

        let allowed = transcribe_cpp::allowed_backends();
        let allowed_backends: Vec<&str> = [
            ("cpu", transcribe_cpp::BackendMask::CPU),
            ("metal", transcribe_cpp::BackendMask::METAL),
            ("vulkan", transcribe_cpp::BackendMask::VULKAN),
            ("cuda", transcribe_cpp::BackendMask::CUDA),
            ("rocm", transcribe_cpp::BackendMask::ROCM),
            ("other", transcribe_cpp::BackendMask::OTHER),
        ]
        .into_iter()
        .filter(|(_, bit)| allowed.contains(*bit))
        .map(|(name, _)| name)
        .collect();
        let devices: Vec<serde_json::Value> = transcribe_cpp::devices()
            .into_iter()
            .map(|device| {
                let device_type = device_type(&device);
                let gpu_class = (device.kind == "vulkan")
                    .then(|| GpuClass::of(&device_type, device.device_id.as_deref()));
                serde_json::json!({
                    "name": compute::sanitize(&device.name, compute::MAX_LABEL_BYTES),
                    "description": compute::sanitize(
                        &device.description,
                        compute::MAX_DESCRIPTION_BYTES,
                    ),
                    "kind": compute::sanitize(&device.kind, compute::MAX_LABEL_BYTES),
                    "type": device_type,
                    "gpu_class": gpu_class,
                })
            })
            .collect();
        Ok(serde_json::json!({
            "requested": mode,
            "gpu_build": cfg!(pumice_vulkan).then_some("vulkan"),
            "bundle": bundle,
            "cpu_variant": loaded_cpu_variant(),
            "gpu_module": gpu.module_state(),
            "gpu_devices": gpu.gpu_devices(),
            "allowed_backends": allowed_backends,
            "devices": devices,
            "fallback_reason": compute::planned_fallback(mode, gpu),
        }))
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
    /// Linux and x86-64 Windows). The variant is chosen here, not by ggml: ggml's pinned
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
    ///
    /// With the `vulkan` build (x86-64 Windows and x86-64 Linux with glibc,
    /// experimental) the optional GPU module sits alone in
    /// `<exe dir>/gpu/vulkan/` and is scanned after the CPU module, with the
    /// same backend mask (D2 of p14.1-windows-vulkan). It is scanned only
    /// when the backend mode allows a GPU (`auto`, `gpu`); in `cpu` mode it
    /// is neither checked nor loaded.
    #[cfg(pumice_dynamic_backends)]
    mod dynamic_backends {
        use crate::bundle::{BundleError, Expected, Manifest};
        use crate::compute::{GpuAvailability, GpuDevices};
        use ::transcribe_cpp::BackendMask;
        use std::fmt;
        use std::path::{Path, PathBuf};
        use std::sync::OnceLock;
        use std::time::Instant;

        /// One x86 ISA feature a ggml CPU variant can require, mirroring the
        /// GGML_* compile definitions in ggml's pinned CMake variant table
        /// (transcribe-cpp-sys ggml/src/CMakeLists.txt,
        /// ggml_add_cpu_backend_variant). `Avx512` stands for the five base
        /// flags of ggml's AVX512 tier (-mavx512f/cd/vl/dq/bw). `Sse3` to
        /// `Lzcnt` are never in ggml's lists; they are the extra features an
        /// MSVC `/arch` switch lets the compiler emit (`msvc_implied`).
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        enum CpuFeature {
            Sse3,
            Ssse3,
            Sse41,
            Popcnt,
            Bmi1,
            Lzcnt,
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

        /// The variants an MSVC x86-64 build produces: the same pinned CMake
        /// table minus the entries it skips under MSVC — ivybridge and
        /// piledriver (MSVC defines no `__F16C__`/`__FMA__`), cooperlake and
        /// zen4 (no AVX512-BF16 intrinsics without `/arch:AVX10.1`) and
        /// sapphirerapids (no AMX). Feature lists come from `VARIANTS`, so
        /// both tables stay one source.
        const MSVC_BUILT: &[&str] = &[
            "x64",
            "sse42",
            "sandybridge",
            "haswell",
            "skylakex",
            "cannonlake",
            "cascadelake",
            "icelake",
            "alderlake",
        ];

        /// Which entries of `VARIANTS` a build produces, and what else its
        /// compiler may emit for each of them.
        struct VariantTable {
            /// `None`: every entry.
            built: Option<&'static [&'static str]>,
            /// Features required beyond a variant's ggml list.
            implied: fn(&[CpuFeature]) -> &'static [CpuFeature],
        }

        const GNU_TABLE: VariantTable = VariantTable {
            built: None,
            implied: no_implied,
        };

        const MSVC_TABLE: VariantTable = VariantTable {
            built: Some(MSVC_BUILT),
            implied: msvc_implied,
        };

        /// The table of the modules this executable was built with.
        const TABLE: &VariantTable = if cfg!(target_env = "msvc") {
            &MSVC_TABLE
        } else {
            &GNU_TABLE
        };

        /// GCC/Clang only emit what ggml's `-m` flags list.
        fn no_implied(_: &[CpuFeature]) -> &'static [CpuFeature] {
            &[]
        }

        /// What MSVC's `/arch` switches may emit beyond ggml's list. ggml
        /// compiles its SSE4.2 tier with `/arch:SSE4.2` ("the full set of SSE
        /// instructions"), the AVX tier with `/arch:AVX`, and the AVX2 and
        /// AVX-512 tiers with `/arch:AVX2`/`/arch:AVX512`; Microsoft
        /// documents that each switch "may also enable the use of other
        /// nonvector instructions", naming BMI for `/arch:AVX2`, and MSVC's
        /// STL skips its runtime ISA check for POPCNT and LZCNT/TZCNT under
        /// AVX/AVX2. Real CPUs at those tiers all have these, so requiring
        /// them costs nothing there and keeps a guest that masks one of them
        /// on a lower tier.
        fn msvc_implied(required: &[CpuFeature]) -> &'static [CpuFeature] {
            use CpuFeature::*;
            if required.contains(&Avx2) {
                &[Sse3, Ssse3, Sse41, Popcnt, Bmi1, Lzcnt]
            } else if required.contains(&Sse42) {
                &[Sse3, Ssse3, Sse41, Popcnt]
            } else {
                &[]
            }
        }

        /// Variants whose features are all usable on this host, weakest
        /// first, in the preference order `choose_variant` uses: feature
        /// count ascending, table order on ties (the later entry wins).
        /// Implied features gate compatibility but not the ranking, so both
        /// tables rank shared variants alike. `x64` has no requirements, so
        /// the list is never empty.
        fn compatible_variants(
            table: &VariantTable,
            detect: impl Fn(CpuFeature) -> bool,
        ) -> Vec<(&'static str, &'static [CpuFeature])> {
            let mut variants: Vec<_> = VARIANTS
                .iter()
                .filter(|(name, _)| table.built.is_none_or(|built| built.contains(name)))
                .filter(|(_, required)| {
                    required
                        .iter()
                        .chain((table.implied)(required))
                        .all(|&f| detect(f))
                })
                .copied()
                .collect();
            variants.sort_by_key(|(_, required)| required.len());
            variants
        }

        /// The best variant whose features are all usable on this host, or
        /// the `x64` portable floor. The detector is injected so the choice
        /// is unit-testable independent of the host.
        fn choose_variant(
            table: &VariantTable,
            detect: impl Fn(CpuFeature) -> bool,
        ) -> &'static str {
            compatible_variants(table, detect)
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
                Sse3 => std::arch::is_x86_feature_detected!("sse3"),
                Ssse3 => std::arch::is_x86_feature_detected!("ssse3"),
                Sse41 => std::arch::is_x86_feature_detected!("sse4.1"),
                Popcnt => std::arch::is_x86_feature_detected!("popcnt"),
                Bmi1 => std::arch::is_x86_feature_detected!("bmi1"),
                Lzcnt => std::arch::is_x86_feature_detected!("lzcnt"),
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

        /// The rebuild hint for this build's staging script.
        const STAGER_HINT: &str = if cfg!(windows) {
            "rebuild the bundle with crates/transcription/scripts/build-windows-vulkan.ps1"
        } else if cfg!(pumice_vulkan) {
            "rebuild the bundle with crates/transcription/scripts/build-linux-vulkan.sh"
        } else {
            "rebuild the staged folder with crates/transcription/scripts/build-cpu-variants.sh"
        };

        /// Why backend initialization failed, in fixed text safe to show on
        /// stderr: paths and variant names only, never native error strings.
        /// Each variant names its category (modules missing / no usable CPU
        /// module / GPU module rejected / bundle manifest mismatch) and
        /// carries the same recovery hint.
        #[derive(Debug)]
        pub enum InitError {
            ExeDir,
            /// The staged files do not match `bundle.json`, or a target that
            /// requires it has none: an installation error, never a reason
            /// to fall back.
            Bundle {
                exe_dir: PathBuf,
                error: BundleError,
            },
            ModulesMissing {
                exe_dir: PathBuf,
                variant: &'static str,
            },
            NoUsableCpuModule {
                variant: &'static str,
                module_dir: PathBuf,
            },
            GpuModuleRejected {
                module_dir: PathBuf,
            },
        }

        impl fmt::Display for InitError {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let hint = STAGER_HINT;
                match self {
                    InitError::ExeDir => {
                        write!(f, "cannot locate the executable directory; {hint}")
                    }
                    InitError::Bundle { exe_dir, error } => write!(
                        f,
                        "the bundle at {} does not match its manifest: {error} (bundle manifest \
                         mismatch); the installation is damaged or mixes files from different \
                         builds; {hint}",
                        exe_dir.display()
                    ),
                    InitError::ModulesMissing { exe_dir, variant } => write!(
                        f,
                        "no staged ggml CPU module for {} or any lower \
                         variant under {} (modules missing); the staged build is incomplete; \
                         {hint}",
                        module_file_name(variant),
                        exe_dir.join("cpu").display()
                    ),
                    InitError::NoUsableCpuModule {
                        variant,
                        module_dir,
                    } => write!(
                        f,
                        "ggml could not load the selected CPU module {} \
                         from {} (no usable CPU module); the staged libraries may not match \
                         this executable; {hint}",
                        module_file_name(variant),
                        module_dir.display()
                    ),
                    InitError::GpuModuleRejected { module_dir } => write!(
                        f,
                        "ggml rejected the GPU module directory {} (GPU module rejected); \
                         {hint}",
                        module_dir.display()
                    ),
                }
            }
        }

        /// The file name ggml's loader matches for one CPU variant:
        /// `libggml-cpu-<variant>.so` on Linux, `ggml-cpu-<variant>.dll` on
        /// Windows.
        fn module_file_name(variant: &str) -> String {
            if cfg!(windows) {
                format!("ggml-cpu-{variant}.dll")
            } else {
                format!("libggml-cpu-{variant}.so")
            }
        }

        /// The staged module file for one variant:
        /// `<exe dir>/cpu/<variant>/<module file name>`.
        fn variant_module_file(exe_dir: &Path, variant: &str) -> PathBuf {
            exe_dir
                .join("cpu")
                .join(variant)
                .join(module_file_name(variant))
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
            table: &VariantTable,
            chosen: &'static str,
            detect: impl Fn(CpuFeature) -> bool,
        ) -> Option<(&'static str, PathBuf)> {
            let candidates = compatible_variants(table, detect);
            let chosen_idx = candidates.iter().position(|(name, _)| *name == chosen)?;
            candidates[..=chosen_idx]
                .iter()
                .rev()
                .find(|(name, _)| variant_module_file(exe_dir, name).is_file())
                .map(|(name, _)| (*name, exe_dir.join("cpu").join(name)))
        }

        /// What startup did with the optional GPU module directory.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum GpuDir {
            /// This build has no GPU module (`cpu-variants` without
            /// `vulkan`, or a target without one).
            NotBuilt,
            /// `cpu` mode: not looked at, not checked, not loaded.
            NotRequested,
            /// No `gpu/vulkan/` module is staged next to the executable.
            Absent,
            /// Scanned with the process's backend mask.
            Scanned,
        }

        /// The staged Vulkan module: `<exe dir>/gpu/vulkan/ggml-vulkan.dll`
        /// on Windows, `<exe dir>/gpu/vulkan/libggml-vulkan.so` on Linux.
        fn gpu_module_file(exe_dir: &Path) -> PathBuf {
            let file = if cfg!(windows) {
                "ggml-vulkan.dll"
            } else {
                "libggml-vulkan.so"
            };
            exe_dir.join("gpu").join("vulkan").join(file)
        }

        /// The GPU module directory, relative to the executable, as
        /// `bundle.json` and the status lines name it.
        const GPU_DIR: &str = "gpu/vulkan";

        /// The GPU module directory to scan, or why none is scanned. In
        /// `cpu` mode (`wanted` false) the directory is not even looked at.
        fn plan_gpu_dir(exe_dir: &Path, built: bool, wanted: bool) -> Result<PathBuf, GpuDir> {
            if !built {
                return Err(GpuDir::NotBuilt);
            }
            if !wanted {
                return Err(GpuDir::NotRequested);
            }
            let module = gpu_module_file(exe_dir);
            if !module.is_file() {
                return Err(GpuDir::Absent);
            }
            Ok(exe_dir.join("gpu").join("vulkan"))
        }

        /// The one backend mask of this process. ggml fixes it at the first
        /// registration and rejects a wider one later, so the CPU directory
        /// is already loaded with the GPU bit when a GPU directory follows.
        /// Never `ALL`: its OTHER bit would also admit a `GGML_BACKEND_PATH`
        /// plugin.
        fn backend_mask(scan_gpu: bool) -> BackendMask {
            if scan_gpu {
                BackendMask::CPU | BackendMask::VULKAN
            } else {
                BackendMask::CPU
            }
        }

        /// Whether this build's bundles must carry a `bundle.json`: every
        /// Windows bundle and every build with a GPU module, whose stagers
        /// always write one. The plain Linux `cpu-variants` stage has none
        /// (`build-cpu-variants.sh` writes no manifest), so there it is
        /// checked only when present and existing staged folders keep working.
        const MANIFEST_REQUIRED: bool = cfg!(windows) || cfg!(pumice_vulkan);

        /// What startup checked against `bundle.json`.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum BundleCheck {
            /// No manifest, on a target that does not require one.
            Absent,
            /// Every library startup relies on matched it.
            Verified,
        }

        impl BundleCheck {
            pub fn as_str(self) -> &'static str {
                match self {
                    BundleCheck::Absent => "absent",
                    BundleCheck::Verified => "verified",
                }
            }
        }

        /// What startup loaded.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct Selection {
            pub variant: &'static str,
            pub bundle: BundleCheck,
            pub gpu: GpuDir,
            /// Vulkan devices registered after scanning the GPU module, by
            /// class; `None` when it was not scanned. None registered: the
            /// module could not load (no Vulkan loader) or found no usable
            /// device.
            pub vulkan_devices: Option<GpuDevices>,
        }

        impl Selection {
            /// What the backend policy needs to know about the GPU.
            pub fn gpu_availability(&self) -> GpuAvailability {
                match self.gpu {
                    GpuDir::NotBuilt => GpuAvailability::NotBuilt,
                    GpuDir::NotRequested => GpuAvailability::NotRequested,
                    GpuDir::Absent => GpuAvailability::ModuleAbsent,
                    GpuDir::Scanned => {
                        GpuAvailability::Devices(self.vulkan_devices.unwrap_or_default())
                    }
                }
            }
        }

        static SELECTION: OnceLock<Selection> = OnceLock::new();

        /// The selection of the last successful initialization, if any.
        pub fn selection() -> Option<Selection> {
            SELECTION.get().copied()
        }

        /// Initialize backends, returning a fixed-text reason on failure.
        /// `scan_gpu`: the backend mode allows a GPU, so the staged GPU
        /// module (if any) is checked and scanned. Runs once per process:
        /// later calls return the first success (ggml fixes the mask at the
        /// first registration and could not load anything new anyway, and
        /// the files are not hashed twice).
        ///
        /// With a `bundle.json`, everything is checked before the first
        /// registration: the core libraries, the selected CPU module's
        /// directory and, when it will be scanned, the GPU module's
        /// directory. A present module that does not match is an
        /// installation error, not a fallback; only an absent GPU module, or
        /// one that loads and finds no device, leaves the process on CPU.
        pub fn init_from_exe_dir(scan_gpu: bool) -> Result<Selection, InitError> {
            if let Some(selection) = SELECTION.get() {
                return Ok(*selection);
            }
            let exe_dir = std::env::current_exe()
                .ok()
                .and_then(|path| path.parent().map(Path::to_path_buf))
                .ok_or(InitError::ExeDir)?;
            let bundle_error = |error| InitError::Bundle {
                exe_dir: exe_dir.clone(),
                error,
            };
            let manifest =
                Manifest::load(&exe_dir, Expected::this_build()).map_err(bundle_error)?;
            if manifest.is_none() && MANIFEST_REQUIRED {
                return Err(bundle_error(BundleError::Missing));
            }
            let chosen = choose_variant(TABLE, detect);
            let (variant, module_dir) = select_module_dir(&exe_dir, TABLE, chosen, detect)
                .ok_or_else(|| InitError::ModulesMissing {
                    exe_dir: exe_dir.clone(),
                    variant: chosen,
                })?;
            let gpu_plan = plan_gpu_dir(&exe_dir, cfg!(pumice_vulkan), scan_gpu);
            let bundle = match &manifest {
                Some(manifest) => {
                    let started = Instant::now();
                    let mut verified = manifest.verify_core(&exe_dir).map_err(bundle_error)?;
                    verified += manifest
                        .verify_dir(&exe_dir, &format!("cpu/{variant}"))
                        .map_err(bundle_error)?;
                    if gpu_plan.is_ok() {
                        verified += manifest
                            .verify_dir(&exe_dir, GPU_DIR)
                            .map_err(bundle_error)?;
                    }
                    eprintln!(
                        "cpu-variants: {} verified: {} files, {} bytes in {} ms",
                        crate::bundle::FILE_NAME,
                        verified.files,
                        verified.bytes,
                        started.elapsed().as_millis()
                    );
                    BundleCheck::Verified
                }
                None => BundleCheck::Absent,
            };
            let mask = backend_mask(gpu_plan.is_ok());
            ::transcribe_cpp::init_backends_with(Some(&module_dir), mask).map_err(|_| {
                InitError::NoUsableCpuModule {
                    variant,
                    module_dir: module_dir.clone(),
                }
            })?;
            if variant == chosen {
                eprintln!(
                    "cpu-variants: selected ggml CPU module {}",
                    module_file_name(variant)
                );
            } else {
                eprintln!(
                    "cpu-variants: selected ggml CPU module {} (fallback: {} is not staged)",
                    module_file_name(variant),
                    module_file_name(chosen)
                );
            }
            let (gpu, vulkan_devices) = match gpu_plan {
                Ok(gpu_dir) => {
                    // The CPU device is already registered, so this only
                    // fails on a mask or path problem; a Vulkan module that
                    // cannot load or finds no device is skipped by ggml.
                    ::transcribe_cpp::init_backends_with(Some(&gpu_dir), mask).map_err(|_| {
                        InitError::GpuModuleRejected {
                            module_dir: gpu_dir.clone(),
                        }
                    })?;
                    let classes: Vec<_> = super::registered_gpus()
                        .into_iter()
                        .map(|(_, class)| class)
                        .collect();
                    let vulkan_devices = GpuDevices::count(&classes);
                    if vulkan_devices.total() == 0 {
                        eprintln!(
                            "cpu-variants: scanned {GPU_DIR}: no Vulkan device registered (no \
                             Vulkan driver or device, or the module could not load); CPU only"
                        );
                    } else {
                        eprintln!(
                            "cpu-variants: scanned {GPU_DIR}: {} Vulkan device(s) registered \
                             ({} discrete, {} integrated, {} software)",
                            vulkan_devices.total(),
                            vulkan_devices.discrete,
                            vulkan_devices.integrated,
                            vulkan_devices.software
                        );
                    }
                    (GpuDir::Scanned, Some(vulkan_devices))
                }
                Err(GpuDir::NotRequested) => {
                    eprintln!("cpu-variants: {GPU_DIR} not scanned (backend cpu)");
                    (GpuDir::NotRequested, None)
                }
                Err(GpuDir::Absent) => {
                    eprintln!("cpu-variants: no GPU module staged; CPU only");
                    (GpuDir::Absent, None)
                }
                Err(state) => (state, None),
            };
            let selection = Selection {
                variant,
                bundle,
                gpu,
                vulkan_devices,
            };
            let _ = SELECTION.set(selection);
            Ok(selection)
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
                assert_eq!(choose_variant(&GNU_TABLE, detector(&[])), "x64");
            }

            #[test]
            fn sse42_only_cpu_gets_sse42() {
                assert_eq!(choose_variant(&GNU_TABLE, detector(&[Sse42])), "sse42");
            }

            #[test]
            fn avx_tiers_upgrade_stepwise() {
                let sandybridge = [Sse42, Avx];
                assert_eq!(
                    choose_variant(&GNU_TABLE, detector(&sandybridge)),
                    "sandybridge"
                );
                let ivybridge = [Sse42, Avx, F16c];
                assert_eq!(
                    choose_variant(&GNU_TABLE, detector(&ivybridge)),
                    "ivybridge"
                );
                let piledriver = [Sse42, Avx, F16c, Fma];
                assert_eq!(
                    choose_variant(&GNU_TABLE, detector(&piledriver)),
                    "piledriver"
                );
            }

            #[test]
            fn haswell_set_chooses_haswell() {
                let haswell = [Sse42, Avx, F16c, Fma, Avx2, Bmi2];
                assert_eq!(choose_variant(&GNU_TABLE, detector(&haswell)), "haswell");
            }

            #[test]
            fn avx_vnni_upgrades_haswell_to_alderlake() {
                let alderlake = [Sse42, Avx, F16c, Fma, Avx2, Bmi2, AvxVnni];
                assert_eq!(
                    choose_variant(&GNU_TABLE, detector(&alderlake)),
                    "alderlake"
                );
            }

            #[test]
            fn os_enabled_avx512_reaches_the_avx512_tiers() {
                let base = [Sse42, Avx, F16c, Fma, Avx2, Bmi2];
                assert_eq!(
                    choose_variant(&GNU_TABLE, detector(&[base.as_slice(), &[Avx512]].concat())),
                    "skylakex"
                );
                assert_eq!(
                    choose_variant(
                        &GNU_TABLE,
                        detector(&[base.as_slice(), &[Avx512, Avx512Vbmi]].concat())
                    ),
                    "cannonlake"
                );
                assert_eq!(
                    choose_variant(
                        &GNU_TABLE,
                        detector(&[base.as_slice(), &[Avx512, Avx512Vnni]].concat())
                    ),
                    "cascadelake"
                );
                assert_eq!(
                    choose_variant(
                        &GNU_TABLE,
                        detector(&[base.as_slice(), &[Avx512, Avx512Vbmi, Avx512Vnni]].concat())
                    ),
                    "icelake"
                );
                assert_eq!(
                    choose_variant(
                        &GNU_TABLE,
                        detector(&[base.as_slice(), &[Avx512, Avx512Vnni, Avx512Bf16]].concat())
                    ),
                    "cooperlake"
                );
                assert_eq!(
                    choose_variant(
                        &GNU_TABLE,
                        detector(
                            &[
                                base.as_slice(),
                                &[Avx512, Avx512Vbmi, Avx512Vnni, Avx512Bf16]
                            ]
                            .concat()
                        )
                    ),
                    "zen4"
                );
            }

            #[test]
            fn amx_completes_sapphirerapids() {
                let sapphire = [
                    Sse42, Avx, F16c, Fma, Avx2, Bmi2, Avx512, Avx512Vbmi, Avx512Vnni, Avx512Bf16,
                    AmxTile, AmxInt8,
                ];
                assert_eq!(
                    choose_variant(&GNU_TABLE, detector(&sapphire)),
                    "sapphirerapids"
                );
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
                assert_eq!(
                    choose_variant(&GNU_TABLE, detector(&avx512_os_disabled)),
                    "haswell"
                );
            }

            #[test]
            fn avx_os_disabled_keeps_out_vex_tiers() {
                // Same shape for plain AVX: with the OS AVX state disabled,
                // only the non-AVX tiers are selectable.
                let avx_os_disabled = [Sse42];
                assert_eq!(
                    choose_variant(&GNU_TABLE, detector(&avx_os_disabled)),
                    "sse42"
                );
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
                    (
                        InitError::GpuModuleRejected {
                            module_dir: temp.path().to_path_buf(),
                        },
                        "GPU module rejected",
                    ),
                    (
                        InitError::Bundle {
                            exe_dir: temp.path().to_path_buf(),
                            error: BundleError::Missing,
                        },
                        "bundle manifest mismatch",
                    ),
                ];
                for (error, category) in cases {
                    let message = error.to_string();
                    assert!(message.contains(category), "{message}");
                    assert!(message.contains(STAGER_HINT), "{message}");
                }
            }

            fn stage_module(exe_dir: &Path, variant: &str) {
                let dir = exe_dir.join("cpu").join(variant);
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join(module_file_name(variant)), b"").unwrap();
            }

            #[test]
            fn chosen_variant_directory_is_selected() {
                let temp = tempfile::tempdir().unwrap();
                stage_module(temp.path(), "x64");
                stage_module(temp.path(), "haswell");
                let (variant, dir) =
                    select_module_dir(temp.path(), &GNU_TABLE, "haswell", detector(HOST_FEATURES))
                        .unwrap();
                assert_eq!(variant, "haswell");
                assert_eq!(dir, temp.path().join("cpu").join("haswell"));
            }

            #[test]
            fn missing_chosen_variant_falls_back_to_a_lower_one() {
                let temp = tempfile::tempdir().unwrap();
                stage_module(temp.path(), "sse42");
                stage_module(temp.path(), "x64");
                let (variant, dir) =
                    select_module_dir(temp.path(), &GNU_TABLE, "haswell", detector(HOST_FEATURES))
                        .unwrap();
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
                let (variant, dir) = select_module_dir(
                    temp.path(),
                    &GNU_TABLE,
                    "alderlake",
                    detector(&alderlake_host),
                )
                .unwrap();
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
                let (variant, dir) = select_module_dir(
                    temp.path(),
                    &GNU_TABLE,
                    "cascadelake",
                    detector(&cascadelake_host),
                )
                .unwrap();
                assert_eq!(variant, "skylakex");
                assert_eq!(dir, temp.path().join("cpu").join("skylakex"));
            }

            #[test]
            fn only_x64_staged_still_runs() {
                let temp = tempfile::tempdir().unwrap();
                stage_module(temp.path(), "x64");
                let (variant, dir) = select_module_dir(
                    temp.path(),
                    &GNU_TABLE,
                    "alderlake",
                    detector(HOST_FEATURES),
                )
                .unwrap();
                assert_eq!(variant, "x64");
                assert_eq!(dir, temp.path().join("cpu").join("x64"));
            }

            #[test]
            fn empty_variant_directory_does_not_count() {
                let temp = tempfile::tempdir().unwrap();
                std::fs::create_dir_all(temp.path().join("cpu").join("haswell")).unwrap();
                stage_module(temp.path(), "x64");
                let (variant, _) =
                    select_module_dir(temp.path(), &GNU_TABLE, "haswell", detector(HOST_FEATURES))
                        .unwrap();
                assert_eq!(variant, "x64");
            }

            #[test]
            fn no_staged_variant_is_an_error() {
                let temp = tempfile::tempdir().unwrap();
                assert_eq!(
                    select_module_dir(temp.path(), &GNU_TABLE, "haswell", detector(HOST_FEATURES)),
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
                    select_module_dir(temp.path(), &GNU_TABLE, "haswell", detector(HOST_FEATURES)),
                    None
                );
            }

            #[cfg(unix)]
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
                let (variant, dir) = select_module_dir(
                    temp.path(),
                    &GNU_TABLE,
                    "alderlake",
                    detector(HOST_FEATURES),
                )
                .unwrap();
                assert_eq!(variant, "x64");
                assert_eq!(dir, x64);
                assert!(variant_module_file(temp.path(), "x64").is_file());
                for dir in [&cpu, &x64] {
                    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
                }
            }

            /// Everything MSVC's `/arch` switches may additionally emit.
            const MSVC_IMPLIED: &[CpuFeature] = &[Sse3, Ssse3, Sse41, Popcnt, Bmi1, Lzcnt];

            fn msvc_host(features: &[CpuFeature]) -> Vec<CpuFeature> {
                [features, MSVC_IMPLIED].concat()
            }

            #[test]
            fn msvc_table_lists_only_known_variants() {
                for name in MSVC_BUILT {
                    assert!(
                        VARIANTS.iter().any(|(known, _)| known == name),
                        "{name} is not in VARIANTS"
                    );
                }
                assert_eq!(MSVC_BUILT.len(), 9);
            }

            #[test]
            fn msvc_table_never_offers_variants_msvc_does_not_build() {
                let everything = msvc_host(&[
                    Sse42, Avx, F16c, Fma, Bmi2, Avx2, AvxVnni, Avx512, Avx512Vbmi, Avx512Vnni,
                    Avx512Bf16, AmxTile, AmxInt8,
                ]);
                let names: Vec<_> = compatible_variants(&MSVC_TABLE, detector(&everything))
                    .into_iter()
                    .map(|(name, _)| name)
                    .collect();
                for missing in [
                    "ivybridge",
                    "piledriver",
                    "cooperlake",
                    "zen4",
                    "sapphirerapids",
                ] {
                    assert!(!names.contains(&missing), "{missing} offered: {names:?}");
                }
                // Same ranking as the GNU table among shared variants: the
                // AVX-512 VBMI+VNNI tier beats alderlake.
                assert_eq!(
                    choose_variant(&MSVC_TABLE, detector(&everything)),
                    "icelake"
                );
            }

            #[test]
            fn msvc_tiers_match_their_gnu_counterparts_on_real_cpus() {
                let cases: [(&[CpuFeature], &str); 6] = [
                    (&[], "x64"),
                    (&[Sse42], "sse42"),
                    (&[Sse42, Avx, F16c, Fma], "sandybridge"),
                    (&[Sse42, Avx, F16c, Fma, Avx2, Bmi2], "haswell"),
                    (&[Sse42, Avx, F16c, Fma, Avx2, Bmi2, AvxVnni], "alderlake"),
                    (
                        &[Sse42, Avx, F16c, Fma, Avx2, Bmi2, Avx512, Avx512Vnni],
                        "cascadelake",
                    ),
                ];
                for (features, expected) in cases {
                    let host = msvc_host(features);
                    assert_eq!(
                        choose_variant(&MSVC_TABLE, detector(&host)),
                        expected,
                        "{features:?}"
                    );
                }
            }

            #[test]
            fn msvc_avx2_tiers_need_the_bit_manipulation_features_arch_avx2_implies() {
                // A guest that masks LZCNT (or BMI1) while exposing AVX2:
                // /arch:AVX2 code may use them, so stay on the AVX tier.
                let base = [Sse42, Avx, F16c, Fma, Avx2, Bmi2, AvxVnni];
                for masked in [Lzcnt, Bmi1] {
                    let host: Vec<_> = msvc_host(&base)
                        .into_iter()
                        .filter(|&f| f != masked)
                        .collect();
                    assert_eq!(
                        choose_variant(&MSVC_TABLE, detector(&host)),
                        "sandybridge",
                        "{masked:?} masked"
                    );
                }
            }

            #[test]
            fn msvc_sse_tiers_need_the_full_sse_set_and_popcnt() {
                for masked in [Sse3, Ssse3, Sse41, Popcnt] {
                    let host: Vec<_> = msvc_host(&[Sse42, Avx])
                        .into_iter()
                        .filter(|&f| f != masked)
                        .collect();
                    assert_eq!(
                        choose_variant(&MSVC_TABLE, detector(&host)),
                        "x64",
                        "{masked:?} masked"
                    );
                }
            }

            #[test]
            fn gnu_table_ignores_the_msvc_implied_features() {
                // Linux behavior is unchanged: no implied feature is checked.
                let haswell = [Sse42, Avx, F16c, Fma, Avx2, Bmi2];
                assert_eq!(choose_variant(&GNU_TABLE, detector(&haswell)), "haswell");
            }

            #[test]
            fn msvc_fallback_skips_variants_msvc_does_not_build() {
                // A stray ivybridge module in an MSVC bundle is never used.
                let host = msvc_host(&[Sse42, Avx, F16c, Fma]);
                let temp = tempfile::tempdir().unwrap();
                stage_module(temp.path(), "ivybridge");
                stage_module(temp.path(), "x64");
                let chosen = choose_variant(&MSVC_TABLE, detector(&host));
                assert_eq!(chosen, "sandybridge");
                let (variant, _) =
                    select_module_dir(temp.path(), &MSVC_TABLE, chosen, detector(&host)).unwrap();
                assert_eq!(variant, "x64");
            }

            fn stage_gpu_module(exe_dir: &Path) {
                let file = gpu_module_file(exe_dir);
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                std::fs::write(file, b"").unwrap();
            }

            #[test]
            fn gpu_dir_is_scanned_only_when_built_wanted_and_staged() {
                let temp = tempfile::tempdir().unwrap();
                assert_eq!(
                    plan_gpu_dir(temp.path(), false, true),
                    Err(GpuDir::NotBuilt)
                );
                assert_eq!(plan_gpu_dir(temp.path(), true, true), Err(GpuDir::Absent));
                // An empty gpu/vulkan/ directory does not count either.
                std::fs::create_dir_all(temp.path().join("gpu").join("vulkan")).unwrap();
                assert_eq!(plan_gpu_dir(temp.path(), true, true), Err(GpuDir::Absent));
                stage_gpu_module(temp.path());
                assert_eq!(
                    plan_gpu_dir(temp.path(), true, true),
                    Ok(temp.path().join("gpu").join("vulkan"))
                );
                // backend cpu: a staged module is never planned for a scan.
                assert_eq!(
                    plan_gpu_dir(temp.path(), true, false),
                    Err(GpuDir::NotRequested)
                );
                assert_eq!(
                    plan_gpu_dir(temp.path(), false, true),
                    Err(GpuDir::NotBuilt)
                );
            }

            #[cfg(unix)]
            #[test]
            fn cpu_mode_never_looks_at_the_gpu_dir() {
                use std::os::unix::fs::PermissionsExt;
                // An unreadable gpu/ directory: looking inside would fail,
                // so a cpu-mode plan proves it never looks.
                let temp = tempfile::tempdir().unwrap();
                stage_gpu_module(temp.path());
                let gpu = temp.path().join("gpu");
                std::fs::set_permissions(&gpu, std::fs::Permissions::from_mode(0o000)).unwrap();
                let planned = plan_gpu_dir(temp.path(), true, false);
                std::fs::set_permissions(&gpu, std::fs::Permissions::from_mode(0o755)).unwrap();
                assert_eq!(planned, Err(GpuDir::NotRequested));
            }

            #[test]
            fn selections_map_to_the_policy_availability() {
                let selection = |gpu, vulkan_devices| Selection {
                    variant: "x64",
                    bundle: BundleCheck::Absent,
                    gpu,
                    vulkan_devices,
                };
                assert_eq!(
                    selection(GpuDir::NotBuilt, None).gpu_availability(),
                    GpuAvailability::NotBuilt
                );
                assert_eq!(
                    selection(GpuDir::NotRequested, None).gpu_availability(),
                    GpuAvailability::NotRequested
                );
                assert_eq!(
                    selection(GpuDir::Absent, None).gpu_availability(),
                    GpuAvailability::ModuleAbsent
                );
                let found = GpuDevices {
                    discrete: 1,
                    integrated: 1,
                    software: 0,
                };
                assert_eq!(
                    selection(GpuDir::Scanned, Some(GpuDevices::default())).gpu_availability(),
                    GpuAvailability::Devices(GpuDevices::default())
                );
                assert_eq!(
                    selection(GpuDir::Scanned, Some(found)).gpu_availability(),
                    GpuAvailability::Devices(found)
                );
            }

            #[test]
            fn backend_mask_is_cpu_or_cpu_plus_vulkan_never_other() {
                assert_eq!(backend_mask(false), BackendMask::CPU);
                assert_eq!(backend_mask(true), BackendMask::CPU | BackendMask::VULKAN);
                for scan in [false, true] {
                    assert!(!backend_mask(scan).contains(BackendMask::OTHER));
                    assert_ne!(backend_mask(scan), BackendMask::ALL);
                }
            }
        }
    }
}
