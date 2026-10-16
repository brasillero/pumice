# Backlog — fix-windows-spawn-race

## Active

## Deferred

### #1 · Deferred · The Windows pipe test can leave its stub waiting after cleanup
- type: Tech debt
- area: process
- origin: conversation
- ticket: —
- created: 2026-10-16
- reason: Codex review P3; the stub's own 60 s deadline already prevents an indefinite leak

**Context.** In `tests/pumice_smoke.rs::a_child_outliving_a_killed_pumice_does_not_hold_its_pipes` (Windows only) the cleanup guard writes the `release` file and then the temporary directory is deleted without waiting for the stub, so the stub can miss the file and run until its 60 s deadline.

**To do.**
- [ ] Keep an owned handle to the stub's PID and wait for it to exit, with a deadline, after writing `release` and before deleting the directory.


## Closed
