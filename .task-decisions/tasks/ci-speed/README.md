---
title: Shorter CI and release pipelines
state: Done
ticket: —
branch: ci-speed
mr: https://github.com/brasillero/pumice/pull/133
start: 2026-10-30
end: 2026-10-30
---

CI, Release preview and Transcription engine are cheaper with all pre-merge proof kept. PR #133 repaired the Cargo caches, added them to the core release builds, cancels superseded PR runs and sped up the npm registry fixture and the Windows dependency check (#1–#3). PR #136 caches the installed Vulkan SDKs (#7); the native compiler cache was trialled and discarded (#4). PR #134 reuses one native install across the CPU-variants job's profiles (#6). PR #137 lets Release previews reuse the same PR head's transcription bundles when their exact build identity matches, and builds locally otherwise (#5, #8). Measurements, cache budget and remaining limits are in `reports/2026-10-30-comparison.md`.
