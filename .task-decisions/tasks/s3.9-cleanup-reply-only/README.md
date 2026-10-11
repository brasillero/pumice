---
title: Cleanup reads only the reply
state: Done
ticket: —
branch: s3.9-cleanup-reply-only
mr: PR #77
start: 2026-10-08
end: 2026-10-08
---

Owner decision (2026-10-08): cleanup no longer consults the dictation, and an unclosed `<think>` tag is kept as text instead of failing; `CleanupError::UnclosedReasoning` removed.

Imported from the status tables in `HANDOFF.md`.
