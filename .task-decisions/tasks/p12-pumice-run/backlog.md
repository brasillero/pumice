# Backlog — p12-pumice-run

## Active

## Deferred

### #1 · Deferred · Panic handling for `pumice run --json`
- type: Tech debt
- area: cli
- origin: conversation
- ticket: —
- created: 2026-10-15
- reason: no input is known to trigger a panic on the reporting path; revisit when a UI starts calling `pumice run`

**Context.** Codex review round 4: an unexpected panic under `--json` still prints Rust's default panic text on stderr (not JSONL) and gives no envelope; with a full, unread stderr the panic message can block the exit. Every panic found on the reporting path was removed in this task, but no panic hook exists.

**To do.**
- [ ] Define panic handling together with cleanup (a hook alone does not keep the result contract: children and temp dirs must still be torn down).
- [ ] Test with an injected panic and a pre-filled stderr.

**Acceptance criteria.**
- A panic under `--json` ends with a valid envelope (`internal`) or exit 1 without blocking, and no orphan processes.

**Where in the code.**
- `src/orchestrator/cli.rs` (`report`, `write_envelope`), `src/orchestrator/events.rs`

### #2 · Deferred · Run `pumice run` cancellation on Windows and macOS
- type: Task
- area: cli
- origin: D1
- ticket: —
- created: 2026-10-15
- reason: needs the owner's manual Windows check; CI runs the rest of the suite there

**Context.** The Ctrl-C/SIGTERM integration tests (no orphan transcription or provider processes, temp dirs removed, exit 130/143) are Unix-only. Windows relies on the relay's Job Object teardown through the new cleanup scope, and the scoped temp-dir retry path is only exercised on Unix.

**To do.**
- [ ] Owner: on Windows, press Ctrl-C during transcription and during the provider call; check no `pumice-transcribe` or AI CLI process is left and the temp dir is gone.
- [ ] Add a Windows console-control test if the manual check finds a gap.

**Where in the code.**
- `src/orchestrator/cancel.rs`, `crates/relay/src/process/tree.rs` (`CleanupScope`)

## Closed

### #3 · Discarded · Kimi receives the transcript on its command line
- type: Tech debt
- area: providers
- origin: conversation
- ticket: —
- created: 2026-10-15
- closed: 2026-10-29
- outcome: Kimi provider deleted (p16-provider-effort/D6)

**Context.** Other local users can read a Kimi call's transcript in the process list while it runs. Documented in `docs/orchestrator.md` (Privacy). Other providers and the formatter's instructions (a private agent file) are not affected.

**To do.**
- [ ] Switch the Kimi adapter to stdin or a file once the CLI supports it.

**Where in the code.**
- `crates/relay/src/providers/kimi.rs`
