---
title: Cleanup A: bound test waits and remove scheduling assumptions
state: Done
ticket: —
branch: chore-cleanup-test-bounds
mr: —
start: 2026-10-16
end: 2026-10-16
---

First of two house-cleanup PRs the owner asked for on 2026-10-16, based on Codex's audit of the whole test suite (`reports/2026-10-16-test-audit.md`; decisions D1–D4). Tests, test support and CI only; no production code changed. A shared test helper (`crates/relay/tests/support/bounded.rs`) gives every connect, write, framed response, process exit and output collection in the process and HTTP tests one deadline, kills and reaps owned children, caps collected output and names the last stage reached on failure; ordering sleeps became notifications or polled conditions; disconnect tests assert the fake CLI started; tight wall-clock checks use gates; the fake download server is owned and stoppable; CI test steps have finite timeouts. While doing it the cause of the Windows CI hang (`chore-small-fixes#1`) was found: a production spawn race in process-wrap (D4, #1, fixed in a separate PR). Implemented by Claude Opus 5.5 subagents; three Codex review rounds (round 1: three P2 in the helper; round 2: one P1 macOS send deadline, one P2; round 3: approve with one follow-up, #2); final review by the orchestrator. 1141 tests pass on Linux; CI green on all three platforms.
