# Backlog — chore-cleanup-test-bounds

## Active

## Deferred

### #2 · Deferred · `sleeping_fake_can_be_killed` can leave its fake running after a failure
- type: Tech debt
- area: process
- origin: reports/2026-10-16-test-audit.md
- ticket: —
- created: 2026-10-16
- reason: Codex round-3 nit on Cleanup A; the test is unchanged by it and cannot hang CI past the new step timeouts

**Context.** `crates/relay/tests/fake_cli.rs::sleeping_fake_can_be_killed` spawns a fake that sleeps 30 s and calls `wait_with_output()` without a deadline; if an assertion fails before the kill, the fake keeps running until its sleep ends.

**To do.**
- [ ] Put the child under `OwnedChild` right after spawn and bound the wait.

## Closed

### #1 · Done · A child spawned on Windows can outlive a killed `pumice` and pin its pipes
- type: Bug
- area: process
- origin: D4
- ticket: —
- created: 2026-10-16
- closed: 2026-10-16
- outcome: PR #115 (`fix-windows-spawn-race`); the upstream report is drafted for the owner in its `reports/`

**Context.** process-wrap 10.0.1 starts a Windows child suspended, assigns it to the Job Object, then resumes it. A hard kill of the parent in between leaves the child suspended outside any job, holding every inheritable handle it inherited, including the parent's stdout/stderr (CI: `children left: 8840 pumice-transcribe.exe [Suspended]`, runs 38866510968, 38867288787, 38867950120). Affects every spawn through `ProcessRunner`/`ProcessTree` (relay providers, `pumice run`, `pumice serve`). Unverified: whether Rust's `Stdio::inherit` still works for `pumice transcribe` after the originals are made non-inheritable (std duplicates the handle as inheritable, which should keep it working).

**To do.**
- [x] On Windows, clear `HANDLE_FLAG_INHERIT` on the process's standard handles at startup of `pumice` and `pumice-relay`; check `pumice transcribe`'s inherited stdio still works.
- [x] A Windows test: a child that is never resumed does not keep the caller's pipes open.
- [x] Draft upstream report for the owner to file at process-wrap: create the child inside its Job Object atomically (`PROC_THREAD_ATTRIBUTE_JOB_LIST`, Windows 10+) instead of suspend–assign–resume.
