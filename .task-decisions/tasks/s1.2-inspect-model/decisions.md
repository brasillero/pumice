# Decisions — s1.2-inspect-model

### D1 · Active · A built-in inspect model echoes the request
- date: 2026-10-05
- decided by: brasillero
- origin: conversation
- area: api
- revisit if: —

**Decision.** The built-in `inspect` model returns the full JSON request body verbatim, without calling any AI; it is listed after `passthrough`.

**Why.** Owner-approved diagnostic scope addition. Imported from `HANDOFF.md`; see `docs/research/S1.2-inspect-model.md`.

**Rejected.** —
