//! The compute backend policy and the reported compute snapshot
//! (p14.1-windows-vulkan/D3–D5, narrowed by p14.5-linux-vulkan/D1–D3).
//!
//! `backend: cpu|auto|gpu` (`--backend` on `file`, `serve` and `backends`)
//! chooses what the model load may use:
//! - `cpu` never scans or loads a GPU module;
//! - `auto` uses a discrete GPU when the bundle's GPU module registers one,
//!   loading on that device explicitly; with only integrated, unverified or
//!   software GPUs, or none, it runs on the CPU with a [`FallbackReason`];
//! - `gpu` requires a GPU device of any class, uses the engine's choice
//!   (discrete before integrated) and never falls back.
//!
//! The engine's device-type axis cannot tell a discrete GPU from a software
//! or virtual one: ggml-vulkan reports every Vulkan device that is not
//! integrated as `gpu`, including a CPU-type device such as Mesa's lavapipe
//! forced in with `GGML_VK_VISIBLE_DEVICES`. [`GpuClass::of`] therefore only
//! calls a `gpu` device discrete when it also reports a PCI bus id and its
//! description names no known software or virtual Vulkan implementation
//! (p14.5-linux-vulkan/D2).
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
    /// A discrete GPU when one registers, else the CPU.
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
    /// The GPU module was scanned and registered these GPU devices (none:
    /// no loader, no driver or no usable device; the public API cannot tell
    /// which).
    Devices(GpuDevices),
}

/// How the backend policy classes one registered GPU device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GpuClass {
    /// Dedicated GPU hardware: the only class `auto` loads on.
    Discrete,
    /// A GPU sharing system memory with the CPU; `backend: gpu` only.
    Integrated,
    /// A `gpu` device Pumice cannot prove is discrete hardware: no PCI bus
    /// id, or a virtual or layered implementation that may run on anything
    /// (Venus, Dozen); `backend: gpu` only.
    Unverified,
    /// A known software rasterizer, or a device of another engine type;
    /// `backend: gpu` only.
    Software,
}

/// Lowercase substrings of the Vulkan `deviceName` (ggml-vulkan reports it
/// as the device description, ggml-vulkan.cpp `ggml_vk_get_device_description`)
/// of software rasterizers, which run on the CPU:
/// - Mesa llvmpipe, and lavapipe, which reports llvmpipe's screen name:
///   `llvmpipe (LLVM <version>, <n> bits)` (mesa
///   src/gallium/drivers/llvmpipe/lp_screen.c,
///   src/gallium/frontends/lavapipe/lvp_device.c);
/// - Google SwiftShader: `SwiftShader Device (<backend>)` (swiftshader
///   src/Vulkan/VkConfig.hpp `SWIFTSHADER_DEVICE_NAME`,
///   src/Vulkan/VkPhysicalDevice.cpp);
/// - Microsoft's WARP adapter, `Microsoft Basic Render Driver`, as Dozen
///   wraps it: `Microsoft Direct3D12 (Microsoft Basic Render Driver)`.
///
/// `lavapipe` is not in any name today; it is kept in case a build or
/// `force_vk_devicename` uses it.
const SOFTWARE_DEVICE_NAMES: &[&str] = &[
    "llvmpipe",
    "lavapipe",
    "swiftshader",
    "microsoft basic render",
];

/// Lowercase substrings of the `deviceName` of virtual or layered Vulkan
/// implementations, which may report PCI information while running on
/// anything from a real GPU to a software rasterizer:
/// - Mesa Venus (Vulkan over virtio-gpu): `Virtio-GPU Venus (<host
///   device>)` (mesa src/virtio/vulkan/vn_physical_device.c);
/// - Mesa Dozen (Vulkan over Direct3D 12): `Microsoft Direct3D12
///   (<adapter>)` (mesa src/microsoft/vulkan/dzn_device.c).
///
/// `virgl` (virtio-gpu's OpenGL renderer) and `dozen` are not in any Vulkan
/// name today; they are kept as cheap guards for the same stacks.
const VIRTUAL_DEVICE_NAMES: &[&str] =
    &["venus", "virtio", "virgl", "dozen", "microsoft direct3d12"];

