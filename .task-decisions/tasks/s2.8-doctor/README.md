---
title: Pumice doctor with an explicit login check
state: Done
ticket: —
branch: s2.8-doctor
mr: PR #29
start: 2026-10-04
end: 2026-10-04
---

`doctor` renders a fresh detection report (found/missing + install hints + warnings), exit 0/1/2; `--login-check --provider <id>` is the only quota-bearing path: one fixed call through the selected provider, no fallback, text-free result.

Imported from the status tables in `HANDOFF.md`.
