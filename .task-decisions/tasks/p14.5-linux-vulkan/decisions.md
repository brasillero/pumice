# Decisions — p14.5-linux-vulkan

### D1 · Active · `auto` uses discrete GPUs only; an integrated GPU needs `backend: gpu`
- date: 2026-10-16
- decided by: claude (orchestrator, owner-delegated 2026-10-16)
- origin: reports/2026-10-16-server-1-real-gpu.md
- area: transcription
- revisit if: a measurement shows an integrated GPU faster than the best CPU variant for turbo on a supported machine

**Decision.** Narrows `p14.1-windows-vulkan/D3`. `backend: auto` uses a GPU only when a discrete GPU registers; when only integrated (or software) GPUs are present it runs on the CPU with `fallback_reason: "gpu_integrated_only"`. `backend: gpu` keeps using whatever GPU the engine selects, discrete first, then integrated. The default stays `cpu`.

**Why.** On server-1 the integrated Intel UHD 770 transcribes turbo about 1.7× slower than the CPU (9.9 s vs 6.5 s for 14 s of audio, warm), so picking it automatically would make transcription worse; integrated GPUs are typically weaker than a modern multi-core CPU for this model.

**Rejected.** Auto-benchmarking at startup (costs a model run each start, design §6 rejected it); keeping `auto` as discrete-then-integrated.

### D2 · Active · `auto` only uses a GPU it can positively identify as discrete hardware
- date: 2026-10-16
- decided by: claude (orchestrator, owner-delegated 2026-10-16)
- origin: D1
- area: transcription
- revisit if: the engine exposes the Vulkan physical-device type and driver id, or the owner's Windows check finds a real discrete card classified otherwise

**Decision.** Refines D1. The engine reports every non-integrated Vulkan device as `gpu`, so Pumice classifies a device as discrete only when it is `gpu`, reports a PCI bus id, and its device description does not name a known software or virtual Vulkan implementation (for example llvmpipe/lavapipe, SwiftShader, Venus/virtio-gpu, Microsoft's Direct3D 12 "Dozen" layer, Microsoft Basic Render). A `gpu` device that fails either test is `unverified`. With no discrete device, `auto` runs on the CPU and reports why (`gpu_integrated_only`, `gpu_software_only`, or `gpu_unverified` when only unverified devices exist). `backend: gpu` can still use any of them.

**Why.** A virtual GPU can report PCI information while running on a software renderer, and some real cards may lack the optional `VK_EXT_pci_bus_info` extension (Codex P14.5 review round 2). `auto` must never make transcription slower; a user with an unusual card keeps `backend: gpu`.

**Rejected.** Trusting the PCI bus id alone; querying Vulkan directly from Pumice (new unsafe dependency, duplicates the engine).

### D3 · Active · In `auto`, a discrete GPU that fails to initialize falls back to the CPU; a failed model load does not
- date: 2026-10-16
- decided by: claude (orchestrator, owner-delegated 2026-10-16)
- origin: p14.1-windows-vulkan/D4
- area: transcription
- revisit if: —

**Decision.** If the engine reports that the chosen discrete device could not be initialized, before any model weights were allocated on it, `auto` loads on the CPU with `fallback_reason: "gpu_unavailable"`. A failure after allocation began (out of memory, model load error) stays a hard `model_load_failed`, as `p14.1-windows-vulkan/D4` says; `backend: gpu` never falls back. If the engine cannot tell the two apart, `auto` keeps the hard failure and the docs say so.

**Why.** `p14.1-windows-vulkan/D3` promised that `auto` falls back when no GPU initializes; D4 only forbids retrying after a model allocation failure (Codex P14.5 review round 2).

**Rejected.** Falling back on every GPU error; failing hard on device initialization in `auto`.
