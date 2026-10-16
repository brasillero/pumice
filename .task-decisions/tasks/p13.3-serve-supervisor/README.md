---
title: P13.3: `pumice serve` supervisor and status shell
state: Done
ticket: —
branch: p13.3-serve-supervisor
mr: —
start: 2026-10-15
end: 2026-10-15
---

Third of four P13 PRs (design and decisions: `p13.1-flow-seam/reports/2026-10-15-p13-design.md`, `p13.1-flow-seam/D1`–`D9`). `pumice serve [--config] [--port] [--json]` reads a strict `serve:` section in `orchestrator.yml` (port default 17570, 7567 refused, every enabled flow must use `serve.transcription_model`), binds `127.0.0.1` early, and supervises one `pumice-transcribe serve --managed` child: strict lifecycle handshake and `/ready` identity check, 1 s liveness probes, restarts after 0.5/1/2 s (three per episode), fatal protocol errors stop recovery, readiness withdrawn before a child is retired, a `recycle(generation)` hook for P13.4, and D9 shutdown. Routes: `/health`, `/ready`, `/pumice/v1/status`, `/pumice/v1/flows`, behind the D8 guard (literal Host, own Origin only, no cross-site `Sec-Fetch-Site`) with per-route connection policies. Core panics are reported without payload and go through the bounded cleanup (exit 1). Implemented by Claude Opus 5.5 subagents; two Codex review rounds (round 1: three major, one minor; round 2: approve with one test race, fixed by the orchestrator). Orchestrator smoke test on server-1 with the real CPU-variants engine on port 17571: routes, guard, restart after a killed child (generation 2), SIGTERM stop in 0.11 s with exit 143 and no leftover child.
