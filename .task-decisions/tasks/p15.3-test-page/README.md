---
title: P15.3: browser test page on pumice serve
state: Done
ticket: —
branch: p15.3-test-page
mr: PR #122
start: 2026-10-18
end: 2026-10-18
---

`pumice serve` serves an embedded test page at `/` (`p15-serve-transcriptions/D8`): it lists the published models from `/v1/models`, records or uploads audio, calls `/v1/audio/transcriptions` on the same origin and shows the final text, the raw transcript, the post-processing status and timings. Strict content security policy, no external assets, the existing Host/Origin guard. Opt-in `serve.allowed_origins` lets the owner reach it over Tailscale through `tailscale serve` (D1); `pumice serve` still listens on loopback only. `tools/transcribe-test` stays for the standalone `pumice-transcribe serve`. Implemented by a Claude Opus 5.5 subagent; one Codex review round (approve); final review by the orchestrator.
