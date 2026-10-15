---
title: P12: `pumice run` combined flow
state: In progress
ticket: —
branch: p12-pumice-run
mr: —
start: 2026-10-15
end: —
---

Step 6b of `p6-transcribe-http/reports/2026-10-15-implementation-order.md`, with the minimal part of step 5b it needs: `pumice run <AUDIO> --flow <ID> [--json]` transcribes a file with the sibling `pumice-transcribe`, applies the flow's formatter in-process and sends the result through the relay in-process, keeping the raw transcript and the final text separate. Flows are defined in a new `orchestrator.yml`. Design in `reports/2026-10-15-flow-design.md`, decisions D1–D8. Implementation by Kimi k3 has started.
