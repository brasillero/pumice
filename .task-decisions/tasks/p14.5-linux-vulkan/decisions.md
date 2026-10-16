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
