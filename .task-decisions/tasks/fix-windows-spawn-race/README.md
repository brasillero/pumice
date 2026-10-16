---
title: Windows: a child spawned during a hard kill must not pin the caller's pipes
state: In progress
ticket: —
branch: fix-windows-spawn-race
mr: —
start: 2026-10-16
end: —
---

Fixes `chore-cleanup-test-bounds#1` per `chore-cleanup-test-bounds/D4`. On Windows, process-wrap creates a child suspended, assigns it to the Job Object and then resumes it; a hard kill of the parent in between leaves the child suspended outside any job, holding every inheritable handle, including the parent's own stdout and stderr, so whoever reads them waits forever (two Windows CI hangs). This PR makes the process's standard handles non-inheritable at startup on Windows, so such an orphan can no longer pin the caller's pipes, and drafts an upstream report for the owner. Implementation by a Claude Opus 5.5 subagent, review by Codex, final review by the orchestrator.
