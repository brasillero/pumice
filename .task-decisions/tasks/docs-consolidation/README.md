---
title: Docs: consolidate product docs and research notes into maintained docs
state: Done
ticket: —
branch: docs-consolidation
mr: PR #131, PR #132
start: 2026-10-29
end: 2026-10-29
---

The product docs (`docs/product.pt-BR.md`, `docs/spec.md`) and the 17 research notes in `docs/research/` are gone (D1). What was still needed now lives in maintained docs: provider invocations, limits, effort and model rules, and the dated subscription-terms evidence in the new `docs/providers.md` (D2); the captured Handy contract and the `inspect` model in `docs/api.md`; the WSL notes and the maintainers' release procedure in `docs/install.md`; the CPU-variant evidence in `docs/transcription.md`. Spec-only details were dropped (D5). `AGENTS.md` carries the approved wording and the D4 fixes. PR #131 moves the content; PR #132, stacked on it, deletes the files. Historical task notes still cite the removed files; `.task-decisions/README.md` says to retrieve them from Git history.
