# Decisions — phase2-plan

### D1 · Active · Phase 2 covers OpenCode, Antigravity, the generic adapter, detection and doctor
- date: 2026-10-04
- decided by: claude (orchestrator), brasillero (authorized)
- origin: conversation
- area: process
- revisit if: —

**Decision.** Phase 2 scope: OpenCode (S2.6), Antigravity (S2.5, dormant), the loopback-only generic adapter (S2.7), auto-detection and `pumice doctor` (S2.8), and a shared adapter contract suite. Kimi (S2.4) stays on standby.

**Why.** The owner was away and authorized the orchestrator to set specifications and proceed, reporting afterwards. Imported from `HANDOFF.md` (Phase 2 decisions); full list in `docs/research/phase2-architecture.md`.

**Rejected.** —

**Consequences.** Kimi was reactivated on 2026-10-06 (`phase1-plan/D11`).

### D2 · Active · New providers are off unless enabled
- date: 2026-10-04
- decided by: claude (orchestrator), brasillero (authorized)
- origin: conversation
- area: config
- revisit if: —

**Decision.** New providers are off by default: only `enabled: true` turns one on; detection never does.

**Why.** Imported from `HANDOFF.md` (Phase 2 decisions).

**Rejected.** —

### D3 · Superseded by D7 · OpenCode requires an explicit provider/model
- date: 2026-10-04
- decided by: claude (orchestrator), brasillero (authorized)
- origin: conversation
- area: providers
- revisit if: —

**Decision.** When OpenCode is enabled it needs an explicit `provider/model`; there is no hosted default.

**Why.** Imported from `HANDOFF.md` (Phase 2 decisions).

**Rejected.** —

### D4 · Active · Antigravity is built against the fake CLI only and cannot be enabled
- date: 2026-10-04
- decided by: claude (orchestrator), brasillero (authorized)
- origin: conversation
- area: providers
- revisit if: —

**Decision.** The Antigravity protocol is built against the fake CLI only; turning it on is refused until a safe per-launch tool policy exists. `agy` is never run.

**Why.** No verified way to switch its tools off. Imported from `HANDOFF.md` (Phase 2 decisions).

**Rejected.** —

### D5 · Active · The generic adapter is loopback-only and off by default
- date: 2026-10-04
- decided by: claude (orchestrator), brasillero (authorized)
- origin: conversation
- area: providers
- revisit if: —

**Decision.** The generic OpenAI-compatible adapter only calls servers on the same machine and is off by default. The owner approved merging it (2026-10-04) despite the AGENTS.md outbound-call rule; the matching exception is in AGENTS.md.

**Why.** Imported from `HANDOFF.md` (Phase 2 decisions).

**Rejected.** —

### D6 · Active · The Phase 2 gate is the contract suite on three OSes
- date: 2026-10-04
- decided by: claude (orchestrator), brasillero (authorized)
- origin: conversation
- area: process
- revisit if: —

**Decision.** Gate: the contract suite passes on Windows, Linux and macOS for every implemented adapter; Antigravity is reported as protocol coverage plus safe refusal.

**Why.** Imported from `HANDOFF.md` (Phase 2 decisions).

**Rejected.** —

### D7 · Active · OpenCode support is removed
- date: 2026-10-08
- decided by: brasillero
- origin: D3
- area: providers
- revisit if: —

**Decision.** OpenCode support is deprecated and deleted (S2.12, PR #68); a config entry `id: opencode` fails with a removal message.

**Why.** Owner decision 2026-10-08, imported from `HANDOFF.md`.

**Rejected.** —
