---
title: Landing page at /
state: Done
ticket: —
branch: s4.8-landing-page
mr: PR #84
start: 2026-10-11
end: 2026-10-11
---

Owner decision (2026-10-11): a corporate web filter blocked the public address as uncategorized, so `GET /` now serves a one-paragraph static page (what Pumice is, link to the source) for filter reviewers. The page avoids words such as proxy, tunnel or VPN that text filters flag (a test enforces it). The README, install notes, example config, CLI help and npm/Cargo descriptions now describe Pumice as a tool rather than a proxy.

Imported from the status tables in `HANDOFF.md`.
