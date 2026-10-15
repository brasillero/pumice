---
title: P13.2: managed mode for `pumice-transcribe serve`
state: In progress
ticket: —
branch: p13.2-managed-transcribe
mr: —
start: 2026-10-15
end: —
---

Second of four P13 PRs (design and decisions: `p13.1-flow-seam/reports/2026-10-15-p13-design.md`, `p13.1-flow-seam/D1`–`D9`). Adds a managed mode to `pumice-transcribe serve` so `pumice serve` can own it as a warm child (`p13.1-flow-seam/D4`): port-0 lifecycle announcement on stdout, stdin EOF as the shutdown lifeline with a bounded watchdog, managed-only `GET /ready` and `POST /pumice/v1/transcriptions` (detailed result) reusing the existing parser, limits, admission and worker. The public OpenAI routes stay byte-identical. Implementation by a Claude Opus 5.5 subagent, review by Codex, final review by the orchestrator.
