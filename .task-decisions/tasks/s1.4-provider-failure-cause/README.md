---
title: Log why a provider attempt failed
state: Done
ticket: —
branch: s1.4-provider-failure-cause
mr: PR #45
start: 2026-10-06
end: 2026-10-06
---

Owner request (2026-10-06), after an unexplained `claude` nonzero exit: each failed CLI attempt captures a diagnostic through a task-local slot (no provider-interface change). The ordinary log shows only safe fields: exit code, the envelope's `api_error_status` and an identifier-only `subtype`. The opt-in debug log adds a per-attempt `trail` with a 2 KB stderr tail and stdout excerpt, which may contain dictated text.

Imported from the status tables in `HANDOFF.md`.
