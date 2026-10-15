---
title: P6b: browser test page for transcription
state: Done
ticket: —
branch: p6b-transcribe-test-page
mr: PR #98
start: 2026-10-15
end: 2026-10-15
---

`tools/transcribe-test/`: a self-contained page that records from the microphone, builds a 16-bit mono WAV in the browser (browsers record WebM/Opus, which the server did not accept yet), sends it through a small loopback proxy to `pumice-transcribe serve` the way an OpenAI client would, and shows the transcript, timing and errors. Written by Codex; the owner used it against Turbo on server-1 over Tailscale.