impl GpuClass {
    /// From the engine's device type (`gpu`, `igpu`, `cpu`, `accel`,
    /// `unknown`, as `backends --json` reports it), the PCI bus id the
    /// device reports, if any, and its description (p14.5-linux-vulkan/D2).
    /// Only a `gpu` with a PCI bus id whose description names no software or
    /// virtual implementation is discrete. A software rasterizer is named
    /// before a virtual layer, so Venus or Dozen over llvmpipe or WARP is
    /// `software`.
    pub fn of(device_type: &str, pci_bus_id: Option<&str>, description: &str) -> Self {
        let on_pci = pci_bus_id.is_some_and(|id| !id.trim().is_empty());
        let description = description.to_lowercase();
        let names = |list: &[&str]| list.iter().any(|name| description.contains(name));
        match device_type {
            "igpu" => Self::Integrated,
            "gpu" if names(SOFTWARE_DEVICE_NAMES) => Self::Software,
            "gpu" if on_pci && !names(VIRTUAL_DEVICE_NAMES) => Self::Discrete,
            "gpu" => Self::Unverified,
            _ => Self::Software,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Discrete => "discrete",
            Self::Integrated => "integrated",
            Self::Unverified => "unverified",
            Self::Software => "software",
        }
    }
}

/// The GPU devices one scan registered, by class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GpuDevices {
    pub discrete: usize,
    pub integrated: usize,
    pub unverified: usize,
    pub software: usize,
}

impl GpuDevices {
    pub fn count(classes: &[GpuClass]) -> Self {
        let mut devices = Self::default();
        for class in classes {
            match class {
                GpuClass::Discrete => devices.discrete += 1,
                GpuClass::Integrated => devices.integrated += 1,
                GpuClass::Unverified => devices.unverified += 1,
                GpuClass::Software => devices.software += 1,
            }
        }
        devices
    }

    pub const fn total(self) -> usize {
        self.discrete + self.integrated + self.unverified + self.software
    }
}

/// The device `auto` loads on: the first discrete GPU in registry order (the
/// order the engine itself probes GPUs in), as an index into `classes`.
pub fn auto_device(classes: &[GpuClass]) -> Option<usize> {
    classes
        .iter()
        .position(|class| *class == GpuClass::Discrete)
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
            Self::Devices(devices) => devices.total(),
            _ => 0,
        }
    }
}

/// What the model load asks the engine for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadRequest {
    /// The CPU device only.
    Cpu,
    /// `auto`: exactly the device [`auto_device`] picks, a discrete GPU,
    /// without fallback.
    Discrete,
    /// `gpu`: the build's GPU backend on the engine's choice of device
    /// (discrete before integrated), without fallback.
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
    /// No discrete GPU, but an integrated one (and maybe others);
    /// `auto` skips integrated GPUs, which are usually slower than the CPU
    /// for this work. `backend: gpu` uses them.
    GpuIntegratedOnly,
    /// No discrete or integrated GPU, but a `gpu` device that could not be
    /// verified as discrete hardware (and maybe software ones). `backend:
    /// gpu` uses it.
    GpuUnverified,
    /// The only GPU devices are software rasterizers, which are no faster
    /// than the CPU they run on.
    GpuSoftwareOnly,
}

