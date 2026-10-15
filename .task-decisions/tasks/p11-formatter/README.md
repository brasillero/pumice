---
title: P11: formatter module
state: Done
ticket: —
branch: p11-formatter
mr: PR #104
start: 2026-10-15
end: 2026-10-15
---

Step 6a of `p6-transcribe-http/reports/2026-10-15-implementation-order.md` is done: `crates/formatter` (library, `pumice-format`, and `pumice format …` in-process) inserts an explicitly selected formatter's rules as one `system` message into an OpenAI chat request (D2), re-emitting everything else byte for byte (D4), refusing on existing instructions unless `--on-conflict append`, and rejecting duplicate top-level keys (D5). `builtin:clean` is adapted from Handy (D1) and Handy's MIT notice ships with the code, `show` output, release archives and npm packages. Implemented by Kimi (k2.8-code, then k3 for the fix rounds); four Codex review rounds, final approve with no findings. No HTTP endpoint (D3).
