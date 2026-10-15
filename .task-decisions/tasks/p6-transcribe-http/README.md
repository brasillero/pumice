---
title: P6: transcription over HTTP
state: Done
ticket: —
branch: p6-transcribe-http
mr: PR #97
start: 2026-10-15
end: 2026-10-15
---

`pumice-transcribe serve` ships an OpenAI-compatible `POST /v1/audio/transcriptions` (plus `GET /v1/models`, `GET /v1/models/{model}` and `/health`) on `127.0.0.1`, merged in PR #97 after three Codex review rounds and a check with the official OpenAI Python SDK. The browser test page continues in `p6b-transcribe-test-page`; open follow-ups (#2–#4) are deferred, Opus support (#4) approved for after the CPU speed step.
