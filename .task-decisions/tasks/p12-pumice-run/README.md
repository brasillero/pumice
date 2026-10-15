---
title: P12: `pumice run` combined flow
state: Done
ticket: —
branch: p12-pumice-run
mr: PR #105
start: 2026-10-15
end: 2026-10-15
---

Step 6b of `p6-transcribe-http/reports/2026-10-15-implementation-order.md`, with the minimal part of step 5b it needs. Done: `pumice run <AUDIO> --flow <ID> [--config FILE] [--json]` transcribes a file with the sibling `pumice-transcribe`, applies the flow's formatter and calls the relay in-process, and returns the raw transcript and the final text apart in a versioned envelope. Flows are defined in a strict `orchestrator.yml`. Design in `reports/2026-10-15-flow-design.md`, decisions D1–D10. Implemented by Kimi k3; fix rounds by a Claude Opus 5.5 subagent after Kimi hit its quota; four Codex review rounds, approved. Follow-ups deferred in the backlog: panic handling (#1), Windows/macOS cancellation check (#2), Kimi argv exposure (#3).
