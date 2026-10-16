---
title: P13.4: dictation route for `pumice serve`
state: In progress
ticket: —
branch: p13.4-dictations
mr: —
start: 2026-10-15
end: —
---

Last of four P13 PRs (design and decisions: `p13.1-flow-seam/reports/2026-10-15-p13-design.md`, `p13.1-flow-seam/D1`–`D9`). Adds `POST /pumice/v1/dictations` to `pumice serve`: multipart audio plus a flow id in, the P12 run envelope out (raw transcript and final text, per-stage status), using the warm managed child for transcription and the P13.1 flow seam for formatter and relay. One work permit, deadlines anchored to request arrival, client-disconnect cancellation, and child recycle after an abandoned transcription (`p13.1-flow-seam/D5`, `D6`). Implementation by a Claude Opus 5.5 subagent, review by Codex, final review by the orchestrator.
