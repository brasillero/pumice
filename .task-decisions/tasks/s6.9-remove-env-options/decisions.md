# Decisions — s6.9-remove-env-options

### D1 · Active · Each CLI keeps its own configuration
- date: 2026-10-08
- decided by: brasillero
- origin: conversation
- area: config
- revisit if: —

**Decision.** Provider `env:` and `options:` are removed: each CLI's own configuration (gateway, routing, login) is inherited, and Pumice's YAML does not duplicate it.

**Why.** Imported from `HANDOFF.md`.

**Rejected.** —
