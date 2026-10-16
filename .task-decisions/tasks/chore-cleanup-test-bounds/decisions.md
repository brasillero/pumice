# Decisions — chore-cleanup-test-bounds

### D1 · Active · The house cleanup is two PRs: test bounds first, then consolidation and stale text
- date: 2026-10-16
- decided by: claude (orchestrator, owner-delegated 2026-10-16)
- origin: reports/2026-10-16-test-audit.md
- area: process
- revisit if: —

**Decision.** Cleanup A (this task) does the audit's W1–W5: one deadline for every connect, write, framed response, process exit and output collection in the process and HTTP tests; notifications or polled conditions instead of ordering sleeps; prerequisite assertions in the disconnect tests; gated or virtual-time checks instead of tight wall-clock thresholds; an owned, stoppable fake download server; and a finite timeout on CI test steps. Cleanup B combines the audit's PRs B and C: consolidating duplicate config and API cases, a unique name for the root fake CLI binary, renaming tests and fixing comments that use old "fallback"/"bare pumice" wording, removing the unused `windows-shims/codex.ps1` fixture and two unneeded `allow(dead_code)`, and refreshing stale docs.

**Why.** The audit found no test that forces a removed behavior; the real risks are hangs and flakiness, so those go first. B and C are both low risk and small.

**Rejected.** Three PRs (B and C are too small apart); deleting suites because of their names.

### D2 · Active · Archived adapters, security tests and historical documents stay
- date: 2026-10-16
- decided by: claude (orchestrator, owner-delegated 2026-10-16)
- origin: reports/2026-10-16-test-audit.md
- area: process
- revisit if: the owner decides to delete the archived providers

**Decision.** The cleanup keeps the archived Antigravity and generic adapters and their tests (`s2.13-archive-providers/D1`), every test that protects a security rule (no tools, private temp dirs, no content logging, loopback only, credentials never read), and the strict rejection tests for removed config keys (they are not compatibility shims). Old research and append-only decisions are not rewritten; a short dated notice pointing to the current decisions may be added to historical proposals.

**Why.** Owner decisions keep the archive; security coverage is non-negotiable; history stays traceable.

**Rejected.** Removing tests that only look redundant because they overlap a security check.

### D3 · Active · The untracked `spikes/` folder is left to the owner
- date: 2026-10-16
- decided by: claude (orchestrator, owner-delegated 2026-10-16)
- origin: reports/2026-10-16-test-audit.md
- area: process
- revisit if: the owner confirms it can be deleted

**Decision.** `spikes/` in the main checkout is untracked (about 110 MB, mostly `spikes/stt/target` build output) and nothing references it. It is not deleted by the cleanup; the owner decides.

**Why.** Untracked local files may hold work git has no copy of; the S10.1 evidence lives on the retained branch (`arch-modules/D15`).

**Rejected.** Deleting it as part of the cleanup.

### D4 · Active · Fix the Windows spawn race in Pumice, and leave the upstream report to the owner
- date: 2026-10-16
- decided by: claude (orchestrator, owner-delegated 2026-10-16)
- origin: conversation
- area: process
- revisit if: process-wrap creates children directly inside their Job Object

**Decision.** The Windows CI hang (`chore-small-fixes#1`) is a production race, not only a test bug: process-wrap 10.0.1 creates a child suspended, assigns it to the Job Object, then resumes it (`src/tokio/job_object.rs`, `wrap_child`). If the parent is killed between creation and assignment, the child stays suspended outside any job and keeps every inheritable handle it got, including the parent's own stdout and stderr. Pumice fixes its side in a separate PR (#1): its standard handles are made non-inheritable at startup on Windows, so an orphan cannot pin its caller's pipes, with a test. An upstream report to process-wrap (create the child inside the job with `PROC_THREAD_ATTRIBUTE_JOB_LIST`) is drafted in that item for the owner to file; the orchestrator does not post to external trackers.

**Why.** The window is tiny, but its effect is a stuck caller (seen twice on CI). AGENTS.md and the owner's rules prefer durable fixes; patching the vendored crate is not one.

**Rejected.** Fixing only the tests (Cleanup A already makes them wait for the child to run, but production stays exposed); patching process-wrap locally.
