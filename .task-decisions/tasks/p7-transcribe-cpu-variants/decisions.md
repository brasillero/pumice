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
