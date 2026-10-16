//! The compute backend policy and the reported compute snapshot
//! (p14.1-windows-vulkan/D3–D5).
//!
//! `backend: cpu|auto|gpu` (`--backend` on `file`, `serve` and `backends`)
//! chooses what the model load may use:
//! - `cpu` never scans or loads a GPU module;
//! - `auto` uses a GPU when the bundle's GPU module registers one and it
//!   initializes (the engine tries discrete before integrated GPUs), else the
//!   CPU, with a [`FallbackReason`];
//! - `gpu` requires a GPU device and never falls back.
//!
//! After the model loads, one immutable [`Compute`] snapshot is built from the
//! device the engine actually loaded it on — not from the request — and every
//! report (`file --json`, `serve` startup, managed `ready` and `/ready`)
//! shows that snapshot. Its hardware strings are trimmed, free of control
//! characters and bounded, so a managed record always stays under its size
//! cap.
//!
//! The policy functions here are pure: the engine adapter feeds them what
//! registered and what loaded, so every outcome is testable without a GPU.

use crate::{Error, ErrorCode};
use serde::Serialize;

/// Longest device name (and CPU variant) reported, in UTF-8 bytes.
pub const MAX_LABEL_BYTES: usize = 128;
/// Longest device description reported, in UTF-8 bytes.
pub const MAX_DESCRIPTION_BYTES: usize = 256;

/// The requested backend mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum BackendMode {
    /// The CPU only; the GPU module is never scanned or loaded.
    #[default]
    Cpu,
    /// A GPU when one initializes, else the CPU.
    Auto,
    /// A GPU, or a startup error.
    Gpu,
}

impl BackendMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Auto => "auto",
            Self::Gpu => "gpu",
        }
    }

    /// The exact configuration spelling.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "cpu" => Some(Self::Cpu),
            "auto" => Some(Self::Auto),
            "gpu" => Some(Self::Gpu),
            _ => None,
        }
    }

    /// Whether this mode may scan the GPU module at all.
    pub const fn wants_gpu(self) -> bool {
        !matches!(self, Self::Cpu)
    }
}

/// What backend registration found for the GPU, before the model loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuAvailability {
    /// This build has no GPU module (the default `engine` build, the Linux
    /// `cpu-variants` build, macOS).
    NotBuilt,
    /// `cpu` mode: the GPU module was not looked at.
    NotRequested,
    /// The build supports a GPU module, but none is staged next to the
    /// executable.
    ModuleAbsent,
    /// The GPU module was scanned and registered this many GPU devices (0:
    /// no loader, no driver or no usable device; the public API cannot tell
    /// which).
    Devices(usize),
}

impl GpuAvailability {
    /// The `gpu_module` value `backends --json` reports.
    pub const fn module_state(self) -> &'static str {
        match self {
            Self::NotBuilt => "not_built",
            Self::NotRequested => "not_requested",
            Self::ModuleAbsent => "absent",
            Self::Devices(_) => "scanned",
        }
    }

    pub const fn gpu_devices(self) -> usize {
        match self {
            Self::Devices(count) => count,
            _ => 0,
        }
    }
}

/// What the model load asks the engine for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadRequest {
    /// The CPU device only.
    Cpu,
    /// The engine's automatic choice: discrete GPU, integrated GPU, CPU.
    Auto,
    /// The build's GPU backend, without fallback.
    Gpu,
}

/// Why `auto` runs on the CPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackReason {
    /// This build has no GPU module.
    GpuNotBuilt,
    /// The GPU module is not staged next to the executable.
    GpuModuleAbsent,
    /// The GPU module registered no device, or no GPU device initialized.
    GpuUnavailable,
}

impl FallbackReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GpuNotBuilt => "gpu_not_built",
            Self::GpuModuleAbsent => "gpu_module_absent",
            Self::GpuUnavailable => "gpu_unavailable",
        }
    }

    fn for_availability(gpu: GpuAvailability) -> Self {
        match gpu {
            GpuAvailability::NotBuilt => Self::GpuNotBuilt,
            GpuAvailability::ModuleAbsent => Self::GpuModuleAbsent,
            GpuAvailability::NotRequested | GpuAvailability::Devices(_) => Self::GpuUnavailable,
        }
    }
}

/// The kind of device the model runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ComputeBackend {
    Cpu,
    Vulkan,
}

