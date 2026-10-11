# Decisions — s6.5-provider-list

### D1 · Active · Providers are an explicit ordered list
- date: 2026-10-07
- decided by: brasillero
- origin: conversation
- area: config
- revisit if: —

**Decision.** `providers:` is an explicit ordered list; each entry has `enabled`, and `model` when enabled. Nothing is enabled implicitly; the old keys are rejected as removed.

**Why.** Configuration format 0.2. Imported from `HANDOFF.md`. The fallback and default-model changes in this PR are recorded in `phase1-plan/D12` and `phase1-plan/D13`.

**Rejected.** —
