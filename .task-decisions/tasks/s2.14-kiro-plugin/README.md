---
title: Kiro plugin
state: Done
ticket: —
branch: s2.14-kiro-plugin
mr: PR #81
start: 2026-10-11
end: 2026-10-11
---

Owner decision (2026-10-10): rewritten from scratch, archived S2.9 adapter deleted unread. `kiro-cli chat --agent-engine v2 --no-interactive --output-format stream-json --trust-tools= --effort low`, message on stdin, client system prompt in a no-tools agent file `.kiro/agents/pumice.json` in the temp dir (approved exception to AGENTS.md rule 3); `file:` system prompts refused (`UnsupportedSystemPrompt`). 5 real calls; end to end works (4.7 s) but haiku-4.5 under Kiro's base instructions answered instead of formatting; the owner's later test with `glm-5` (2026-10-11) formatted correctly. Limitations in BACKLOG and docs/research/S2.14-kiro-plugin.md. Core by Claude, rest by Kimi, reviewed by Claude and Codex.

Imported from the status tables in `HANDOFF.md`.
