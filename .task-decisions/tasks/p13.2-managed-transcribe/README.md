---
title: P13.2: managed mode for `pumice-transcribe serve`
state: Done
ticket: —
branch: p13.2-managed-transcribe
mr: —
start: 2026-10-15
end: 2026-10-15
---

Second of four P13 PRs (design and decisions: `p13.1-flow-seam/reports/2026-10-15-p13-design.md`, `p13.1-flow-seam/D1`–`D9`). `pumice-transcribe serve --managed --instance-id ID` is the warm child `pumice serve` will own (`p13.1-flow-seam/D4`): stdout carries only lifecycle JSONL (`ready` after load and bind, `stopping`, `stopped`, or one `error`); stdin EOF is the lifeline, started before model load, with a 2 s watchdog that exits with status 3 (immediately if the watchdog thread cannot start); managed-only `GET /ready` and `POST /pumice/v1/transcriptions` (text, language, audio seconds, inference time) share the OpenAI route's parser, limits, permit and worker; managed routes reject any `Origin` and non-literal loopback `Host` with 403, including on unsupported methods. The public OpenAI routes are byte-identical in both modes (parity test). Model ids over 512 bytes are refused in managed mode. Implemented by Claude Opus 5.5 subagents; three Codex review rounds (round 1: one major, two minor; round 2: one minor test race; round 3: approve); final review by the orchestrator. A flaky smoke test seen during review is #1.