impl ComputeBackend {
    /// From the engine's device kind; `None` for a kind this build never
    /// selects.
    pub fn from_kind(kind: &str) -> Option<Self> {
        match kind {
            "cpu" => Some(Self::Cpu),
            "vulkan" => Some(Self::Vulkan),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Vulkan => "vulkan",
        }
    }

    pub const fn is_gpu(self) -> bool {
        !matches!(self, Self::Cpu)
    }
}

/// The device the model runs on, as the engine names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComputeDevice {
    /// 1 to [`MAX_LABEL_BYTES`] bytes, e.g. `CPU` or `Vulkan0`.
    pub name: String,
    /// At most [`MAX_DESCRIPTION_BYTES`] bytes, e.g. the CPU or GPU model.
    pub description: String,
}

/// The immutable compute snapshot of a loaded model. Field order is the
/// documented JSON order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Compute {
    pub requested: BackendMode,
    pub backend: ComputeBackend,
    pub device: ComputeDevice,
    /// The loaded ggml CPU module (`cpu-variants` builds), else `None`. On a
    /// GPU it names the CPU helper, not the primary device.
    pub cpu_variant: Option<String>,
    /// Set only when `auto` runs on the CPU.
    pub fallback_reason: Option<FallbackReason>,
}

impl Compute {
    /// A snapshot with sanitized strings. The engine adapter uses
    /// [`Compute::from_loaded`]; this is for engines that know their device
    /// some other way (fakes).
    pub fn new(
        requested: BackendMode,
        backend: ComputeBackend,
        name: &str,
        description: &str,
        cpu_variant: Option<&str>,
        fallback_reason: Option<FallbackReason>,
    ) -> Self {
        let name = sanitize(name, MAX_LABEL_BYTES);
        Compute {
            requested,
            backend,
            device: ComputeDevice {
                name: if name.is_empty() {
                    "unknown".to_string()
                } else {
                    name
                },
                description: sanitize(description, MAX_DESCRIPTION_BYTES),
            },
            cpu_variant: cpu_variant
                .map(|variant| sanitize(variant, MAX_LABEL_BYTES))
                .filter(|variant| !variant.is_empty()),
            fallback_reason,
        }
    }

    /// The snapshot of a model the engine loaded on a device of `kind`.
    /// `requested`/`gpu` are the mode and what registration found. A `gpu`
    /// request that did not end up on a GPU is an error, never reported as
    /// a success.
    pub fn from_loaded(
        requested: BackendMode,
        gpu: GpuAvailability,
        kind: &str,
        name: &str,
        description: &str,
        cpu_variant: Option<&str>,
    ) -> Result<Self, Error> {
        let backend = ComputeBackend::from_kind(kind).ok_or_else(|| {
            Error::new(
                ErrorCode::ModelLoadFailed,
                "the model loaded on a device kind this build does not report",
            )
        })?;
        let fallback_reason = match (requested, backend) {
            (BackendMode::Gpu, ComputeBackend::Cpu) => {
                return Err(Error::gpu_unavailable(GPU_NO_DEVICE));
            }
            (BackendMode::Cpu, backend) if backend.is_gpu() => {
                return Err(Error::internal(
                    "backend cpu loaded the model on a GPU; refusing to report it",
                ));
            }
            (BackendMode::Auto, ComputeBackend::Cpu) => Some(FallbackReason::for_availability(gpu)),
            _ => None,
        };
        Ok(Self::new(
            requested,
            backend,
            name,
            description,
            cpu_variant,
            fallback_reason,
        ))
    }

    pub fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("a compute snapshot always serializes")
    }

    /// One line for humans, e.g. `vulkan: Vulkan0 (NVIDIA GeForce RTX 4070)
    /// [cpu: haswell], requested auto`.
    pub fn label(&self) -> String {
        let mut label = format!("{}: {}", self.backend.as_str(), self.device.name);
        if !self.device.description.is_empty() {
            label.push_str(&format!(" ({})", self.device.description));
        }
        if let Some(variant) = &self.cpu_variant {
            if self.backend.is_gpu() {
                label.push_str(&format!(" [cpu: {variant}]"));
            } else {
                label.push_str(&format!(" [{variant}]"));
            }
        }
        label.push_str(&format!(", requested {}", self.requested.as_str()));
        if let Some(reason) = self.fallback_reason {
            label.push_str(&format!(", fallback: {}", reason.as_str()));
        }
        label
    }
}

