---
title: P13.1: shared flow execution seam for `pumice run` and `pumice serve`
state: In progress
ticket: —
branch: p13.1-flow-seam
mr: —
start: 2026-10-15
end: —
---

First of four P13 PRs that add `pumice serve`, a long-running local stack a UI calls with audio plus a flow and gets the raw transcript and the final text back (design: `reports/2026-10-15-p13-design.md`, decisions D1–D9, which cover the whole P13 series). This PR only extracts the stage runner from `src/orchestrator/run.rs` behind an injectable audio source and transcription transport, deadline and cancellation context, so `pumice serve` can reuse it; `pumice run` behaves exactly as before. Implementation by a Claude Opus 5.5 subagent, review by Codex, final review by the orchestrator.
