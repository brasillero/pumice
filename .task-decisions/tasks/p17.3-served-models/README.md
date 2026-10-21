---
title: P17.3: serve model names, provider/preset combinations and relay-off execution
state: Done
ticket: —
branch: p17.3-served-models
mr: PR #125
start: 2026-10-21
end: 2026-10-21
---

Implements p17-serve-model-names/D1, D2 and D9 after the explicit-settings and model-folder prerequisites (PR #123 and PR #124, both merged). The server lists the selected plain model for raw transcription and configured provider/preset combinations when the relay is enabled. Disabling the relay builds no AI runtime and rejects AI dictations while keeping raw dictations available. Independent review approved the implementation and all 24 checks passed; PR #125 merged as `57c4b1e`. See [the validation report](reports/2026-10-21-implementation-review.md). The subsequent [live migration](../p17-live-migration/reports/2026-10-21-live-migration.md) is complete and verified.
