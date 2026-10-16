---
title: P14.2: backend policy (`cpu|auto|gpu`) and reported compute
state: In progress
ticket: —
branch: p14.2-backend-policy
mr: —
start: 2026-10-16
end: —
---

Second GPU transcription PR (design: `p14.1-windows-vulkan/reports/2026-10-16-gpu-design.md` §6, §10 item 2; decisions `p14.1-windows-vulkan/D3`–`D5`; replaces the temporary switches of `p14.1-windows-vulkan#3`). Adds `backend: cpu|auto|gpu` to `transcribe.yml` and `--backend` to `file` and `serve`, one immutable `compute` snapshot reported in `file --json`, `serve` startup output, managed `ready` and `/ready` (internal protocol 2) and `pumice serve`'s status, and the D4 failure rules. Implementation by a Claude Opus 5.5 subagent, review by Codex, final review by the orchestrator.
