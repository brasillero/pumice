---
title: Shorter CI and release pipelines
state: In progress
ticket: —
branch: ci-speed
mr: https://github.com/brasillero/pumice/pull/133
start: 2026-10-30
end: —
---

Cut CI, Release preview and Transcription engine wall time and runner minutes without weakening what they prove (three-OS tests, static linkage, bundle validation, npm install smokes, notices). Codex measured the pipelines and ranked the changes (`reports/2026-10-30-pipeline-speed.md`); the dependency caches are empty on every job, so fixing them comes first. The owner authorized every item (D1); PR #133 implements #1–#3; PR #136 adds #4 and #7; PR #137 implements exact-identity component reuse for #5/#8. PR #134 owns #6 separately. The comparison report records completed samples and explicitly pending warm/sharing measurements; full CI validation is still in progress.
