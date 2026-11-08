# Backlog — arch-modules

## Active

## Deferred

## Closed

### #1 · Done · Architecture analysis for the modules
- type: Task
- area: architecture
- origin: D1
- ticket: —
- created: 2026-10-12
- closed: 2026-11-07
- outcome: PR #92 merged the analysis; PR #104 and PR #121, plus p13.1-flow-seam and p13.4-dictations, implemented the first-iteration formatter, combined API and limits. D14 defines the loopback-only network scope; remote modules remain outside this completed iteration.

**Context.** Codex writes the analysis: modules and responsibilities, contracts and communication, repository and binary layout, distribution, what changes in today's Pumice and in `AGENTS.md`, and the order of work. The owner reviews it before any decision on the open choices.

**To do.**
- [x] Codex analysis written to `docs/research/architecture-modules.md`.
- [x] Owner review of the first four choices: D6–D10.
- [x] Revise the phased plan to D6–D10 (`docs/research/architecture-plan.md`) (bare `pumice` as orchestrator, `relay.yml`, CPU + runtime CPU + GPU, downloader).
- [x] Remaining owner questions from the analysis (formatter, combined API, limits, network), when those phases start.
