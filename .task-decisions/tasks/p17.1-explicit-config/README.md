---
title: P17.1: every module setting is written in YAML
state: In progress
ticket: —
branch: p17.1-explicit-config
mr: —
start: 2026-10-20
end: —
---

Implements `p17-serve-model-names/D3` for `relay.yml`, `transcribe.yml`, `formatter.yml` and `orchestrator.yml`, as the first pull request of P17 (`p17-serve-model-names/D10`). Every setting must be written; an omitted one is an error at the mapping it belongs in (line 1 for a top-level one), and `null` stays valid only where it means something. A missing module file is an error too, except for the commands that only report what exists (`pumice relay doctor`, `pumice-transcribe models catalog|download|verify` and `backends --backend`, `pumice format list` and `show builtin:clean`). The example files write every setting with the old default values; `serve.transcription_flows` and transcription `models:` stay until the later P17 pull requests. Implemented by a Claude Opus 5.5 subagent; waiting for review. The owner's live configs need migrating before this ships (#3).
