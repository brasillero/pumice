---
title: Configurable reasoning effort per provider
state: In progress
ticket: —
branch: p16-provider-effort
mr: —
start: 2026-10-20
end: —
---

Lets the owner set the reasoning effort of each AI provider in `relay.yml`, on one common scale (`low`, `medium`, `high`) mapped to each CLI's own setting, with a separate `thinking` switch for Claude (D2, D4, D5), and deletes the archived Kimi and Antigravity providers (D1, D6). Implemented and pushed on the branch, waiting for review: every provider entry writes `effort` and `claude` writes `thinking`, the plugins are deleted, AGENTS.md carries the authorized wording, and the mapping and its evidence are in `docs/research/P16-provider-effort.md`. Along the way the descriptors' built-in settings were dropped (`p17.1-explicit-config#1`). Models the CLIs are known to run without effort (Claude Code's `claude-haiku-4-5` and older, Kiro's `claude-haiku-4.5`) are refused at the `effort` key, and since Codex review round 2 Claude models are matched the way Claude Code canonicalizes their names and `thinking: false` is refused for models that keep thinking on; waiting for the next review round. Open: owner checks with real calls (#3), a thinking budget question (#4), the remaining Kimi mentions in AGENTS.md (#5), the live `relay.yml` before deploying (#6) and the shipped example's `haiku` with `thinking: false` (#7).
