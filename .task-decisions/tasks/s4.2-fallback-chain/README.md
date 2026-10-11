---
title: Try the next provider before falling back to raw text
state: Done
ticket: —
branch: s4.2-fallback-chain
mr: PR #15
start: 2026-10-04
end: 2026-10-04
---

`pipeline.rs` walks `fallback_order` within the total budget; skips duplicates/disabled/unbuilt; `FormatOutcome.attempts`; raw fallback carries the last failure.

Imported from the status tables in `HANDOFF.md`.
