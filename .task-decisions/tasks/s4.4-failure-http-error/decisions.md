# Decisions — s4.4-failure-http-error

### D1 · Active · A failed request gets an OpenAI-format HTTP error
- date: 2026-10-08
- decided by: brasillero
- origin: conversation
- area: api
- revisit if: —

**Decision.** No 200 with the transcript on failure: a request without a model, with an unknown or disabled one, or whose provider fails gets an HTTP error (400/404/429/502/503/504) with a text-free reason, logged as `FAILED`. Handy and OpenWhispr then paste their own saved transcript.

**Why.** Imported from `HANDOFF.md`. Deviates from spec S4.2, S4.3 and S6.2.

**Rejected.** Returning the original transcript with HTTP 200.
