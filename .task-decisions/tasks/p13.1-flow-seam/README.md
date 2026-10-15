---
title: P13.1: shared flow execution seam for `pumice run` and `pumice serve`
state: Done
ticket: —
branch: p13.1-flow-seam
mr: —
start: 2026-10-15
end: 2026-10-15
---

First of four P13 PRs that add `pumice serve`, a long-running local stack a UI calls with audio plus a flow and gets the raw transcript and the final text back (design: `reports/2026-10-15-p13-design.md`; decisions D1–D9 cover the whole P13 series). This PR extracts the stage runner from `src/orchestrator/run.rs` into `src/orchestrator/flow.rs`: a `Runtime` built once (relay pipeline, formatter registry, `CleanupScope`), per-operation execution with an injectable `Transcriber` transport, a caller-anchored deadline and a cancellation context with service reasons (`service_shutdown`, `client_disconnected`). `pumice run` behaves exactly as before (`tests/pumice_run.rs` unchanged). Also: `CleanupScope::drain` is now cancellation-safe (a dropped drain returns unfinished teardown to the scope), and `pumice` is a library plus a thin binary so integration tests reach the seam. Implemented by a Claude Opus 5.5 subagent; two Codex review rounds (round 1: one major, two minor; round 2: approve); final review by the orchestrator.
