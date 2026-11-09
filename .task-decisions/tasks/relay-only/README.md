---
title: Restore Pumice as the relay only
state: In progress
ticket: —
branch: relay-only
mr: PR #140
start: 2026-11-09
end: —
---

Restores the relay as the single Pumice product at the owner's request. Removes transcription, formatter and orchestrator implementations and their delivery infrastructure, and brings the relay code and tests to the root package. Current docs and project rules follow the relay-only scope. Historical task notes remain evidence of previous decisions; live installations and user data are outside this source change.
