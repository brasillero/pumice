---
title: Generic model names for pumice serve, and a generic T3 voice demo
state: In progress
ticket: —
branch: p17-serve-model-names
mr: —
start: 2026-10-20
end: —
---

Makes `pumice serve` look like a plain OpenAI-compatible transcription server: model ids built from the real transcription model name plus, when the relay is on, the relay provider and formatter preset (`model:relay:preset`). Every setting is written in YAML, with no hidden defaults. The local T3 Code voice demo (`~/scratch/t3/t3code`, branch `pumice-voice-demo`, outside this repository) becomes generic first. Designed in conversation; T3 demo work starting; the Pumice part waits for #2.
