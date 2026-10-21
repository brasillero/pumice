---
title: P17.3: serve model names, provider/preset combinations and relay-off execution
state: In progress
ticket: —
branch: p17.3-served-models
mr: —
start: 2026-10-21
end: —
---

Implements p17-serve-model-names/D1, D2 and D9 after the explicit-settings and model-folder prerequisites (PR #123 and PR #124, both merged). The server lists the selected plain model for raw transcription and configured provider/preset combinations when the relay is enabled. Disabling the relay builds no AI runtime and rejects AI dictations while keeping raw dictations available. Implementation and independent review are pending. Live migration follows this pull request.
