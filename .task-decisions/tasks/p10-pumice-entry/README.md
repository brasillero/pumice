---
title: P10: `pumice` entry point and `relay.yml`
state: In progress
ticket: —
branch: p10-pumice-entry
mr: —
start: 2026-10-15
end: —
---

Step 5a of `p6-transcribe-http/reports/2026-10-15-implementation-order.md`: `pumice` dispatches to its modules (`pumice relay …` in-process, `pumice transcribe …` through the sibling `pumice-transcribe` executable) and the relay's configuration file becomes `relay.yml`, with no compatibility aliases (`arch-modules/D6`, `D7`, `D12`, `D13`). The live instance on server-1 keeps its installed binary until it is updated by hand. Implementation by Kimi k2.8-code has started.
