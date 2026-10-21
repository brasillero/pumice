---
title: Generic model names for pumice serve, and a generic T3 voice demo
state: Done
ticket: —
branch: p17-serve-model-names
mr: —
start: 2026-10-20
end: 2026-10-21
---

Completed the generic T3 voice demo on its local-only branch and shipped real model names, explicit settings, models-folder discovery, provider/preset expansion and relay-off execution through PR #123, PR #124 and PR #125. Independent review and all implementation checks passed. The live settings and binaries are migrated, both model files are in the default models folder, and raw transcription, Codex formatting and the public relay passed verification. See [the deployment report](../p17-live-migration/reports/2026-10-21-live-migration.md). Clients use the new model ids; dictation flow ids remain unchanged.
