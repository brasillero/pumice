---
title: Let Claude use the user's own settings and gateway
state: Done
ticket: —
branch: s5.1-claude-user-settings
mr: PR #46
start: 2026-10-06
end: 2026-10-06
---

Owner-approved (2026-10-06): `--restricted` removed because it ignored `~/.claude/settings.json`, including the gateway `env` block, so Claude fell back to the subscription. `--safe-mode`, `--tools ""`, `--strict-mcp-config` and tool-activity rejection stay. The "weekly limit" message is classified as quota. Findings appended to `docs/research/S0.2-cli-matrix.md`.

Imported from the status tables in `HANDOFF.md`.
