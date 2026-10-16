---
title: Cleanup A: bound test waits and remove scheduling assumptions
state: In progress
ticket: —
branch: chore-cleanup-test-bounds
mr: —
start: 2026-10-16
end: —
---

First of two house-cleanup PRs the owner asked for on 2026-10-16, based on Codex's audit of the whole test suite (`reports/2026-10-16-test-audit.md`; decisions D1–D3). This PR fixes the audit's W1–W5: waits in process and HTTP tests that can hang CI (including the likely cause of the Windows hang in `chore-small-fixes#1`), ordering sleeps and missing prerequisite assertions, tight wall-clock thresholds, the fake download server's lifetime, and a finite CI test-step timeout. Cleanup B (consolidation and stale names, comments and docs) follows. Implementation by a Claude Opus 5.5 subagent, review by Codex, final review by the orchestrator.
