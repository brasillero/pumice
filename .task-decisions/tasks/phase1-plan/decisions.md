# Decisions — phase1-plan

### D1 · Active · New CLIs are modules behind one provider interface
- date: 2026-10-04
- decided by: brasillero
- origin: conversation
- area: providers
- revisit if: —

**Decision.** A new CLI is a new module behind a common interface (S2.1), plus one registry entry.

**Why.** Keeps providers replaceable and alike. Imported from `HANDOFF.md` (Phase 1 owner decisions).

**Rejected.** —

### D2 · Active · Codex is built in Phase 1 alongside Claude
- date: 2026-10-04
- decided by: brasillero
- origin: conversation
- area: process
- revisit if: —

**Decision.** Phase 1 has two adapters: Claude (S2.2) and Codex (S2.3), moved up from Phase 2. OpenCode, Antigravity, the generic adapter and auto-detection come later.

**Why.** Imported from `HANDOFF.md` (Phase 1 owner decisions); changes the spec's phasing, the spec is unchanged.

**Rejected.** Claude only in Phase 1 (the spec's original phasing).

### D3 · Superseded by D12 · A failed provider falls through a fallback chain
- date: 2026-10-04
- decided by: brasillero
- origin: conversation
- area: pipeline
- revisit if: —

**Decision.** After the selected provider fails, try the next in `fallback_order`, then return the raw text, all within the total timeout (S4.2 in Phase 1).

**Why.** Imported from `HANDOFF.md` (Phase 1 owner decisions).

**Rejected.** —

### D4 · Superseded by D11 · Kimi stays on standby with no adapter
- date: 2026-10-04
- decided by: brasillero
- origin: conversation
- area: providers
- revisit if: —

**Decision.** No Kimi adapter: its subscription terms allow interactive use only (S0.3).

**Why.** Terms risk. Imported from `HANDOFF.md` (Phase 1 owner decisions).

**Rejected.** —

### D5 · Active · The default port is 7567
- date: 2026-10-04
- decided by: brasillero
- origin: conversation
- area: config
- revisit if: —

**Decision.** Pumice listens on port 7567 by default (S1.5); Handy points at `http://127.0.0.1:7567/v1`.

**Why.** Imported from `HANDOFF.md` (Phase 1 owner decisions).

**Rejected.** —

### D6 · Active · Claude runs with thinking off
- date: 2026-10-04
- decided by: brasillero
- origin: conversation
- area: providers
- revisit if: —

**Decision.** Claude runs with thinking off by default (`MAX_THINKING_TOKENS=0`). Effort tuning and Handy's `reasoning_effort` field come later.

**Why.** Speed and cost for short dictations. Imported from `HANDOFF.md` (Phase 1 owner decisions).

**Rejected.** —

### D7 · Active · Pumice runs on the user's plan quota through the official CLIs
- date: 2026-10-04
- decided by: brasillero
- origin: conversation
- area: process
- revisit if: —

**Decision.** Plan quota use is intended: Pumice runs on the user's subscription through the official CLIs.

**Why.** It is the product's purpose (no API keys). Imported from `HANDOFF.md` (Phase 1 owner decisions).

**Rejected.** —

### D8 · Active · Installer, auto-update and Windows service belong to Phase 3
- date: 2026-10-04
- decided by: brasillero
- origin: conversation
- area: distribution
- revisit if: —

**Decision.** The installer, auto-update and running as a Windows service are Phase 3 work.

**Why.** Imported from `HANDOFF.md` (Phase 1 owner decisions). Distribution itself later moved to GitHub archives and npm (`s7.1-portable-releases/D1`).

**Rejected.** —

### D9 · Active · Codex runs in its most restricted mode with the residual tool risk accepted
- date: 2026-10-04
- decided by: brasillero
- origin: conversation
- area: providers
- revisit if: —

**Decision.** "Make it work first, refine its behavior later": Codex runs in its most restricted documented mode, and its residual tool risk is accepted (S5.1).

**Why.** Codex cannot switch every tool off. Imported from `HANDOFF.md` (Phase 1 owner decisions). At the time any run showing tool activity was also rejected; that check was dropped in `s3.7-no-tool-checks/D1`.

**Rejected.** —

### D10 · Superseded by D13 · Claude defaults to haiku and Codex to gpt-6.1-sol
- date: 2026-10-04
- decided by: brasillero
- origin: conversation
- area: config
- revisit if: —

**Decision.** Built-in default models: Claude `haiku`, Codex `gpt-6.1-sol`.

**Why.** Imported from `HANDOFF.md` (Phase 1 owner decisions).

**Rejected.** —

### D11 · Active · Kimi ships as an adapter
- date: 2026-10-06
- decided by: brasillero
- origin: D4
- area: providers
- revisit if: —

**Decision.** Kimi is integrated after all (S2.4, PR #52): the adapter ships, at first off by default with the terms risk warning.

**Why.** Owner reactivated Kimi on 2026-10-06. Imported from the superseded notes in `HANDOFF.md`.

**Rejected.** —

### D12 · Active · A request runs exactly one provider, with no fallback
- date: 2026-10-06
- decided by: brasillero
- origin: D3
- area: pipeline
- revisit if: —

**Decision.** The fallback chain is removed (S6.5, PR #53): a request runs exactly one provider, and any failure is answered without trying another.

**Why.** Imported from the superseded notes in `HANDOFF.md`. What a failure returns changed again in `s4.4-failure-http-error/D1`.

**Rejected.** —

### D13 · Active · No model is built in; every enabled provider sets its own
- date: 2026-10-06
- decided by: brasillero
- origin: D10
- area: config
- revisit if: —

**Decision.** S6.5 (PR #53) removes the built-in default models: every enabled provider entry sets its own `model`.

**Why.** Pumice assumes nothing. Imported from the superseded notes in `HANDOFF.md`.

**Rejected.** —
