---
title: P6: transcription over HTTP
state: In progress
ticket: —
branch: p6-transcribe-http
mr: PR #97
start: 2026-10-15
end: —
---

`pumice-transcribe serve`: an OpenAI-compatible `POST /v1/audio/transcriptions` (plus `GET /v1/models`, `GET /v1/models/{model}` and `/health`) on `127.0.0.1`, with one model loaded at startup and one transcription at a time. First version by Kimi; three Codex review rounds, with the supervisor fixing rounds 1 and 2 directly at the owner's request; round 3 approved with nits. Checked with the official OpenAI Python SDK against the real Turbo model. Waiting for the owner's review of the PR; the browser test page (#1) comes next.
