# Decisions — p14.1-windows-vulkan

### D1 · Active · GPU transcription starts as Windows Vulkan CI artifacts, not a public release
- date: 2026-10-16
- decided by: claude (orchestrator, owner-delegated 2026-10-16)
- origin: reports/2026-10-16-gpu-design.md
- area: distribution
- revisit if: the owner's real-GPU and clean-machine checks on Windows pass

**Decision.** The GPU work follows the design's sequence: P14.1 Windows Vulkan bundle foundation, P14.2 backend policy and reporting, P14.3 Windows packaging preview with a CPU-only archive, then macOS arm64 Metal and Linux GNU Vulkan in their own PRs. Until the owner has checked a real GPU on Windows, the bundles are CI artifacts only; `p6-transcribe-http/D2` (no downloadable transcription packages yet) stays in force for public releases. CUDA and ROCm are out of scope.

**Why.** The owner asked for GPU transcription (2026-10-16); Windows is the main target, but real-GPU behavior can only be checked on the owner's machine.

**Rejected.** Publishing GPU packages before a hardware check; starting with Linux because server-1 has an Intel GPU (kept for P14.5).

### D2 · Active · The Vulkan module loads next to the CPU variants with one fixed backend mask
- date: 2026-10-16
- decided by: claude (orchestrator, owner-delegated 2026-10-16)
- origin: reports/2026-10-16-gpu-design.md
- area: transcription
- revisit if: —

**Decision.** The GPU bundle reuses the `cpu-variants` machinery (`p7-transcribe-cpu-variants`): the OS-aware chooser loads one `cpu/<variant>/` module, then `gpu/vulkan/` is loaded with the same final mask (CPU|VULKAN, never `ALL`, so `GGML_BACKEND_PATH` plugins are never accepted). In `cpu` mode the GPU directory is never scanned. The executable and core libraries must not import Vulkan; only the optional module does, so a machine without a Vulkan loader or device still runs on CPU.

**Why.** One bundle works with and without a GPU, without new FFI or a second loader.

**Rejected.** A separate GPU-only executable; `BackendMask::ALL`.

### D3 · Active · Backend selection is `cpu`, `auto` or `gpu`, with `cpu` as the default
- date: 2026-10-16
- decided by: claude (orchestrator, owner-delegated 2026-10-16)
- origin: reports/2026-10-16-gpu-design.md
- area: config
- revisit if: the owner's Windows GPU check shows `auto` is reliable enough to be the default

**Decision.** `transcribe.yml` gets `backend: cpu|auto|gpu` (default `cpu`), overridable by `--backend` on `file` and `serve`. `auto` uses the pack's GPU when one initializes (discrete before integrated) and falls back to CPU with a safe reason; `gpu` requires the GPU and never falls back. No device index in the first increment; the actual device is reported. No per-request backend changes and no backend settings in `orchestrator.yml`.

**Why.** CPU must stay available as a dependable escape hatch for broken drivers, and a required-GPU mode keeps tests and measurements honest.

**Rejected.** Automatic-only selection; a persistent numeric device index.

### D4 · Active · No hidden CPU retry after a GPU failure
- date: 2026-10-16
- decided by: claude (orchestrator, owner-delegated 2026-10-16)
- origin: reports/2026-10-16-gpu-design.md
- area: transcription
- revisit if: —

**Decision.** A GPU model-allocation failure at load fails startup with `model_load_failed` and fixed guidance to select `cpu`; it is not retried on CPU. A native engine failure during inference returns the existing typed error, is never retried on CPU or replayed, and retires the process (readiness withdrawn; under `pumice serve` the existing bounded restart in `p13.1-flow-seam/D5` applies). Cancellation and input errors do not retire it.

**Why.** Same no-replay rule as the rest of Pumice; a poisoned device is not trusted in the same process.

**Rejected.** Same-process driver reset; a permanent hidden downgrade to CPU.

### D5 · Active · The selected backend is reported everywhere through internal protocol 2
- date: 2026-10-16
- decided by: claude (orchestrator, owner-delegated 2026-10-16)
- origin: reports/2026-10-16-gpu-design.md
- area: api
- revisit if: —

**Decision.** One immutable `compute` snapshot (requested mode, actual backend and device, CPU variant, fallback reason; bounded strings) appears in `file --json`, standalone `serve` startup output, managed `ready` and `GET /ready`, and `pumice serve`'s `/pumice/v1/status`. Because both ends validate exact field sets, the managed protocol version becomes 2 on both sides in the same PR (P14.2). The public `/v1/audio/transcriptions` response is unchanged.

**Why.** Users and the supervisor must see what actually runs, not what was requested.

**Rejected.** Reporting the requested mode only.

### D6 · Active · Windows GPU builds keep the static CRT unless evidence forces an owner decision
- date: 2026-10-16
- decided by: claude (orchestrator, owner-delegated 2026-10-16)
- origin: reports/2026-10-16-gpu-design.md
- area: distribution
- revisit if: the `/MT` audit of the full native DLL graph fails

**Decision.** The Windows Vulkan build uses `+crt-static` and native `/MT` (`CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded`). Every shipped executable and DLL is audited with `dumpbin /dependents`; a dependency on vcruntime, msvcp or a UCRT redistributable fails the build. If `/MT` cannot work, the work stops and the evidence goes to the owner; no silent switch to `/MD`.

**Why.** AGENTS.md: no developer runtimes for end users.

**Rejected.** Shipping or requiring the VC++ redistributable.

### D7 · Superseded by D8 · The Vulkan SDK and its install action are pinned
- date: 2026-10-16
- decided by: claude (orchestrator, owner-delegated 2026-10-16)
- origin: reports/2026-10-16-gpu-design.md
- area: distribution
- revisit if: the pinned SDK version stops being downloadable or fails to build the pinned engine

**Decision.** CI installs LunarG Vulkan SDK 1.4.350.0 (the version the pinned engine revision uses) with `humbletim/install-vulkan-sdk` pinned by commit SHA, on a pinned Windows runner image, and verifies `glslc`, the Vulkan headers and import library before building. End users need only their GPU driver's Vulkan loader; no SDK, shader compiler or runtime is shipped or required.

**Why.** Reproducible builds; the SDK is build-host tooling only.

**Rejected.** Headers+loader-only setups (no `glslc`); unpinned `windows-latest` with the latest SDK.

### D8 · Active · CI installs the Vulkan SDK with LunarG's own installer, checked by SHA-256
- date: 2026-10-16
- decided by: claude (orchestrator, owner-delegated 2026-10-16)
- origin: D7
- area: distribution
- revisit if: the pinned SDK version stops being downloadable or fails to build the pinned engine

**Decision.** The Windows job downloads LunarG Vulkan SDK 1.4.350.0's installer, verifies its SHA-256 against the value LunarG publishes, and runs it silently, as the pinned engine's own CI does; the runner image stays pinned (`windows-2022`). It then checks `glslc`, the Vulkan headers and `vulkan-1.lib` before building. End users still need only their GPU driver's Vulkan loader.

**Why.** `humbletim/install-vulkan-sdk@30ba978` unpacks only `Bin\` from this installer (no `Include\`, no `Lib\vulkan-1.lib`, no SPIRV-Headers), so the build cannot find them (P14.1 CI run 2).

**Rejected.** The install action of D7; building shaderc and the headers from source in CI.
