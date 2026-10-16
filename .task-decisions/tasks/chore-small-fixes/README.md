---
title: Small fixes: help after relay subcommands, test binaries in installs, a flaky test
state: In progress
ticket: —
branch: chore-small-fixes
mr: —
start: 2026-10-16
end: —
---

Fixes three small deferred items in one PR: `p10-pumice-entry#3` (`cargo install --path .` also installs the test helper binaries), `p10-pumice-entry#4` (`--help` after a `pumice relay` subcommand is rejected) and `p13.2-managed-transcribe#1` (a smoke test can fail with "Text file busy"). Implementation by a Claude Opus 5.5 subagent, review by Codex, final review by the orchestrator.
