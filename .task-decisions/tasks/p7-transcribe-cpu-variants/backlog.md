# Backlog — p7-transcribe-cpu-variants

## Active

## Deferred

## Closed

### #1 · Discarded · CPU variants on Windows and macOS
- type: Improvement
- area: distribution
- origin: D1
- ticket: —
- created: 2026-10-15
- closed: 2026-11-11
- outcome: the feature this item belongs to was removed (`relay-only/D1`); the owner asked on 2026-11-11 to clear stale material

**Context.** The `cpu-variants` mode is wired up for Linux x86-64 only. Windows needs DLL search rules and the static-runtime question; macOS on Apple Silicon has no x86 variants and would rather use Metal (GPU).

**To do.**
- [ ] Revisit together with Windows/macOS engine builds and GPU work.
