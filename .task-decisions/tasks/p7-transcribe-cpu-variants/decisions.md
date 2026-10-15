# Decisions — p7-transcribe-cpu-variants

### D1 · Active · The speed target is faster-than-real-time warm transcription on server-1
- date: 2026-10-15
- decided by: claude (orchestrator), brasillero (authorized)
- origin: p6-transcribe-http/reports/2026-10-15-implementation-order.md
- area: transcription
- revisit if: the owner sets a different target machine or latency

**Decision.** The reference machine is server-1 (i5-14500T, 20 threads), where the server runs today. Goal: warm Turbo Q8_0 transcription of the 14.19 s sample in under 14 s (real-time factor below 1), with the same transcript, and the portable baseline kept as a fallback. The owner's Windows PC is measured later by the owner.

**Why.** The plan asks for a target before starting (item 4a, "Before starting"); the owner was away and authorized the supervisor to decide (`p6-transcribe-http/D10`).

**Rejected.** Waiting for Windows measurements first: no Windows access, and server-1 is the machine in use.

### D2 · Active · CPU variant modules are staged one per directory at build time
- date: 2026-10-15
- decided by: claude (orchestrator), brasillero (authorized)
- origin: D1
- area: transcription
- revisit if: transcribe-cpp exposes a safe single-file backend loader

**Decision.** The staging script places each ggml CPU module alone in `<exe dir>/cpu/<variant>/`. At startup Pumice picks the variant with OS-aware feature detection and initialises backends from that one directory with the CPU mask; nothing is copied or written at runtime.

**Why.** Copying the chosen module into the install directory at runtime fails on read-only installs, can leave directories behind and was shown to be attackable through symlinks (review round 2); the per-directory layout keeps transcribe-cpp's safe API and the CPU-only mask.

**Rejected.** Runtime copy into a private directory; calling ggml's unexported-by-the-bindings `ggml_backend_load` through a hand-written `extern "C"` declaration, which bypasses the CPU mask.
