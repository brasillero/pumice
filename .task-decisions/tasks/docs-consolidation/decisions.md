# Decisions — docs-consolidation

### D1 · Active · Product docs and research notes are deleted once their still-needed content has moved
- date: 2026-10-29
- decided by: brasillero
- origin: conversation
- area: docs
- revisit if: —

**Decision.** `docs/product.pt-BR.md`, `docs/spec.md` and all 17 research notes in `docs/research/` are deleted, after the knowledge still needed has moved into maintained documentation. Git history keeps the files.

**Why.** The spec predates the pass-through direction and the modular architecture, and owner decisions in `.task-decisions/` already override it; the research notes mix current facts with diaries, prototypes and superseded proposals. Keeping both beside the module guides leaves two sources for the same facts.

**Rejected.** Keeping the files as read-only history in the tree: Git history already keeps them, and live links into them keep stale text in front of readers.

**Consequences.** Historical task notes keep citing the removed files; they describe the repository at that time (#3).

### D2 · Active · Provider evidence gets one new doc and the release procedure goes into the install guide
- date: 2026-10-29
- decided by: brasillero
- origin: conversation
- area: docs
- revisit if: —

**Decision.** The only new document is `docs/providers.md`, for the three supported providers (Claude, Codex, Kiro): invocations, limits, effort and model rules, and the subscription-terms evidence. The release and npm maintainer procedure goes into `docs/install.md`, in a clearly marked maintainers section near the end. There is no `docs/distribution.md`.

**Why.** Owner request: one scoped new doc only.

**Rejected.** A separate `docs/distribution.md` for the release procedure.

### D3 · Active · AGENTS.md drops the product-doc and research-note rules in the approved wording
- date: 2026-10-29
- decided by: brasillero
- origin: conversation
- area: process
- revisit if: —

**Decision.** The owner authorized and approved these `AGENTS.md` edits: the architecture sentence points at the module guides and `arch-modules`; the original-spec paragraph becomes "Owner decisions recorded in `.task-decisions/` take precedence over descriptive documentation. Current behavior is documented in the relevant module guides."; the documents table drops the product-doc and research rows and gains a row for `docs/*.md` and crate READMEs; findings that reveal a documentation gap or conflict with an owner decision go to the task's `reports/` under **Questions for the owner**; the Language rule drops the Portuguese exception; provider limitations are recorded in `docs/providers.md` with the CLI version, evidence date and verification status; the last two stop-and-ask bullets refer to `.task-decisions/` only.

**Why.** The rules named files D1 deletes.

**Rejected.** —

### D4 · Active · AGENTS.md describes the formatter and orchestrator as implemented and follows the one-CLI install
- date: 2026-10-29
- decided by: brasillero
- origin: conversation
- area: process
- revisit if: —

**Decision.** `AGENTS.md` no longer calls the formatter and the orchestrator "planned" (both are implemented), and its "single executable per platform" framing is replaced with wording grounded in `arch-modules/D16`: Pumice is one CLI that may be installed as a folder, and the existing static linkage of `pumice` and `pumice-relay` stays as an implementation choice.

**Why.** Both points were stale; `arch-modules/D16` already superseded the single-file framing and left the `AGENTS.md` change to the owner.

**Rejected.** —

### D5 · Active · Spec-only details are not carried over
- date: 2026-10-29
- decided by: brasillero
- origin: conversation
- area: docs
- revisit if: —

**Decision.** Details that live only in the spec are dropped, not moved: the 30 s response goal, the 10–20 dictation corpus, the Handy dictionary note and the meaning of the name.

**Why.** They describe no current behaviour or rule.

**Rejected.** Moving them into a maintained doc.
