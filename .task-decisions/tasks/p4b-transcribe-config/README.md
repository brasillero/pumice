---
title: P4b: transcription config file, model list and user guide
state: Done
ticket: —
branch: p4b-transcribe-config
mr: PR #96
start: 2026-10-14
end: 2026-10-14
---

Follow-up to `p4-transcribe-cpu`: `pumice-transcribe` reads a strict `transcribe.yml` (named models, default model, language, threads; errors with file:line:column), selects models with `--model ID` or `--model-file PATH`, and has `check-config` and `models list` with JSON output. Example config at the root and a user guide in `docs/transcription.md`. Implemented by Kimi (`kimi-k2.7-code-highspeed`), reviewed and revised by Codex (one round, fixes in `352f03e`), final review by the orchestrator: approved. Merged through PR #96; the release archive and Windows/macOS builds continue in #2.
