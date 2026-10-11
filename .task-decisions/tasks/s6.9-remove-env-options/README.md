---
title: Remove provider env and options from the config
state: Done
ticket: —
branch: s6.9-remove-env-options
mr: PR #72
start: 2026-10-08
end: 2026-10-08
---

Owner decision (2026-10-08): each CLI's own config is inherited; `env:`/`options:` fail as removed; descriptor `allowed_env`/`validate_options` and `ProviderSettings.env/options` gone; Claude keeps only its thinking-off variable; generic's descriptor refuses to build (its config is redesigned when it returns).

Imported from the status tables in `HANDOFF.md`.
