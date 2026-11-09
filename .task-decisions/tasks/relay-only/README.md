---
title: Restore Pumice as the relay only
state: Done
ticket: —
branch: relay-only
mr: PR #140
start: 2026-11-09
end: 2026-11-09
---

Restored the relay as the single root Pumice package, executable and settings file. Removed transcription, formatter, orchestration and their delivery infrastructure. Current docs and project rules describe the relay-only scope. All 16 cross-platform and packaging checks passed on `645a426`; these notes are finished before merging PR #140. Historical task notes remain evidence; live installations and user data were not changed.
