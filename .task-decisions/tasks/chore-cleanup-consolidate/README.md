---
title: Cleanup B: consolidate duplicate tests and remove stale names, comments and leftovers
state: In progress
ticket: —
branch: chore-cleanup-consolidate
mr: —
start: 2026-10-16
end: —
---

Second house-cleanup PR (plan: `chore-cleanup-test-bounds/D1`, audit `chore-cleanup-test-bounds/reports/2026-10-16-test-audit.md`, sections "Stale-test triage", "Redundancy and consolidation", "Dead/deprecated code" and proposed PRs B and C). Consolidates repeated config and API cases, gives the root fake CLI binary its own name, renames tests and fixes comments that still say "fallback" or "bare pumice", removes an unused fixture and two unneeded `allow(dead_code)`, and refreshes stale docs. Archived adapters, security tests and history stay (`chore-cleanup-test-bounds/D2`). Implementation by a Claude Opus 5.5 subagent, review by Codex, final review by the orchestrator.
