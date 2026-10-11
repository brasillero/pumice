# Backlog — s2.4-kimi-adapter

## Active

## Deferred

### #1 · Deferred · Kimi gets the dictation as a command-line argument
- type: Tech debt
- area: providers
- origin: conversation
- ticket: —
- created: 2026-10-06
- reason: until the Kimi CLI accepts stdin or a prompt file

**Context.** Kimi's print mode has no stdin or file input, so the dictation travels as one argument (visible to other local processes while the call runs), capped at 24 KiB and at the Windows command-line limit. See `docs/research/S2.4-kimi-adapter.md`. Was a BACKLOG known limitation.

**To do.**
- [ ] Switch transport if Kimi adds stdin or a prompt file.

## Closed
