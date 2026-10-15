# Backlog — p7-transcribe-cpu-variants

## Active

## Deferred

### #1 · Deferred · CPU variants on Windows and macOS
- type: Improvement
- area: distribution
- origin: D1
- ticket: —
- created: 2026-10-15
- reason: the downloadable package and Windows/macOS engine builds are not scheduled (`p6-transcribe-http/D2`)

**Context.** The `cpu-variants` mode is wired up for Linux x86-64 only. Windows needs DLL search rules and the static-runtime question; macOS on Apple Silicon has no x86 variants and would rather use Metal (GPU).

**To do.**
- [ ] Revisit together with Windows/macOS engine builds and GPU work.

## Closed
