---
title: Configurable reasoning effort per provider
state: In progress
ticket: —
branch: p16-provider-effort
mr: —
start: 2026-10-20
end: —
---

Lets the owner set the reasoning effort of each AI provider in `relay.yml`, on one common scale (`low`, `medium`, `high`) mapped to each CLI's own setting, with a separate `thinking` switch for Claude (D2, D4, D5), and deletes the archived Kimi and Antigravity providers (D1, D6). Implemented and pushed on the branch, waiting for review: every provider entry writes `effort` and `claude` writes `thinking`, the plugins are deleted, AGENTS.md carries the authorized wording, and the mapping and its evidence are in `docs/research/P16-provider-effort.md`. Along the way the descriptors' built-in settings were dropped (`p17.1-explicit-config#1`). Open: owner checks with real calls (#3), a thinking budget question (#4), the remaining Kimi mentions in AGENTS.md (#5) and the live `relay.yml` before deploying (#6).
