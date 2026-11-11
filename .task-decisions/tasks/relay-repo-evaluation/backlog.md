# Backlog — relay-repo-evaluation

## Active

## Deferred

## Closed

### #1 · Done · Remove leftovers and stale material
- type: Tech debt
- area: architecture
- origin: reports/2026-11-11-repository-evaluation.md
- ticket: —
- created: 2026-11-11
- closed: 2026-11-11
- outcome: reports/2026-11-11-delivery-follow-up.md

**Context.** The owner asked on 2026-11-11 to "clean the house" after the relay-only change: remove stale and unused things and comments that only restate the code. The report lists what was found. The archived `generic` adapter stays until the owner answers, because an owner decision keeps it (`s2.13-archive-providers#1`).

**To do.**
- [x] Remove code kept only for the removed `pumice run` (`CleanupScope`, the caller deadline) with its tests.
- [x] Declare the provider modules where `cargo fmt` sees them, and format them.
- [x] Fix stale documentation and shipped texts (the `relay serve` command in `docs/install.md`, references to removed features).
- [x] Remove comments that only restate the code; keep the ones that record a reason, a limit or evidence.
- [x] Close the open notes of removed features.

**Acceptance criteria.**
- `cargo fmt --check`, clippy, `cargo test --locked` and the packaging tests pass.
- No behavior change in the relay.

**Where in the code.**
- `src/process/`, `src/pipeline.rs`, `src/providers/mod.rs`, `docs/`, `tests/`

### #2 · Done · Stop building and packaging the Intel Mac target
- type: Task
- area: distribution
- origin: D1
- ticket: —
- created: 2026-11-11
- closed: 2026-11-11
- outcome: reports/2026-11-11-delivery-follow-up.md

**Context.** D1 leaves three release targets. The Intel Mac target is still in the release workflow, the packaging scripts, the notices and the documentation.

**To do.**
- [x] Remove `x86_64-apple-darwin` / `darwin-x64` from the release workflow's build and npm smoke matrices.
- [x] Remove it from the packaging and notices scripts and their tests.
- [x] Update the documentation and the changelog's unreleased section.

**Acceptance criteria.**
- The release preview builds three archives and four npm packages (the launcher and three native ones).
- The launcher reports an Intel Mac as an unsupported platform.

**Where in the code.**
- `.github/workflows/release.yml`, `scripts/package_release.py`, `scripts/package_npm.py`, `scripts/third_party_notices.py`, `docs/install.md`, `scripts/npm/README.front.md`

