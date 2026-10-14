---
title: P5: more audio formats for transcription
state: In progress
ticket: —
branch: p5-transcribe-codecs
mr: PR #95
start: 2026-10-14
end: —
---

Phase 5 of `arch-modules` (D11): `pumice-transcribe file` accepts MP3, FLAC and Ogg Vorbis besides WAV, detected by content and decoded with Symphonia under the same 25 MB / 120 s / 8–192 kHz / mono-or-stereo limits; Ogg Opus, MP1/MP2, chained Ogg and mid-stream format changes are rejected with typed errors. Implemented by Kimi (`kimi-k2.7-code-highspeed`), reviewed and revised by Codex (one round, fixes in `7346997`), final review by the orchestrator: approved. PR #95 is open; waiting for CI and the owner's answer on truncated MP3 files without a frame count.
