---
title: Let Codex inherit the user's own config and gateway
state: Done
ticket: —
branch: s5.1-codex-user-config
mr: PR #48
start: 2026-10-06
end: 2026-10-06
---

Owner-approved (2026-10-06), implementing the 2026-10-05 decision: `--ignore-user-config` is omitted, so the gateway in `~/.codex/config.toml` applies. Each `[mcp_servers]` name is disabled with `-c mcp_servers.<name>.enabled=false` (names only, never values); plugins, apps and `notify` are always off. The call fails closed to `--ignore-user-config` on an unreadable or odd config. Adds `toml` (parse only). Findings in `docs/research/S0.2-cli-matrix.md`.

Imported from the status tables in `HANDOFF.md`.
