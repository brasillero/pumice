---
title: List and select configured providers through model IDs
state: Done
ticket: —
branch: s1.2-provider-models
mr: PR #19
start: 2026-10-04
end: 2026-10-04
---

`Pipeline::model_ids` (default first) and `Pipeline::select` (trimmed, case-insensitive) are the single source of truth for listing, selection and the response `model`; covered end to end over HTTP with two fakes.

Imported from the status tables in `HANDOFF.md`.
