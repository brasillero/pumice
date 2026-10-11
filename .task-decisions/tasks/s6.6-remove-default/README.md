---
title: Remove the default key; the client's model selects the provider
state: Done
ticket: —
branch: s6.6-remove-default
mr: PR #55
start: 2026-10-07
end: 2026-10-07
---

Owner decision (2026-10-07): the client's `model` field picks the provider and a request without a model returns the original text. This deviates from spec S6.2 ("an empty value falls back to the default provider"); the owner chose "no model → original text" and the spec is not edited (see AGENTS.md). `default:` (like `default_provider:`/`fallback_order:`) is rejected as removed at its line:column; `RawReason::NoModel`; `/v1/models` follows list order; startup line reads `providers: …; on failure or no model: original text`.

Imported from the status tables in `HANDOFF.md`.
