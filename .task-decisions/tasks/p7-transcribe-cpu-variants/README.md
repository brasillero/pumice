---
title: P7: faster CPU transcription with runtime CPU variants
state: In progress
ticket: —
branch: p7-transcribe-cpu-variants
mr: —
start: 2026-10-15
end: —
---

Make `pumice-transcribe` pick the fastest CPU code the machine supports at runtime (ggml dynamic backends), instead of the portable baseline build that takes about 19 s for the 14 s sample with Turbo. Item 4a of `p6-transcribe-http/reports/2026-10-15-implementation-order.md`. Implementation by Kimi k2.8-code has started.
