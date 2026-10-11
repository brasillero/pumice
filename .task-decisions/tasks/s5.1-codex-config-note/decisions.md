# Decisions — s5.1-codex-config-note

### D1 · Active · Codex inherits the user's own config
- date: 2026-10-05
- decided by: brasillero
- origin: conversation
- area: providers
- revisit if: —

**Decision.** Codex runs without `--ignore-user-config`, so its global configuration (gateway, base URL) applies, instead of duplicating that URL in Pumice's YAML. Tool restrictions and fresh temporary directories stay.

**Why.** Phase 0 showed the flag drops the user's routing. Imported from `HANDOFF.md`; implemented in PR #48.

**Rejected.** Keeping the flag permanently (the earlier plan).
