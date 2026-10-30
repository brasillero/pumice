# Backlog — docs-consolidation

## Active

## Deferred

## Closed

### #1 · Done · Move the still-needed knowledge into maintained docs
- type: Task
- area: docs
- origin: D1
- ticket: —
- created: 2026-10-29
- closed: 2026-10-29
- outcome: `66984ae` (PR #131)

**Context.** Before deleting (D1), every current fact in the product docs and research notes must live in a maintained doc, keeping observation dates, CLI versions, primary-source links and what was verified or not, and dropping diaries, prototype steps and superseded proposals (D2, D5).

**To do.**
- [x] `docs/providers.md` from `S0.2-cli-matrix.md`, `S0.3-terms.md` (Anthropic and OpenAI only, labelled historical), `S2.14-kiro-plugin.md` and `P16-provider-effort.md`.
- [x] `docs/api.md`: the captured Handy contract (`S0.1-handy-request.md`) and the `inspect` model (`S1.2-inspect-model.md`).
- [x] `docs/install.md`: Handy on Windows with Pumice in WSL (`S0.5-wsl-localhost.md`), the Kiro limits, and the maintainers' release section (`S7.1-portable-releases.md`, `S7.4-npm-distribution.md`, `release-0.3.0#1`).
- [x] `docs/transcription.md`: the CPU-variant mechanism and measurements (`P7-cpu-variants.md`).
- [x] Repoint every live reference (code comments, tests, CI, README) and add an Unreleased CHANGELOG line.
- [ ] Review.

**Acceptance criteria.**
- No maintained file links into `docs/research/` or the product docs, and every relative link in maintained docs resolves.

### #2 · Done · Apply the approved AGENTS.md edits
- type: Alignment
- area: process
- origin: D3
- ticket: —
- created: 2026-10-29
- closed: 2026-10-29
- outcome: `eb0df9a` (PR #132)

**Context.** `AGENTS.md` names the product docs and research notes in its architecture sentence, documents table, findings rule, Language rule, Provider plugins and stop-and-ask list, still calls the formatter and orchestrator "planned", and keeps the single-executable framing that `arch-modules/D16` superseded (D3, D4).

**To do.**
- [x] Apply the D3 wording.
- [x] Apply the D4 fixes.

**Where in the code.**
- `AGENTS.md`

### #3 · Done · Delete the product docs and research notes
- type: Task
- area: docs
- origin: D1
- ticket: —
- created: 2026-10-29
- closed: 2026-10-29
- outcome: `eb0df9a` and `aaefa3e` (PR #132)

**Context.** Once #1 is merged, the two product docs and the 17 research notes go (D1). Historical task notes keep their references, so `.task-decisions/README.md` needs to say where to find the cited files and stop naming the spec as current.

**To do.**
- [x] Delete `docs/product.pt-BR.md`, `docs/spec.md` and `docs/research/*.md`.
- [x] Update `.task-decisions/README.md` and add the note on historical references.
- [x] Final reference scan: only historical task notes may still cite the removed files.
