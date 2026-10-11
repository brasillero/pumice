---
title: Health route and opt-in debug log
state: Done
ticket: —
branch: s1.3-health-debug-log
mr: PR #20
start: 2026-10-04
end: 2026-10-04
---

S1.3 Health route: `GET /health` returns ok+version without touching the pipeline; a slow-fake test proves it answers in <300 ms during a dictation. S1.4 Debug log: `src/logging.rs` JSONL sink behind `debug_log.enabled`; records body+headers (only user-agent/content-type), raw_text, outcome, response; mode 0600 on Unix; `tests/privacy.rs` covers redaction and the disabled default.

Imported from the status tables in `HANDOFF.md`.
