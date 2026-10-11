---
title: Readable request log with provider, model and fallback trail
state: Done
ticket: —
branch: s1.3-readable-request-log
mr: PR #44
start: 2026-10-06
end: 2026-10-06
---

Owner request (2026-10-06): one readable entry per request with local time, request number, outcome, the producing provider and model, total time, text length and the requested model. After a fallback or failure, one line per attempt shows provider, model, result and time. Startup prints the default route (`claude (haiku) → codex (gpt-6.1-sol)`). Colors only on a terminal (`NO_COLOR` honored; on Windows only inside Windows Terminal). Still never dictated text. Adds `jiff` for local time.

Imported from the status tables in `HANDOFF.md`.
