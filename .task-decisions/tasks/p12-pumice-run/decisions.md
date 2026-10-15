# Decisions — p12-pumice-run

### D1 · Active · `pumice run` spawns the transcription program and runs the formatter and relay in-process
- date: 2026-10-15
- decided by: claude (orchestrator, owner-delegated 2026-10-15)
- origin: reports/2026-10-15-flow-design.md
- area: architecture
- revisit if: a UI needs warm (already loaded) transcription for frequent dictations

**Decision.** `pumice run` spawns the sibling `pumice-transcribe file --json` (found next to the real `pumice` executable, never via PATH) and calls the formatter and relay libraries in-process for one request; it starts no server and listens on no port. HTTP transports (a running transcription server or relay) are later, additive options with the same result contract.

**Why.** Follows arch-modules/D13 (transcription separate, relay and formatter built in), needs no readiness checks or port management, cannot touch the live relay, and lets `pumice run` cancel the provider CLI directly. The cost is a cold model load per run.

**Rejected.** Calling a running relay over HTTP (recommended earlier in `p6-transcribe-http/reports/2026-10-15-implementation-order.md` §5): needs a running service and can only cancel the connection. Linking the transcription engine into `pumice`: breaks the static core (arch-modules/D13).

### D2 · Active · The flow owns one overall deadline that stages can only shorten
- date: 2026-10-15
- decided by: claude (orchestrator, owner-delegated 2026-10-15)
- origin: plugin-contract#6
- area: process
- revisit if: an HTTP flow endpoint or long-running supervision is added

**Decision.** For `pumice run` only: one absolute deadline from command start; each stage's deadline is the earlier of the remaining overall budget and its own cap; module limits (relay timeouts) can shorten but never extend it; 2.5 s are reserved for cleanup and output; no retries. The general policy in plugin-contract#6 stays open for other entry points.

**Why.** A single clock gives callers a predictable upper bound and prevents a slow transcription from giving the paid AI call an unbounded budget.

**Rejected.** Resetting the budget at each stage; letting each module own its own deadline only.

### D3 · Active · The raw transcript is sent to the relay as literal text
- date: 2026-10-15
- decided by: claude (orchestrator, owner-delegated 2026-10-15)
- origin: reports/2026-10-15-flow-design.md
- area: pipeline
- revisit if: —

**Decision.** `pumice run` builds the relay request itself from the raw transcript and does not run Handy's `<transcript>` envelope extraction on it; the raw transcript is kept byte for byte and reported separately from the final text, also when a later stage fails.

**Why.** Dictation may literally contain tags; the transcript must never be reinterpreted or lost when the AI step fails.

**Rejected.** Reusing the HTTP request extraction for dictated text.

### D4 · Active · A flow without a formatter adds no instructions but still names its relay route
- date: 2026-10-15
- decided by: claude (orchestrator, owner-delegated 2026-10-15)
- origin: reports/2026-10-15-flow-design.md
- area: config
- revisit if: the owner wants raw-only flows without a relay entry

**Decision.** `formatter: null` means no system message is added; `relay_route` is always explicit. A raw-only flow is `formatter: null` with `relay_route: passthrough`. The `inspect` route and a formatter combined with `passthrough` are rejected.

**Why.** Nothing is enabled implicitly (AGENTS.md); a missing formatter must never silently choose or skip a paid route.

**Rejected.** Treating a missing formatter as "raw transcription only".

### D5 · Active · Flow budgets have built-in defaults
- date: 2026-10-15
- decided by: claude (orchestrator, owner-delegated 2026-10-15)
- origin: reports/2026-10-15-flow-design.md
- area: config
- revisit if: real use shows the defaults are wrong for long recordings

**Decision.** `budgets_ms` in a flow is optional; defaults are total 180000, transcription 120000, formatter 1000, relay 60000 ms. Flows still need `enabled: true` and explicit module and route references.

