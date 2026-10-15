---
title: P13.3: `pumice serve` supervisor and status shell
state: In progress
ticket: —
branch: p13.3-serve-supervisor
mr: —
start: 2026-10-15
end: —
---

Third of four P13 PRs (design and decisions: `p13.1-flow-seam/reports/2026-10-15-p13-design.md`, `p13.1-flow-seam/D1`–`D9`). Adds `pumice serve` with the strict `serve:` config section, the loopback frontend listener (default 17570, 7567 refused), one owned `pumice-transcribe serve --managed` child with handshake, liveness probing, bounded restarts and teardown, and the read-only routes `/health`, `/ready`, `/pumice/v1/status` and `/pumice/v1/flows`. The dictation route comes in P13.4. Implementation by a Claude Opus 5.5 subagent, review by Codex, final review by the orchestrator.
