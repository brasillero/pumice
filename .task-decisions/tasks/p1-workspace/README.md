---
title: P1: Cargo workspace with the relay as its own crate
state: In progress
ticket: —
branch: p1-workspace
mr: PR #93
start: 2026-10-12
end: —
---

Phase 1 of `arch-modules` (D11): the repository becomes a Cargo workspace; the relay moves to `crates/relay` (package `pumice-relay`, binary `pumice-relay`) and the root `pumice` binary runs it with unchanged behavior. Implemented by Kimi, reviewed by Codex (three requested changes applied), 572 tests passing. Waiting for CI and the owner's merge.
