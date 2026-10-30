---
title: Shorter CI and release pipelines
state: In progress
ticket: —
branch: ci-speed
mr: —
start: 2026-10-30
end: —
---

Cut CI, Release preview and Transcription engine wall time and runner minutes without weakening what they prove (three-OS tests, static linkage, bundle validation, npm install smokes, notices). Codex measured the pipelines and ranked the changes (`reports/2026-10-30-pipeline-speed.md`); the dependency caches are empty on every job, so fixing them comes first. Nothing is implemented yet; #5 and #8 need owner decisions.
