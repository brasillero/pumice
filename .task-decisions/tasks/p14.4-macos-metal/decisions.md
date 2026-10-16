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

### D2 · Active · On Apple Silicon `auto` does not load the Metal module
- date: 2026-10-16
- decided by: claude (orchestrator, owner-delegated 2026-10-16)
- origin: D1
- area: transcription
- revisit if: D1 is superseded

**Decision.** While D1 holds, `auto` on Apple Silicon never registers the Metal module: it loads with the CPU-only mask and reports `fallback_reason: "gpu_auto_disabled"`. `backend: gpu` uses Metal, and `backends --json` still lists the Metal device as a diagnostic.

**Why.** `auto` can never pick Metal under D1, yet registering it compiled Metal shaders for 43–45 s on the first start in a new folder on GitHub's Apple Silicon runner (Codex P14.4 review round 1).

**Rejected.** Scanning Metal and then falling back with `gpu_unverified` (pays the start-up cost for nothing).
