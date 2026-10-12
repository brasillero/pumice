---
title: S10.1: speech-to-text transcription layer
state: In progress
ticket: —
branch: s10.1-transcription
mr: —
start: 2026-10-12
end: —
---

A transcription layer for Pumice: audio in, text out, through an OpenAI-compatible `POST /v1/audio/transcriptions`, independent of the relay. CPU only for now. Research phase: the leading engine candidate is `transcribe-cpp`, the crate Handy uses (#1); the test model and its source are still open (#2, #3).
