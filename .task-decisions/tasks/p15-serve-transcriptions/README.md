---
title: P15: pumice serve as an OpenAI transcription endpoint with post-processing models
state: Done
ticket: —
branch: p15-serve-transcriptions
mr: PR #121
start: 2026-10-18
end: 2026-10-18
---

First usable version for integrations. `pumice serve` answers OpenAI `GET /v1/models`, `GET /v1/models/{id}` and `POST /v1/audio/transcriptions` next to its dictation routes. The models are the flows listed in `serve.transcription_flows` (D5): each combines the audio model, a post-processing preset (off, a shipped preset or a user's formatter file, D2) and an AI route, and `text` is the final text. Requests take `file`, `model`, `language`, `prompt`, `temperature`, `response_format` json or text and `stream=false` (D6); a `pumice` object (and a `Pumice-Postprocessing-Status` header) carries the raw transcript, the post-processing status and timings. A failure after a successful transcription returns the raw transcript (D4, D9); a transcription or request failure returns OpenAI's error object. Dictations and transcriptions share one work permit, cancellation and cleanup path; the warm transport forwards per-request options; `pumice serve` refuses at startup a flow list whose model list could exceed its size limit. No chat route (D7). Checked end to end on server-1 with the owner's flows (raw, clean, clean-en through Claude). The browser test page (D8, #2) follows as P15.3. Design by Codex; implemented by Claude Opus 5.5 subagents; two Codex review rounds (round 1: D9, model-list size, docs; round 2: approve); final review and the live check by the orchestrator.
