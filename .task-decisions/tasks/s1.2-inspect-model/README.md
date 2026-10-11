---
title: Add inspect model for complete request JSON
state: Done
ticket: —
branch: s1.2-inspect-model
mr: PR #43
start: 2026-10-05
end: 2026-10-05
---

Built-in `inspect` echoes the full JSON request body verbatim, bypassing extraction, AI calls, cleanup, fallback and the busy guard. Listed after `passthrough` in `/v1/models` with case-insensitive selection. Docs and tests updated. Owner-approved diagnostic scope addition; see `docs/research/S1.2-inspect-model.md`.

Imported from the status tables in `HANDOFF.md`.
