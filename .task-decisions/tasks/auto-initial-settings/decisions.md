# Decisions — auto-initial-settings

### D1 · Active · Create the initial settings files automatically on first start
- date: 2026-10-21
- decided by: brasillero
- origin: p17.1-explicit-config/D1
- area: config
- revisit if: —

**Decision.** Pumice creates the initial settings files automatically on first start, using its supplied starting configuration. Users do not need to copy the examples before starting.

**Why.** The owner expects the settings files to be present for transcription and chose automatic creation when asked how first-time setup should work.

**Rejected.** Requiring users to copy the supplied examples manually before first use.

**Consequences.** Creation happens before normal file validation. Existing files are preserved, and missing settings inside an existing file remain errors rather than receiving hidden defaults. This does not download models or log in to AI services.
