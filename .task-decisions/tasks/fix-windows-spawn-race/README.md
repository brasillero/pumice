---
title: Windows: a child spawned during a hard kill must not pin the caller's pipes
state: Done
ticket: —
branch: fix-windows-spawn-race
mr: PR #115
start: 2026-10-16
end: 2026-10-16
---

Fixed `chore-cleanup-test-bounds#1` per `chore-cleanup-test-bounds/D4`. On Windows, `pumice` and `pumice-relay` now clear `HANDLE_FLAG_INHERIT` on their standard handles first thing in `main` (`pumice_relay::process::make_std_handles_non_inheritable`), so a child orphaned in process-wrap's suspend–assign–resume window can no longer pin the caller's stdout/stderr. `Stdio::inherit()` still works for `pumice transcribe` because std duplicates the handle as inheritable for the child; sockets, log and temp files and the parent ends of child pipes were already non-inheritable. A Windows test kills `pumice` while its child lives and requires the pipes to reach end of file; with the fix disabled it failed on CI (run 38881909906), with it all platforms pass (run 38882661354). The orphan itself can still remain until reboot; only process-wrap can remove it, and a draft upstream report is in `reports/2026-10-16-process-wrap-upstream-report.md` for the owner to file. Implemented by a Claude Opus 5.5 subagent; one Codex review round (approve with two P3: report wording, fixed by the orchestrator, and a test cleanup race, #1); final review by the orchestrator.
