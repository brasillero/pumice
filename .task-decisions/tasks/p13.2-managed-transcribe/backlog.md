# Backlog — p13.2-managed-transcribe

## Active

## Deferred

## Closed

### #1 · Done · A smoke test can fail with "Text file busy"
- type: Bug
- area: process
- origin: conversation
- ticket: —
- created: 2026-10-15
- closed: 2026-10-16
- outcome: PR #111 (`chore-small-fixes`): the cause was executables copied by the test process while other threads spawn; tests now hard-link them or copy through a `cp` child

**Context.** During Codex's round-3 review of P13.2, one run of `cargo test --workspace --locked` failed in an existing smoke test with `Text file busy` (`ETXTBSY`), then passed when rerun serially. Unverified: which test it was (Codex named it only as an unchanged smoke test). This is the usual race when a test writes an executable script and runs it while another thread forks with the file's write descriptor still open.

**To do.**
- [x] Find the test (likely in `tests/pumice_smoke.rs` or a fake-CLI helper that writes scripts).
- [x] Close the script's write descriptor before any spawn can inherit it, or write it once before tests run in parallel.
