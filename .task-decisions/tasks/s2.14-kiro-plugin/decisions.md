# Decisions — s2.14-kiro-plugin

### D1 · Active · The Kiro plugin is rewritten from scratch
- date: 2026-10-10
- decided by: brasillero
- origin: s2.13-archive-providers/D1
- area: providers
- revisit if: —

**Decision.** The archived S2.9 adapter is deleted unread and Kiro becomes a registered plugin again, written from scratch.

**Why.** Imported from `HANDOFF.md`.

**Rejected.** Reviving the archived adapter.

### D2 · Active · Kiro gets the system prompt from a one-off agent file in the temp directory
- date: 2026-10-10
- decided by: brasillero
- origin: conversation
- area: providers
- revisit if: —

**Decision.** The plugin writes a no-tools agent file (`.kiro/agents/pumice.json`) carrying the client's system prompt into the call's temporary directory. This is an approved exception to AGENTS.md security rule 3, which now names it.

**Why.** Kiro has no system-prompt flag. Imported from `HANDOFF.md`.

**Rejected.** —

### D3 · Provisional · Kiro runs every model with --effort low
- date: 2026-10-11
- decided by: brasillero
- origin: conversation
- area: providers
- revisit if: a source of per-model effort levels exists (`setup-wizard#3`)

**Decision.** The Kiro plugin sends `--effort low` for every model.

**Why.** Works for all current models; models without effort levels print a warning and run normally. Owner decision in conversation.

**Rejected.** Per-model lowest effort (needs a source of per-model levels).
