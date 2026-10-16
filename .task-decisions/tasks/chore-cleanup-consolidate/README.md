---
title: Cleanup B: consolidate duplicate tests and remove stale names, comments and leftovers
state: Done
ticket: —
branch: chore-cleanup-consolidate
mr: —
start: 2026-10-16
end: 2026-10-16
---

Second house-cleanup PR (plan: `chore-cleanup-test-bounds/D1`, audit `chore-cleanup-test-bounds/reports/2026-10-16-test-audit.md`, proposed PRs B and C). The common provider config cases now run from one table in `crates/relay/tests/config.rs`; Kimi and Kiro keep only their own validation; duplicate model-order and explicit-model tests are merged (1,141 → 1,130 tests, every removed case covered by the table). The root fake CLI binary is `pumice-root-test-cli`, ending Cargo's output-collision warning. Tests and comments that said "fallback" or "bare pumice" now describe the current single-provider, terminal-failure and standalone-relay behavior; the unused `windows-shims/codex.ps1` fixture and two unneeded `allow(dead_code)` are gone; tests that skip themselves say so in CI logs. Docs: the transcription README points to the CPU-variant and Windows Vulkan preview instructions, the two architecture research notes carry a dated supersession notice, and the README roadmap lists the modules that exist now (`pumice setup` paused). Archived adapters, security tests, removed-key rejection tests and history were kept (`chore-cleanup-test-bounds/D2`). Production behavior unchanged. Implemented by a Claude Opus 5.5 subagent; one Codex review round (approve); final review by the orchestrator.
