# Backlog — s2.14-cheapest-settings

## Active

## Deferred

## Closed

### #1 · Discarded · Kimi thinking off could break a thinking-only model
- type: Tech debt
- area: providers
- origin: D1
- ticket: —
- created: 2026-10-08
- closed: 2026-10-29
- outcome: Kimi provider deleted (p16-provider-effort/D6)

**Context.** Pumice forces `KIMI_MODEL_THINKING_EFFORT=off`. Kimi 2.1.1 sends it even for models that must think; such a backend could reject every call (an HTTP error, logged). The owner's models (k2.7-code-highspeed, k2.8) offer thinking off. Routes through non-Kimi providers ignore it. Was a BACKLOG known limitation.

**To do.**
- [ ] Pin the lowest effort instead if a thinking-required model is used.
