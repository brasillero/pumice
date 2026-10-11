# Decisions — s3.9-cleanup-reply-only

### D1 · Active · Cleanup reads only the reply
- date: 2026-10-08
- decided by: brasillero
- origin: s3.6-minimal-cleanup/D1
- area: pipeline
- revisit if: —

**Decision.** Cleanup never consults the dictation, and an opening `<think>` tag that never closes is kept as text instead of failing.

**Why.** Imported from `HANDOFF.md`.

**Rejected.** —
