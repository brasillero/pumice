---
title: Detect installed providers at startup
state: Done
ticket: —
branch: s2.8-provider-detection
mr: PR #28
start: 2026-10-04
end: 2026-10-04
---

`ProbeSpec` + `discovery.rs`: bounded concurrent version probes (2 s) cached at startup, `agy` never spawned; `/v1/models` lists enabled∩found; a missing selected provider still fails at runtime and falls back.

Imported from the status tables in `HANDOFF.md`.
