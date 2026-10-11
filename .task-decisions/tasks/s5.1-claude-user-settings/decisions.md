# Decisions — s5.1-claude-user-settings

### D1 · Active · Claude runs without --restricted so it uses the user's settings
- date: 2026-10-06
- decided by: brasillero
- origin: conversation
- area: providers
- revisit if: —

**Decision.** `--restricted` is removed because it ignored `~/.claude/settings.json`, including the gateway `env` block. `--safe-mode`, `--tools ""` and `--strict-mcp-config` stay.

**Why.** Claude fell back to the subscription instead of the user's gateway. Imported from `HANDOFF.md`.

**Rejected.** —
