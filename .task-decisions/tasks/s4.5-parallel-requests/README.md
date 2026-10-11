---
title: Parallel requests with a queue
state: Done
ticket: —
branch: s4.5-parallel-requests
mr: PR #78
start: 2026-10-08
end: 2026-10-08
---

Owner request (2026-10-08, friends testing over the public link): up to `max_parallel` (default 4, max 32) provider runs at once; extra requests wait FIFO within their time budget, then get 503. Implemented by Kimi, reviewed by Claude and Codex.

Imported from the status tables in `HANDOFF.md`.
