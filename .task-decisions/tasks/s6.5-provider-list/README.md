---
title: Explicit ordered provider list, no implicit defaults
state: Done
ticket: —
branch: s6.5-provider-list
mr: PR #53
start: 2026-10-07
end: 2026-10-07
---

Configuration format changed in 0.2: explicit ordered `providers:` list (`enabled` required, `model` required when enabled), optional `default:` key with a startup warning when unusable; `default_provider`/`fallback_order`/map-form `providers:` rejected as removed; no fallback — one provider per request, failure returns the original text; built-in default models and `disabled_by_default` removed (superseded by S6.6, #55: the `default:` key is itself removed).

Imported from the status tables in `HANDOFF.md`.
