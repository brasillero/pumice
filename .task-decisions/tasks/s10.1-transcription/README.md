---
title: S10.1: speech-to-text transcription layer
state: In progress
ticket: —
branch: s10.1-transcription
mr: —
start: 2026-10-12
end: —
---

A transcription layer for Pumice: audio in, text out, through an OpenAI-compatible `POST /v1/audio/transcriptions`, independent of the relay. CPU only for now. Research phase: the leading engine candidate is `transcribe-cpp`, the crate Handy uses (#1); the test model is Large v3 Turbo Q8_0, the same as the owner's Handy; how users get the model is still open (#3).
