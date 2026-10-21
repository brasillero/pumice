---
title: Create initial settings automatically on first start
state: Done
ticket: —
branch: auto-initial-settings
mr: —
start: 2026-10-21
end: 2026-10-21
---

Implements the owner's choice to create the initial settings automatically on first start, following the setup question in p17.1-explicit-config/D1. A missing required per-user file triggers creation of all four module files and the sample formatter from embedded examples before validation. Existing files remain untouched and must be complete; explicit missing paths and read-only diagnostics retain their behavior. All 1,258 workspace tests, Clippy and packaging checks passed; see [the validation report](reports/2026-10-21-implementation-validation.md). Live installations are unchanged.
