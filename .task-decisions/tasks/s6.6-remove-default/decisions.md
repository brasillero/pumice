# Decisions — s6.6-remove-default

### D1 · Active · The client's model field picks the provider
- date: 2026-10-07
- decided by: brasillero
- origin: conversation
- area: config
- revisit if: —

**Decision.** There is no default provider: the request's `model` picks exactly one enabled provider. The `default:` key is rejected as removed.

**Why.** Imported from `HANDOFF.md`. Deviates from spec S6.2 ("an empty value falls back to the default provider"); the spec is not edited. What a request without a model gets changed in `s4.4-failure-http-error/D1`.

**Rejected.** A default provider for requests without a model.
