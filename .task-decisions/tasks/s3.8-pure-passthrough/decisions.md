# Decisions — s3.8-pure-passthrough

### D1 · Active · Pumice adds no instructions to the request
- date: 2026-10-08
- decided by: brasillero
- origin: conversation
- area: pipeline
- revisit if: —

**Decision.** No fixed instruction, no YAML prompts and no `<transcript>` envelope: the system prompt is the client's own. Only Codex and Kimi, which refuse an empty system prompt, get the neutral line `Follow the user's message.`

**Why.** Pumice is plumbing; the app owns the task. Imported from `HANDOFF.md`; deviates from spec S3.1–S3.3.

**Rejected.** —

### D2 · Active · The app's system prompt goes to the CLI's system prompt when it has one
- date: 2026-10-11
- decided by: brasillero
- origin: D1
- area: providers
- revisit if: —

**Decision.** When a CLI supports a system prompt, the client's system message goes there; otherwise the user message is sent as is.

**Why.** Owner confirmation in conversation; the plugins already work this way.

**Rejected.** —
