# Decisions — s2.13-archive-providers

### D1 · Superseded by D2 · Kiro, Antigravity and the generic adapter are archived
- date: 2026-10-08
- decided by: brasillero
- origin: conversation
- area: providers
- revisit if: —

**Decision.** These adapters are not registered until the plugin architecture settles; their code and adapter-level tests stay, and a config entry fails with an archived message.

**Why.** Focus on Claude, Codex and Kimi first. Imported from `HANDOFF.md`. Kiro returned as a rewrite in `s2.14-kiro-plugin/D1`.

**Rejected.** —

### D2 · Active · Delete the generic adapter
- date: 2026-11-11
- decided by: brasillero
- origin: D1
- area: providers
- revisit if: the owner explicitly requests an HTTP provider

**Decision.** Delete the generic adapter, its tests and its unused dependencies. Antigravity remains removed under p16-provider-effort/D6; Kiro remains the official-CLI rewrite under s2.14-kiro-plugin/D1.

**Why.** The owner explicitly requested deleting the archived adapter during the relay repository cleanup.

**Rejected.** Keeping the generic adapter pending the plugin contract.
