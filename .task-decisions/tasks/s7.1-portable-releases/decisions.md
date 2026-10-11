# Decisions — s7.1-portable-releases

### D1 · Active · Pumice ships as GitHub release archives and npm packages
- date: 2026-10-05
- decided by: brasillero
- origin: conversation
- area: distribution
- revisit if: —

**Decision.** Distribution is GitHub release binaries (S7.1) plus npm packages (S7.4), installable with npm, pnpm, Bun or npx. Releases are cut by hand from the release-preview artifacts.

**Why.** Easy CLI distribution now. Imported from `HANDOFF.md` (distribution note, 2026-10-05).

**Rejected.** The Tauri installer with auto-update, deferred; the spec is unchanged.