/// `gpu` mode in a build without a GPU module.
pub const GPU_NOT_BUILT: &str = "backend gpu needs a GPU build of pumice-transcribe; this build \
     runs on the CPU only. Use backend: cpu (or --backend cpu).";
/// `gpu` mode without a staged GPU module.
pub const GPU_MODULE_ABSENT: &str = "backend gpu found no GPU module (gpu/vulkan/) next to the \
     executable. Use backend: cpu (or --backend cpu), or install the complete bundle.";
/// `gpu` mode when no GPU device registered or initialized.
pub const GPU_NO_DEVICE: &str = "backend gpu found no usable GPU device (none registered or \
     none initialized). Use backend: cpu (or --backend cpu), or backend: auto to fall back to \
     the CPU.";
/// A load that may have allocated on a GPU failed. Not retried on the CPU.
pub const GPU_LOAD_FAILED: &str = "failed to load model with backend {mode}; a GPU load failure \
     is not retried on the CPU. Use backend: cpu (or --backend cpu) to run on the CPU.";

/// What the model load requests for `mode`, given what registered.
pub fn load_request(mode: BackendMode, gpu: GpuAvailability) -> Result<LoadRequest, Error> {
    match (mode, gpu) {
        (BackendMode::Cpu, _) => Ok(LoadRequest::Cpu),
        (BackendMode::Auto, GpuAvailability::Devices(count)) if count > 0 => Ok(LoadRequest::Auto),
        (BackendMode::Auto, _) => Ok(LoadRequest::Cpu),
        (BackendMode::Gpu, GpuAvailability::Devices(count)) if count > 0 => Ok(LoadRequest::Gpu),
        (BackendMode::Gpu, GpuAvailability::NotBuilt) => Err(Error::gpu_unavailable(GPU_NOT_BUILT)),
        (BackendMode::Gpu, GpuAvailability::ModuleAbsent) => {
            Err(Error::gpu_unavailable(GPU_MODULE_ABSENT))
        }
        (BackendMode::Gpu, _) => Err(Error::gpu_unavailable(GPU_NO_DEVICE)),
    }
}

/// Why `auto` will run on the CPU before anything loads, when registration
/// already decides it (no GPU built, staged or registered). A registered
/// GPU can still fail to initialize at load; only the loaded snapshot is
/// certain.
pub fn planned_fallback(mode: BackendMode, gpu: GpuAvailability) -> Option<FallbackReason> {
    match load_request(mode, gpu) {
        Ok(LoadRequest::Cpu) if mode == BackendMode::Auto => {
            Some(FallbackReason::for_availability(gpu))
        }
        _ => None,
    }
}

/// The error for a failed model load. `backend_error`: the engine reported
/// that it could not satisfy the requested backend (no device initialized).
pub fn load_failed(mode: BackendMode, request: LoadRequest, backend_error: bool) -> Error {
    match request {
        LoadRequest::Gpu if backend_error => Error::gpu_unavailable(GPU_NO_DEVICE),
        LoadRequest::Gpu | LoadRequest::Auto => Error::new(
            ErrorCode::ModelLoadFailed,
            GPU_LOAD_FAILED.replace("{mode}", mode.as_str()),
        ),
        LoadRequest::Cpu => Error::model_load_failed(),
    }
}

