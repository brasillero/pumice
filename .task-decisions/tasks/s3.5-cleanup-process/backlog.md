# Backlog — s3.5-cleanup-process

## Active

## Deferred

### #1 · Deferred · The partial-stdin check misses normal-size dictations
- type: Tech debt
- area: providers
- origin: conversation
- ticket: —
- created: 2026-10-08
- reason: accepted; revisit if a CLI is seen ignoring stdin

**Context.** A CLI that exits cleanly without reading all of its stdin is treated as a failure, but the OS pipe buffer (about 64 KB) absorbs normal-size dictations, so in practice only very long ones are caught. Was a BACKLOG known limitation.

**To do.**
- [ ] Nothing unless a CLI is seen ignoring stdin.

## Closed
