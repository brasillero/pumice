---
title: Modular architecture: relay, transcription, formatter, orchestrator
state: Done
ticket: —
branch: arch-modules
mr: PR #92
start: 2026-10-12
end: 2026-11-07
---

The architecture analysis and the first modular implementation are complete. PR #92 merged the architecture documentation on 2026-10-14 as `61047e2`; the workspace, relay, transcription, formatter and orchestrator subsequently shipped through their own tasks. Formatter choices are recorded in `p11-formatter`, combined execution and limits in `p13.1-flow-seam`, `p13.4-dictations` and `p15-serve-transcriptions`. D14 remains Provisional: modules communicate over loopback, and remote modules require a separate network and authentication policy when taken up. The owner requested closing the stale task state on 2026-11-07; all recorded decisions retain their existing states.
