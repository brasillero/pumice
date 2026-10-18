---
title: P15: pumice serve as an OpenAI transcription endpoint with post-processing models
state: In progress
ticket: —
branch: p15-serve-transcriptions
mr: —
start: 2026-10-18
end: —
---

First usable version for integrations (T3 Code, dictation apps): `pumice serve` answers `POST /v1/audio/transcriptions` and `GET /v1/models` on its own port, OpenAI-shaped, and each listed model is a configured combination of audio model, post-processing preset (off, a formatter preset such as clean or clean-en, or any preset the user saves in YAML) and AI route, so `text` comes back ready to use. A browser test page on `pumice serve` exercises it. Design by Codex, then coder → Codex → orchestrator loop.
