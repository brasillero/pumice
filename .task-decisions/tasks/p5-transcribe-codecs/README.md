---
title: P5: more audio formats for transcription
state: In progress
ticket: —
branch: p5-transcribe-codecs
mr: —
start: 2026-10-14
end: —
---

Phase 5 of `arch-modules` (D11): `pumice-transcribe file` accepts MP3, FLAC and Ogg Vorbis besides WAV, decoded with Symphonia under the same 25 MB / 120 s limits, each format backed by fixtures. Kimi (`kimi-k2.7-code-highspeed`) writes the code, Codex reviews and revises Kimi's work, and the orchestrator does the final review. Implementation starting.
