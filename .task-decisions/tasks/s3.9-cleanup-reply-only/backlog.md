# Backlog — s3.9-cleanup-reply-only

## Active

## Deferred

### #1 · Deferred · Decide whether any reply cleanup stays
- type: Alignment
- area: pipeline
- origin: conversation
- ticket: —
- created: 2026-10-07
- reason: waiting for the owner's decision

**Context.** Cleanup is minimal (`s3.6-minimal-cleanup/D1`, `D1`): drop closed `<think>` blocks at the start, trim, reject an empty reply. Still open: strip only one exact `<think>` block, or drop even these rules. Spec S3.4 still describes more stripping (a recorded deviation). Was the BACKLOG item "Cleanup: is any left?".

**To do.**
- [ ] Owner decides which rules stay.

**Where in the code.**
- `src/cleanup.rs`

## Closed
