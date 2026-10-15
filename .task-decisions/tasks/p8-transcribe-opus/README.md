---
title: P8: Opus audio (WebM and Ogg) for transcription
state: Done
ticket: —
branch: p8-transcribe-opus
mr: —
start: 2026-10-15
end: 2026-10-15
---

`pumice-transcribe` accepts WebM/Opus and Ogg/Opus in builds with the `opus` feature (enabled by `engine`): libopus decoding (D1), strict Ogg page and granule checks, bounded WebM validation with a stream-wide timestamp check (missing audio rejected while tolerating Chrome's timestamp jitter), and the existing 25 MB / 120 s / no-partial-results rules. The real Chrome MediaRecorder recording transcribes correctly over HTTP. Implemented by Kimi k2.8-code; three Codex review rounds, final approve with a comment nit fixed. Research: `reports/2026-10-15-opus-research.md`.
