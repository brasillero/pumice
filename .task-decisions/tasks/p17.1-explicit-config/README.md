---
title: P17.1: every module setting is written in YAML
state: Done
ticket: —
branch: p17.1-explicit-config
mr: PR #123
start: 2026-10-20
end: 2026-10-20
---

Implements `p17-serve-model-names/D3` for `relay.yml`, `transcribe.yml`, `formatter.yml` and `orchestrator.yml`, as the first pull request of P17 (`p17-serve-model-names/D10`). Every setting must be written; an omitted one is an error at the mapping it belongs in (line 1 for a top-level one), and `null` stays valid only where it means something. A missing module file is an error too, except for the commands that only report what exists (`pumice relay doctor`, `pumice-transcribe models catalog|download|verify` and `backends --backend`, `pumice format list` and `show builtin:clean`). The original one-off `file --model-file` exception was rejected by the owner in D1 and removed in PR #124; every file transcription requires complete settings. The CI configs the workflows write are complete too. The example files write every setting with the old default values; the later P17 pull requests replaced `serve.transcription_flows` and transcription `models:`. Implemented by a Claude Opus 5.5 subagent, approved by Codex and the orchestrator. All four live settings files were completed and verified with the deployed binaries on 2026-10-21 (#3, done).
