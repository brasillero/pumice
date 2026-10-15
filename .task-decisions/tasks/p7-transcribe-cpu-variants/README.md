---
title: P7: faster CPU transcription with runtime CPU variants
state: Done
ticket: —
branch: p7-transcribe-cpu-variants
mr: —
start: 2026-10-15
end: 2026-10-15
---

Opt-in `cpu-variants` build (Linux x86-64): the engine ships as shared libraries with one ggml CPU module per directory (`cpu/<variant>/`, D2); at startup Pumice picks the best variant the CPU and OS can run (OS-aware detection, safe fallback down to `x64`), with nothing written at runtime. Warm Turbo on the 14.19 s sample: about 7–8.5 s against about 24–28 s for the portable build on server-1, same transcript (D1 met). The default `engine` build is unchanged. Implemented by Kimi k2.8-code; four Codex review rounds, final approve. Details: `docs/research/P7-cpu-variants.md`.
