# Backlog — s2.13-archive-providers

## Active

## Deferred

## Closed

### #1 · Done · Antigravity and the generic adapter stay archived
- type: Tech debt
- area: providers
- origin: D1
- ticket: —
- created: 2026-10-08
- closed: 2026-11-11
- outcome: D2; generic removed, Antigravity already removed under p16-provider-effort/D6

**Context.** Code and tests stay, but they are not registered and a config entry fails with a message saying so. Antigravity has never run against the real CLI. Was a BACKLOG known limitation.

**To do.**
- [x] Bring back, or delete, once the plugin contract settles (`plugin-contract#2`).
