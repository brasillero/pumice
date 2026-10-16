---
title: P13.4: dictation route for `pumice serve`
state: Done
ticket: —
branch: p13.4-dictations
mr: —
start: 2026-10-15
end: 2026-10-15
---

Last of four P13 PRs (design and decisions: `p13.1-flow-seam/reports/2026-10-15-p13-design.md`, `p13.1-flow-seam/D1`–`D9`, and D1 here). `pumice serve` now answers `POST /pumice/v1/dictations`: multipart audio plus an enabled flow id in, the P12 schema-1 envelope out with the raw transcript and the final text. The audio goes in memory to the warm managed child, then the P13.1 flow seam runs the formatter and relay. One work permit taken before the body is read (429 when busy, 503 when not ready), deadlines anchored to arrival, bounded upload and response, real-socket disconnect cancellation, child recycle only after an abandoned transcription, and any unconfirmed cleanup stops admission for good. Every refusal on the route is an envelope; the raw transcript survives later failures. Half-close counts as a disconnect (D1). New `docs/api.md` summarizes every Pumice HTTP API. Implemented by Claude Opus 5.5 subagents; two Codex review rounds (round 1: two major, two minor; round 2: approve); final review by the orchestrator, with a real-engine smoke test on server-1 (port 17571): `raw` 5.8 s, `clean` 8.7 s, `clean-en` 8.0 s, busy 429, disconnect cancels and recycles the child, no dictated text in logs, clean SIGTERM. The test page follows in P13.5 (#1).
