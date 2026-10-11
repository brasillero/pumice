---
title: Handy on Windows reaches Pumice in WSL
state: Done
ticket: —
branch: s0.5-wsl-localhost
mr: PR #5
start: 2026-10-04
end: 2026-10-04
---

Works in NAT mode with no changes. Use `127.0.0.1` in Handy: `localhost` first tries IPv6 `::1`, which is not forwarded (~2 s delay).

Imported from the status tables in `HANDOFF.md`.