**Why.** AGENTS.md: global settings have built-in defaults; only enabling things must be explicit.

**Rejected.** Required budgets in every flow (the design's proposal).

### D6 · Active · `pumice run` takes its config with `--config`
- date: 2026-10-15
- decided by: claude (orchestrator, owner-delegated 2026-10-15)
- origin: reports/2026-10-15-flow-design.md
- area: cli
- revisit if: —

**Decision.** The orchestrator config is passed with `--config <FILE>`; without it, `orchestrator.yml` is read from the per-user config directory (XDG/`~/.config/pumice` on Linux and macOS, `%APPDATA%\pumice` on Windows).

**Why.** Same flag name as `pumice-transcribe` and the other modules.

**Rejected.** `--stack-config` (the design's proposal).

### D7 · Active · `pumice run --json` returns one versioned envelope with raw and final text apart
- date: 2026-10-15
- decided by: claude (orchestrator, owner-delegated 2026-10-15)
- origin: arch-modules/D10
- area: cli
- revisit if: a UI needs streaming results

**Decision.** With `--json`, stdout carries exactly one `schema_version: 1` document: `transcript` (raw), `result` (final, only on success), per-stage status and timings, and on failure a stable `error.code` and `error.stage`; `retryable` is always false. Exit codes: 0 success, 2 invalid input or config, 1 runtime failure, 130 Ctrl-C, 143 SIGTERM. Stderr carries only metadata events (started, stage started/finished, finished), never text.

**Why.** UIs build on the CLI (arch-modules/D10); a failed AI step must still give the user their dictation; paid calls are never replayed automatically.

**Rejected.** Printing the raw transcript as the final text on failure.

### D8 · Active · `pumice run` lands as one PR with the relay changes first
- date: 2026-10-15
- decided by: claude (orchestrator, owner-delegated 2026-10-15)
- origin: reports/2026-10-15-flow-design.md
- area: process
- revisit if: the child-process cleanup hits a cross-platform blocker

**Decision.** One PR, in ordered commits: relay caller-deadline and cleanup-scope seam, orchestrator config, the `run` command and envelope, docs. If the cleanup seam hits a Windows or macOS blocker it moves to its own PR; lifecycle tests are never weakened to land it.

**Why.** Keeps the change reviewable while the pieces depend on each other.

**Rejected.** —

### D9 · Active · Paths in the `pumice run` envelope are lossy text
- date: 2026-10-15
- decided by: claude (orchestrator, owner-delegated 2026-10-15)
- origin: conversation
- area: cli
- revisit if: a UI needs to reopen a reported file by its exact native path

**Decision.** Every path in the `--json` envelope (today only `error.location.file`) is written as a string; bytes that are not valid UTF-8 (Unix) or unpaired surrogates (Windows) become U+FFFD. Writing the envelope never panics: if it cannot be serialized or written, the command exits 1 and writes nothing to stderr.

**Why.** Codex review round 3 showed a non-UTF-8 `--config` path made the envelope panic and could hang the exit on a full stderr. The path is metadata shown to the user, so a lossy form loses nothing that matters.

**Rejected.** Encoding native paths as byte arrays (awkward for UIs); dropping the location for such paths.

### D10 · Active · A failed run does not store its transcript
- date: 2026-10-15
- decided by: claude (orchestrator, owner-delegated 2026-10-15)
- origin: D7
- area: cli
- revisit if: users lose dictations because they ran without `--json`

**Decision.** On failure the raw transcript is only in that run's `--json` envelope. Plain mode says so, and says that rerunning repeats the whole flow (a new transcription and possibly another paid provider call); it never claims the transcript was preserved. No transcript is written to disk.

**Why.** The first wording promised a recovery that does not exist, and a rerun can spend quota. Storing dictations would need its own privacy decision (AGENTS.md security rule 5).

**Rejected.** Saving failed transcripts to a file; printing the raw transcript as the result on failure (already rejected in D7).
