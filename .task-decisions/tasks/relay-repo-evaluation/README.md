---
title: Repository evaluation and clean-up after relay-only
state: Done
ticket: —
branch: relay-repo-evaluation
mr: PR #141
start: 2026-11-11
end: 2026-11-11
---

Relay-only cleanup is implemented, including removal of the generic adapter, three native release targets, explicit architecture wording in AGENTS.md, and Windows resolution of Claude's native npm shim. Duplicate CI release builds are removed; release previews still check all shipped binaries and npm/npx installs. Local checks and all 14 checks on PR #141 at `81a6f5b` passed, including native builds and npm/npx installs on all three platforms. The owner approved merging; this final change finishes task notes before merge. Findings and remaining owner questions are recorded in `reports/2026-11-11-delivery-follow-up.md`. No publishing or live deployment was performed.