/// Trims, replaces control characters with spaces and cuts to `max` bytes on
/// a character boundary.
pub fn sanitize(text: &str, max: usize) -> String {
    let clean: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let clean = clean.trim();
    let mut end = clean.len().min(max);
    while !clean.is_char_boundary(end) {
        end -= 1;
    }
    clean[..end].trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_parse_exactly() {
        for mode in [BackendMode::Cpu, BackendMode::Auto, BackendMode::Gpu] {
            assert_eq!(BackendMode::parse(mode.as_str()), Some(mode));
        }
        for bad in ["CPU", " cpu", "vulkan", "", "gpu0"] {
            assert_eq!(BackendMode::parse(bad), None, "{bad}");
        }
        assert_eq!(BackendMode::default(), BackendMode::Cpu);
        assert!(!BackendMode::Cpu.wants_gpu());
        assert!(BackendMode::Auto.wants_gpu() && BackendMode::Gpu.wants_gpu());
    }

    #[test]
    fn cpu_mode_always_loads_on_the_cpu() {
        for gpu in [
            GpuAvailability::NotBuilt,
            GpuAvailability::NotRequested,
            GpuAvailability::ModuleAbsent,
            GpuAvailability::Devices(0),
            GpuAvailability::Devices(2),
        ] {
            assert_eq!(
                load_request(BackendMode::Cpu, gpu).unwrap(),
                LoadRequest::Cpu
            );
        }
    }

    #[test]
    fn auto_uses_the_engine_choice_only_with_a_gpu_device() {
        assert_eq!(
            load_request(BackendMode::Auto, GpuAvailability::Devices(1)).unwrap(),
            LoadRequest::Auto
        );
        for gpu in [
            GpuAvailability::NotBuilt,
            GpuAvailability::ModuleAbsent,
            GpuAvailability::Devices(0),
        ] {
            assert_eq!(
                load_request(BackendMode::Auto, gpu).unwrap(),
                LoadRequest::Cpu
            );
        }
    }

    #[test]
    fn gpu_mode_never_falls_back() {
        assert_eq!(
            load_request(BackendMode::Gpu, GpuAvailability::Devices(1)).unwrap(),
            LoadRequest::Gpu
        );
        for (gpu, message) in [
            (GpuAvailability::NotBuilt, GPU_NOT_BUILT),
            (GpuAvailability::ModuleAbsent, GPU_MODULE_ABSENT),
            (GpuAvailability::Devices(0), GPU_NO_DEVICE),
        ] {
            let error = load_request(BackendMode::Gpu, gpu).unwrap_err();
            assert_eq!(error.code, ErrorCode::GpuUnavailable);
            assert_eq!(error.message, message);
            assert!(error.message.contains("backend: cpu"), "{}", error.message);
        }
    }

    #[test]
    fn planned_fallbacks_only_for_auto_without_a_gpu() {
        assert_eq!(
            planned_fallback(BackendMode::Auto, GpuAvailability::ModuleAbsent),
            Some(FallbackReason::GpuModuleAbsent)
        );
        assert_eq!(
            planned_fallback(BackendMode::Auto, GpuAvailability::Devices(0)),
            Some(FallbackReason::GpuUnavailable)
        );
        assert_eq!(
            planned_fallback(BackendMode::Auto, GpuAvailability::Devices(1)),
            None
        );
        assert_eq!(
            planned_fallback(BackendMode::Cpu, GpuAvailability::NotRequested),
            None
        );
        assert_eq!(
            planned_fallback(BackendMode::Gpu, GpuAvailability::Devices(0)),
            None
        );
    }

    #[test]
    fn load_failures_on_a_gpu_point_to_cpu_and_are_not_retried() {
        let error = load_failed(BackendMode::Gpu, LoadRequest::Gpu, true);
        assert_eq!(error.code, ErrorCode::GpuUnavailable);
        for (mode, request) in [
            (BackendMode::Gpu, LoadRequest::Gpu),
            (BackendMode::Auto, LoadRequest::Auto),
        ] {
            let error = load_failed(mode, request, false);
            assert_eq!(error.code, ErrorCode::ModelLoadFailed);
            assert!(error.message.contains("backend: cpu"), "{}", error.message);
            assert!(error.message.contains(mode.as_str()), "{}", error.message);
            assert!(!error.message.contains("{mode}"));
        }
        // A CPU load (cpu mode, or auto without a GPU) keeps the plain error.
        let error = load_failed(BackendMode::Auto, LoadRequest::Cpu, false);
        assert_eq!(error.code, ErrorCode::ModelLoadFailed);
        assert_eq!(error.message, Error::model_load_failed().message);
    }

    #[test]
    fn the_snapshot_reports_the_loaded_device_not_the_request() {
        let compute = Compute::from_loaded(
            BackendMode::Auto,
            GpuAvailability::Devices(1),
            "vulkan",
            "Vulkan0",
            "NVIDIA GeForce RTX 4070",
            Some("haswell"),
        )
        .unwrap();
        assert_eq!(
            compute.to_value().to_string(),
            r#"{"requested":"auto","backend":"vulkan","device":{"name":"Vulkan0","description":"NVIDIA GeForce RTX 4070"},"cpu_variant":"haswell","fallback_reason":null}"#
        );
        // auto with a registered GPU that did not initialize: the engine
        // loaded on the CPU, and that is what is reported.
        let compute = Compute::from_loaded(
            BackendMode::Auto,
            GpuAvailability::Devices(1),
            "cpu",
            "CPU",
            "Intel(R) Core(TM) i5-14500T   ",
            Some("alderlake"),
        )
        .unwrap();
        assert_eq!(compute.backend, ComputeBackend::Cpu);
        assert_eq!(
            compute.fallback_reason,
            Some(FallbackReason::GpuUnavailable)
        );
        assert_eq!(compute.device.description, "Intel(R) Core(TM) i5-14500T");
    }

    #[test]
    fn auto_on_the_cpu_names_why() {
        for (gpu, reason) in [
            (GpuAvailability::NotBuilt, "gpu_not_built"),
            (GpuAvailability::ModuleAbsent, "gpu_module_absent"),
            (GpuAvailability::Devices(0), "gpu_unavailable"),
        ] {
            let compute =
                Compute::from_loaded(BackendMode::Auto, gpu, "cpu", "CPU", "x", None).unwrap();
            assert_eq!(compute.to_value()["fallback_reason"], reason);
        }
        let cpu = Compute::from_loaded(
            BackendMode::Cpu,
            GpuAvailability::NotRequested,
            "cpu",
            "CPU",
            "x",
            None,
        )
        .unwrap();
        assert_eq!(cpu.fallback_reason, None);
        assert_eq!(cpu.to_value()["cpu_variant"], serde_json::Value::Null);
    }

    #[test]
    fn a_gpu_request_that_landed_on_the_cpu_is_an_error() {
        let error = Compute::from_loaded(
            BackendMode::Gpu,
            GpuAvailability::Devices(1),
            "cpu",
            "CPU",
            "x",
            None,
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::GpuUnavailable);
        let error = Compute::from_loaded(
            BackendMode::Cpu,
            GpuAvailability::NotRequested,
            "vulkan",
            "Vulkan0",
            "x",
            None,
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::Internal);
        let error = Compute::from_loaded(
            BackendMode::Auto,
            GpuAvailability::Devices(1),
            "cuda",
            "CUDA0",
            "x",
            None,
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::ModelLoadFailed);
        assert!(!error.message.contains("cuda"));
    }

    #[test]
    fn hardware_strings_are_trimmed_clean_and_bounded() {
        let long = format!("  {}\u{7}\n{}  ", "é".repeat(200), "\"\\".repeat(200));
        let compute = Compute::new(
            BackendMode::Auto,
            ComputeBackend::Vulkan,
            &long,
            &long,
            Some(&long),
            None,
        );
        for (text, max) in [
            (&compute.device.name, MAX_LABEL_BYTES),
            (&compute.device.description, MAX_DESCRIPTION_BYTES),
            (compute.cpu_variant.as_ref().unwrap(), MAX_LABEL_BYTES),
        ] {
            assert!(text.len() <= max, "{} > {max}", text.len());
            assert!(!text.chars().any(char::is_control), "{text:?}");
            assert_eq!(text.trim(), text.as_str());
            assert!(!text.is_empty());
        }
        assert_eq!(sanitize("a\tb\u{0}c", 16), "a b c");
        assert_eq!(sanitize("ééé", 5), "éé");
        // An empty name is still a name.
        let blank = Compute::new(
            BackendMode::Cpu,
            ComputeBackend::Cpu,
            " \n ",
            "",
            Some(""),
            None,
        );
        assert_eq!(blank.device.name, "unknown");
        assert_eq!(blank.cpu_variant, None);
    }

    #[test]
    fn labels_name_backend_device_and_request() {
        let gpu = Compute::new(
            BackendMode::Auto,
            ComputeBackend::Vulkan,
            "Vulkan0",
            "NVIDIA",
            Some("haswell"),
            None,
        );
        assert_eq!(
            gpu.label(),
            "vulkan: Vulkan0 (NVIDIA) [cpu: haswell], requested auto"
        );
        let cpu = Compute::new(
            BackendMode::Auto,
            ComputeBackend::Cpu,
            "CPU",
            "Intel",
            Some("haswell"),
            Some(FallbackReason::GpuNotBuilt),
        );
        assert_eq!(
            cpu.label(),
            "cpu: CPU (Intel) [haswell], requested auto, fallback: gpu_not_built"
        );
    }
}
