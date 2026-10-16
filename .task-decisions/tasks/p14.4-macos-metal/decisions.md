# Decisions — p14.4-macos-metal

### D1 · Active · On Apple Silicon `auto` uses the CPU until a real Mac shows Metal is faster
- date: 2026-10-16
- decided by: claude (orchestrator, owner-delegated 2026-10-16)
- origin: p14.5-linux-vulkan/D1
- area: transcription
- revisit if: #1 shows Metal faster than the CPU for turbo on an Apple Silicon Mac

**Decision.** Apple Silicon GPUs are integrated (unified memory) and report no PCI bus id, so under `p14.5-linux-vulkan/D1–D2` `auto` runs on the CPU and reports why; `backend: gpu` uses Metal. The default stays `cpu`.

**Why.** `auto` must never make transcription slower, and nothing in CI can measure Metal (hosted runners have no GPU). Metal is widely reported to beat the CPU on Apple Silicon, so this is expected to change after #1.

**Rejected.** Letting `auto` pick Metal without a measurement on our engine and model.
