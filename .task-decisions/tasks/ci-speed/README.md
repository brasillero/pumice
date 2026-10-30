---
title: Shorter CI and release pipelines
state: In progress
ticket: —
branch: ci-speed
mr: https://github.com/brasillero/pumice/pull/133
start: 2026-10-30
end: —
---

Cut CI, Release preview and Transcription engine cost while keeping all pre-merge proof. PR #133 implements #1–#3; PR #136 implements installed SDK caching (#7) and closes compiler caching (#4) after measured trials; PR #137 implements exact-identity component reuse (#5/#8). PR #134 owns the measured CPU-variants reuse (#6). Every item has a measured outcome. The final comparison report includes cold/warm samples, queues, compiler/SDK proof, coordinator cost, cache eviction and Linux-image caveats. Task state stays In progress for the supervisor to finish at merge time.
