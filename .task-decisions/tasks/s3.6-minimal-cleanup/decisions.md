# Decisions — s3.6-minimal-cleanup

### D1 · Active · Cleanup only drops leading think blocks, trims and rejects empty replies
- date: 2026-10-07
- decided by: brasillero
- origin: conversation
- area: pipeline
- revisit if: —

**Decision.** Pumice keeps the model's reply as is, apart from removing closed `<think>`/`<thinking>` blocks at the start, trimming outer whitespace and rejecting an empty reply.

**Why.** Trust the model's reply; the prompt controls formatting. Imported from `HANDOFF.md`; deviates from spec S3.4.

**Rejected.** Preamble, quote and code-fence stripping (spec S3.4).
