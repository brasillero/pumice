---
title: P4b: transcription config file, model list and user guide
state: In progress
ticket: —
branch: p4b-transcribe-config
mr: —
start: 2026-10-14
end: —
---

Follow-up to `p4-transcribe-cpu`, picking up the config and docs part of its deferred item: a `transcribe.yml` for `pumice-transcribe` with named local models, a default model, language and threads, validated with exact file and line on errors; `check-config` and `models list` commands with JSON output; and a user guide in `docs/transcription.md`. Kimi (`kimi-k2.7-code-highspeed`) writes the code, Codex reviews and revises, the orchestrator does the final review. Implementation starting.
