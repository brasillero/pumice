# Backlog — chore-small-fixes

## Active

## Deferred

### #1 · Deferred · A dictation test can hang on Windows CI
- type: Bug
- area: process
- origin: conversation
- ticket: —
- created: 2026-10-16
- reason: seen once; the next Windows run of the same test passed; needs Windows CI iterations to reproduce

**Context.** On PR #111's CI run 38856360617 (commit 1bc5c5e), the Windows `cargo test` step hung for over 50 minutes in `tests/serve_dictations.rs::an_unready_service_refuses_before_reading_the_body` (last line: "has been running for over 60 seconds"; the run was cancelled). The test passed on Windows in PR #110 and again on run 38860779436. The test crashes the first fake child and holds the second in a 60 s `ready_delay_ms`, then sends a partial upload and expects a 503 before the body is read. Unverified: whether the hang is in the 503 read, in `stop()` while the second child is still loading, or in `assert_children_gone`.

**To do.**
- [ ] Bound every wait in the test so a failure is an assertion, not a hang (and add a per-test timeout to the slow serve tests).
- [ ] Reproduce on Windows CI and fix the cause.


## Closed
