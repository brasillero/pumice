---
title: P8b: test page sends the browser's own WebM/Opus recording
state: Done
ticket: —
branch: p8b-test-page-native
mr: PR #101
start: 2026-10-15
end: 2026-10-15
---

The browser test page (`tools/transcribe-test/`) now records with MediaRecorder and sends the browser's own WebM/Opus by default, exactly as an app like T3 would, with the in-page WAV path kept as an option and an automatic fallback. Written by Kimi k2.8-code; the supervisor's review fixed a dropped final chunk and stops WebM recordings at 119 s to stay under the server's 120 s limit.
