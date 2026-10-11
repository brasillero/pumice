---
title: Restore npm scope after correcting account username
state: Done
ticket: —
branch: s7.4-restore-npm-scope
mr: PR #41
start: 2026-10-05
end: 2026-10-05
---

Fresh login confirms `brasillero`; the original `@brasillero` scope is restored and the front package is also scoped (`@brasillero/pumice`, command stays `pumice`) because npm rejects the unscoped name `pumice` as too similar to the existing package `juice`; the owner may pick a final public name later. All five packages are published on npmjs at 0.1.0 as of 2026-10-05; merge completes the story.

Imported from the status tables in `HANDOFF.md`.
