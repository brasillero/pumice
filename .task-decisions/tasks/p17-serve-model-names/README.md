---
title: Generic model names for pumice serve, and a generic T3 voice demo
state: In progress
ticket: —
branch: p17-serve-model-names
mr: —
start: 2026-10-20
end: —
---

Makes `pumice serve` look like a plain OpenAI-compatible transcription server: model ids built from the real transcription model name plus, when the relay is on, the relay provider and formatter preset (`model:relay:preset`). Every setting is written in YAML, with no hidden defaults. The generic T3 demo is complete on its local-only branch (`~/scratch/t3/t3code`, `pumice-voice-demo`) and its web endpoint responds. Pumice explicit settings and models-folder discovery merged in PR #123 and PR #124. Served names, configured expansion and relay-off execution are implemented and independently approved in p17.3-served-models; local validation passed. The live configuration and models stay unchanged until the third implementation PR merges; the authorized migration follows.
