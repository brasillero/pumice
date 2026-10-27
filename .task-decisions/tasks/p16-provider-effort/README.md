---
title: Configurable reasoning effort per provider
state: Done
ticket: —
branch: p16-provider-effort
mr: PR #128
start: 2026-10-20
end: 2026-10-27
---

Each provider entry in `relay.yml` now sets its reasoning effort on one common scale (`low`, `medium`, `high`), mapped to each CLI's own setting, and `claude` sets a separate `thinking` switch (D2, D4, D5); the Kimi and Antigravity providers are deleted (D1, D6). Startup refuses, at the key's file and line, effort on models the CLIs run without it and `thinking: false` on models whose CLI keeps thinking on, matching Claude model names the way Claude Code 2.1.310 canonicalizes them; aliases, the remapped legacy Opus ids and unknown models are accepted. The mapping, the CLIs' caps and the evidence are in `docs/research/P16-provider-effort.md`. Merged in PR #128 and deployed on server-1 with today's behaviour (`effort: low` everywhere, `thinking: false` on `claude`, `kimi` removed). Deferred for the owner: live checks with real calls (#3), a thinking budget (#4), the remaining Kimi mentions in AGENTS.md (#5) and the shipped example's aliases with `thinking: false` (#7).
