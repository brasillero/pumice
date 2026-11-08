---
title: P1: Cargo workspace with the relay as its own crate
state: Done
ticket: —
branch: p1-workspace
mr: PR #93
start: 2026-10-12
end: 2026-11-07
---

Phase 1 of `arch-modules` (D11) is complete: the repository is a Cargo workspace, the relay lives in `crates/relay` (package and binary `pumice-relay`), and the root `pumice` binary dispatches to it. Implemented by Kimi and reviewed by Codex, with three requested changes applied and 572 tests passing at implementation time. PR #93 merged on 2026-10-14 as `8c0562a`; the owner requested closing the stale task state on 2026-11-07.
