---
title: Small fixes: help after relay subcommands, test binaries in installs, a flaky test
state: Done
ticket: —
branch: chore-small-fixes
mr: PR #111
start: 2026-10-16
end: 2026-10-16
---

Fixed three small deferred items: `p10-pumice-entry#3` (the test helpers now need a `test-helpers` feature, enabled for tests by a dev-dependency on the package itself, so `cargo install --path .` installs only `pumice`), `p10-pumice-entry#4` (`--help`/`-h` after `relay serve`, `check-config` or `doctor` prints that command's help; option values spelled `--help` stay values) and `p13.2-managed-transcribe#1` ("Text file busy": tests copied executables in-process while other threads spawned; one shared helper now hard-links them into `CARGO_TARGET_TMPDIR` or copies through a `cp` child on Unix). Implemented by Claude Opus 5.5 subagents; two Codex review rounds (round 1: copy-fallback race and help detection eating option values; round 2: approve); final review by the orchestrator.
