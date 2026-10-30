---
title: Docs: consolidate product docs and research notes into maintained docs
state: In progress
ticket: —
branch: docs-consolidation
mr: —
start: 2026-10-29
end: —
---

The owner asked to remove the product docs (`docs/product.pt-BR.md`, `docs/spec.md`) and the 17 research notes in `docs/research/` after moving what is still needed into maintained docs (D1–D5). Two stacked PRs: `docs-consolidation` adds `docs/providers.md`, moves the remaining knowledge into `docs/api.md`, `docs/install.md` and `docs/transcription.md` and repoints every live reference, with the originals still present; `docs-removal`, on top of it, edits `AGENTS.md` and deletes the old files. Now: the migration is written, waiting for review.