impl FallbackReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GpuNotBuilt => "gpu_not_built",
            Self::GpuModuleAbsent => "gpu_module_absent",
            Self::GpuUnavailable => "gpu_unavailable",
            Self::GpuIntegratedOnly => "gpu_integrated_only",
            Self::GpuUnverified => "gpu_unverified",
            Self::GpuSoftwareOnly => "gpu_software_only",
        }
    }

    fn for_availability(gpu: GpuAvailability) -> Self {
        match gpu {
            GpuAvailability::NotBuilt => Self::GpuNotBuilt,
            GpuAvailability::ModuleAbsent => Self::GpuModuleAbsent,
            GpuAvailability::Devices(devices) if devices.discrete == 0 => {
                if devices.integrated > 0 {
                    Self::GpuIntegratedOnly
                } else if devices.unverified > 0 {
                    Self::GpuUnverified
                } else if devices.software > 0 {
                    Self::GpuSoftwareOnly
                } else {
                    Self::GpuUnavailable
                }
            }
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
/// A load that may have allocated on a GPU failed (for `auto`, also a discrete
/// GPU that did not initialize). Not retried on the CPU.
pub const GPU_LOAD_FAILED: &str = "failed to load model with backend {mode}; a GPU load failure \
     is not retried on the CPU. Use backend: cpu (or --backend cpu) to run on the CPU.";

/// What the model load requests for `mode`, given what registered.
pub fn load_request(mode: BackendMode, gpu: GpuAvailability) -> Result<LoadRequest, Error> {
    match (mode, gpu) {
        (BackendMode::Cpu, _) => Ok(LoadRequest::Cpu),
        (BackendMode::Auto, GpuAvailability::Devices(devices)) if devices.discrete > 0 => {
            Ok(LoadRequest::Discrete)
        }
        (BackendMode::Auto, _) => Ok(LoadRequest::Cpu),
        (BackendMode::Gpu, GpuAvailability::Devices(devices)) if devices.total() > 0 => {
            Ok(LoadRequest::Gpu)
        }
        (BackendMode::Gpu, GpuAvailability::NotBuilt) => Err(Error::gpu_unavailable(GPU_NOT_BUILT)),
        (BackendMode::Gpu, GpuAvailability::ModuleAbsent) => {
            Err(Error::gpu_unavailable(GPU_MODULE_ABSENT))
        }
        (BackendMode::Gpu, _) => Err(Error::gpu_unavailable(GPU_NO_DEVICE)),
    }
}

/// Why `auto` will run on the CPU before anything loads, when registration
/// already decides it (no GPU built, staged or registered, or no discrete
/// one). A discrete GPU can still fail to initialize at load, which is a
/// load error; only the loaded snapshot is certain.
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
///
/// `auto` never falls back here, even when its discrete device failed to
/// initialize (p14.5-linux-vulkan/D3 allows the fallback only when the
/// engine can tell that case apart, and transcribe-cpp 0.3.1 cannot): the
/// exact-device init failure is `TRANSCRIBE_ERR_BACKEND`
/// (transcribe-cpp-sys src/transcribe-load-common.cpp,
/// `init_backends_explicit_device`), but so is any exception after the
/// weights were allocated, such as ggml-vulkan's `vk::SystemError` from a
/// staging-buffer allocation while uploading them (`api_guard_status` in
/// src/transcribe.cpp, `ggml_vk_create_buffer_check` in
/// ggml-vulkan-buffers.cpp), and there is no device-init call to run first.
pub fn load_failed(mode: BackendMode, request: LoadRequest, backend_error: bool) -> Error {
    match request {
        LoadRequest::Gpu if backend_error => Error::gpu_unavailable(GPU_NO_DEVICE),
        LoadRequest::Gpu | LoadRequest::Discrete => Error::new(
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

    fn devices(
        discrete: usize,
        integrated: usize,
        unverified: usize,
        software: usize,
    ) -> GpuAvailability {
        GpuAvailability::Devices(GpuDevices {
            discrete,
            integrated,
            unverified,
            software,
        })
    }

    const NONE: GpuAvailability = GpuAvailability::Devices(GpuDevices {
        discrete: 0,
        integrated: 0,
        unverified: 0,
        software: 0,
    });

    const PCI: Option<&str> = Some("0000:01:00.0");

    #[test]
    fn only_a_gpu_type_device_on_pci_with_a_hardware_name_is_discrete() {
        use GpuClass::*;
        assert_eq!(
            GpuClass::of("gpu", PCI, "NVIDIA GeForce RTX 4070"),
            Discrete
        );
        assert_eq!(
            GpuClass::of("gpu", PCI, "AMD Radeon RX 7800 XT (RADV NAVI32)"),
            Discrete
        );
        assert_eq!(
            GpuClass::of(
                "igpu",
                Some("0000:00:02.0"),
                "Intel(R) UHD Graphics 770 (ADL-S GT1)"
            ),
            Integrated
        );
        assert_eq!(GpuClass::of("igpu", None, "x"), Integrated);
        // Venus reports the host's PCI information, but may run on a
        // software renderer: never discrete.
        assert_eq!(
            GpuClass::of("gpu", PCI, "Virtio-GPU Venus (NVIDIA GeForce RTX 4070)"),
            Unverified
        );
        assert_eq!(
            GpuClass::of("gpu", PCI, "Microsoft Direct3D12 (NVIDIA GeForce RTX 4070)"),
            Unverified
        );
        // A real-looking card without VK_EXT_pci_bus_info.
        assert_eq!(
            GpuClass::of("gpu", None, "NVIDIA GeForce RTX 4070"),
            Unverified
        );
        assert_eq!(
            GpuClass::of("gpu", Some(" "), "NVIDIA GeForce RTX 4070"),
            Unverified
        );
        // lavapipe forced in through GGML_VK_VISIBLE_DEVICES: ggml reports
        // it as `gpu`; its name says it is a software rasterizer, with or
        // without PCI information, and also behind a virtual layer.
        for description in [
            "llvmpipe (LLVM 15.0.7, 256 bits)",
            "SwiftShader Device (Subzero)",
            "Microsoft Direct3D12 (Microsoft Basic Render Driver)",
            "Virtio-GPU Venus (llvmpipe (LLVM 19.1.1, 256 bits))",
        ] {
            assert_eq!(
                GpuClass::of("gpu", None, description),
                Software,
                "{description}"
            );
            assert_eq!(
                GpuClass::of("gpu", PCI, description),
                Software,
                "{description}"
            );
        }
        for other in ["cpu", "accel", "unknown", ""] {
            assert_eq!(
                GpuClass::of(other, PCI, "NVIDIA GeForce RTX 4070"),
                Software
            );
        }
        assert_eq!(
            GpuDevices::count(&[Integrated, Discrete, Software, Unverified, Discrete]),
            GpuDevices {
                discrete: 2,
                integrated: 1,
                unverified: 1,
                software: 1,
            }
        );
        assert_eq!(GpuAvailability::Devices(GpuDevices::count(&[])), NONE);
        assert_eq!(devices(1, 2, 3, 4).gpu_devices(), 10);
    }

    #[test]
    fn auto_decides_from_the_classified_devices() {
        let scan = |devices: &[(&str, Option<&str>, &str)]| {
            let classes: Vec<_> = devices
                .iter()
                .map(|(kind, pci, description)| GpuClass::of(kind, *pci, description))
                .collect();
            let gpu = GpuAvailability::Devices(GpuDevices::count(&classes));
            (
                auto_device(&classes),
                load_request(BackendMode::Auto, gpu).unwrap(),
                planned_fallback(BackendMode::Auto, gpu),
            )
        };
        let venus = ("gpu", PCI, "Virtio-GPU Venus (Intel(R) Arc(tm) A770)");
        let rtx = ("gpu", PCI, "NVIDIA GeForce RTX 4070");
        let uhd = ("igpu", Some("0000:00:02.0"), "Intel(R) UHD Graphics 770");
        let lavapipe = ("gpu", None, "llvmpipe (LLVM 15.0.7, 256 bits)");
        assert_eq!(
            scan(&[venus]),
            (None, LoadRequest::Cpu, Some(FallbackReason::GpuUnverified))
        );
        assert_eq!(
            scan(&[lavapipe, venus]),
            (None, LoadRequest::Cpu, Some(FallbackReason::GpuUnverified))
        );
        assert_eq!(
            scan(&[venus, uhd]),
            (
                None,
                LoadRequest::Cpu,
                Some(FallbackReason::GpuIntegratedOnly)
            )
        );
        assert_eq!(
            scan(&[lavapipe]),
            (
                None,
                LoadRequest::Cpu,
                Some(FallbackReason::GpuSoftwareOnly)
            )
        );
        assert_eq!(scan(&[rtx]), (Some(0), LoadRequest::Discrete, None));
        // The real card wins wherever it registered.
        assert_eq!(
            scan(&[venus, uhd, lavapipe, rtx]),
            (Some(3), LoadRequest::Discrete, None)
        );
    }

    #[test]
    fn auto_picks_the_first_discrete_device() {
        use GpuClass::*;
        assert_eq!(auto_device(&[Discrete]), Some(0));
        // Discrete and integrated: the discrete one, wherever it registered.
        assert_eq!(auto_device(&[Integrated, Discrete]), Some(1));
        assert_eq!(
            auto_device(&[Software, Integrated, Discrete, Discrete]),
            Some(2)
        );
        assert_eq!(auto_device(&[Unverified, Discrete]), Some(1));
        assert_eq!(auto_device(&[Integrated]), None);
        assert_eq!(auto_device(&[Software, Integrated, Unverified]), None);
        assert_eq!(auto_device(&[]), None);
    }

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
            NONE,
            devices(1, 1, 0, 0),
        ] {
            assert_eq!(
                load_request(BackendMode::Cpu, gpu).unwrap(),
                LoadRequest::Cpu
            );
        }
    }

    #[test]
    fn auto_loads_on_a_gpu_only_when_a_discrete_one_registered() {
        for gpu in [
            devices(1, 0, 0, 0),
            devices(1, 1, 0, 0),
            devices(2, 1, 0, 1),
        ] {
            assert_eq!(
                load_request(BackendMode::Auto, gpu).unwrap(),
                LoadRequest::Discrete,
                "{gpu:?}"
            );
        }
        for gpu in [
            GpuAvailability::NotBuilt,
            GpuAvailability::ModuleAbsent,
            NONE,
            devices(0, 1, 0, 0),
            devices(0, 0, 0, 1),
            devices(0, 2, 0, 1),
            devices(0, 0, 1, 0),
            devices(0, 1, 2, 1),
        ] {
            assert_eq!(
                load_request(BackendMode::Auto, gpu).unwrap(),
                LoadRequest::Cpu,
                "{gpu:?}"
            );
        }
    }

    #[test]
    fn gpu_mode_uses_any_gpu_and_never_falls_back() {
        // Integrated-only and software-only machines still get their GPU.
        for gpu in [
            devices(1, 0, 0, 0),
            devices(0, 1, 0, 0),
            devices(0, 0, 1, 0),
            devices(0, 0, 0, 1),
        ] {
            assert_eq!(
                load_request(BackendMode::Gpu, gpu).unwrap(),
                LoadRequest::Gpu,
                "{gpu:?}"
            );
            assert_eq!(planned_fallback(BackendMode::Gpu, gpu), None);
        }
        for (gpu, message) in [
            (GpuAvailability::NotBuilt, GPU_NOT_BUILT),
            (GpuAvailability::ModuleAbsent, GPU_MODULE_ABSENT),
            (NONE, GPU_NO_DEVICE),
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
            planned_fallback(BackendMode::Auto, NONE),
            Some(FallbackReason::GpuUnavailable)
        );
        assert_eq!(
            planned_fallback(BackendMode::Auto, devices(0, 1, 0, 0)),
            Some(FallbackReason::GpuIntegratedOnly)
        );
        // An integrated GPU names the reason even next to a software one.
        assert_eq!(
            planned_fallback(BackendMode::Auto, devices(0, 1, 0, 1)),
            Some(FallbackReason::GpuIntegratedOnly)
        );
        // Unverified (Venus, a card without a PCI bus id), next to software
        // ones too; an integrated GPU still names the reason first.
        assert_eq!(
            planned_fallback(BackendMode::Auto, devices(0, 0, 1, 0)),
            Some(FallbackReason::GpuUnverified)
        );
        assert_eq!(
            planned_fallback(BackendMode::Auto, devices(0, 0, 1, 1)),
            Some(FallbackReason::GpuUnverified)
        );
        assert_eq!(
            planned_fallback(BackendMode::Auto, devices(0, 1, 1, 0)),
            Some(FallbackReason::GpuIntegratedOnly)
        );
        // Software only, e.g. lavapipe forced in with GGML_VK_VISIBLE_DEVICES.
        assert_eq!(
            planned_fallback(BackendMode::Auto, devices(0, 0, 0, 1)),
            Some(FallbackReason::GpuSoftwareOnly)
        );
        assert_eq!(
            planned_fallback(BackendMode::Auto, devices(1, 1, 0, 0)),
            None
        );
        assert_eq!(
            planned_fallback(BackendMode::Cpu, GpuAvailability::NotRequested),
            None
        );
        assert_eq!(planned_fallback(BackendMode::Gpu, NONE), None);
    }

    #[test]
    fn load_failures_on_a_gpu_point_to_cpu_and_are_not_retried() {
        let error = load_failed(BackendMode::Gpu, LoadRequest::Gpu, true);
        assert_eq!(error.code, ErrorCode::GpuUnavailable);
        for (mode, request) in [
            (BackendMode::Gpu, LoadRequest::Gpu),
            (BackendMode::Auto, LoadRequest::Discrete),
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
            devices(1, 1, 0, 0),
            "vulkan",
            "Vulkan1",
            "NVIDIA GeForce RTX 4070",
            Some("haswell"),
        )
        .unwrap();
        assert_eq!(
            compute.to_value().to_string(),
            r#"{"requested":"auto","backend":"vulkan","device":{"name":"Vulkan1","description":"NVIDIA GeForce RTX 4070"},"cpu_variant":"haswell","fallback_reason":null}"#
        );
        // `gpu` on an integrated-only machine reports the integrated GPU.
        let compute = Compute::from_loaded(
            BackendMode::Gpu,
            devices(0, 1, 0, 0),
            "vulkan",
            "Vulkan0",
            "Intel(R) UHD Graphics 770 (ADL-S GT1)",
            Some("alderlake"),
        )
        .unwrap();
        assert_eq!(compute.backend, ComputeBackend::Vulkan);
        assert_eq!(compute.fallback_reason, None);
        // Whatever the engine loaded on is what is reported, even a CPU
        // `auto` did not plan for.
        let compute = Compute::from_loaded(
            BackendMode::Auto,
            devices(1, 0, 0, 0),
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
            (NONE, "gpu_unavailable"),
            (devices(0, 1, 0, 0), "gpu_integrated_only"),
            (devices(0, 0, 2, 0), "gpu_unverified"),
            (devices(0, 0, 0, 2), "gpu_software_only"),
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
            devices(1, 0, 0, 0),
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
            devices(1, 0, 0, 0),
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
