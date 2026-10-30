---
title: Shorter CI and release pipelines
state: In progress
ticket: —
branch: ci-speed
mr: —
start: 2026-10-30
end: —
---

Cut CI, Release preview and Transcription engine wall time and runner minutes without weakening what they prove (three-OS tests, static linkage, bundle validation, npm install smokes, notices). Codex measured the pipelines and ranked the changes (`reports/2026-10-30-pipeline-speed.md`); the dependency caches are empty on every job, so fixing them comes first. The owner authorized every item (D1); implementation is under way, followed by a before/after comparison.
