---
title: P14.2: backend policy (`cpu|auto|gpu`) and reported compute
state: Done
ticket: —
branch: p14.2-backend-policy
mr: PR #116
start: 2026-10-16
end: 2026-10-16
---

Second GPU transcription PR (design: `p14.1-windows-vulkan/reports/2026-10-16-gpu-design.md` §6; decisions `p14.1-windows-vulkan/D3`–`D5`, narrowed by D1 here). `transcribe.yml` gets `backend: cpu|auto|gpu` (default `cpu`) and `file`/`serve`/`backends` get `--backend`; `auto` falls back to CPU with a named reason, `gpu` fails instead. One `compute` snapshot from the model's actual device is reported in `file --json`, the `serve` startup line, managed `ready`/`/ready` (protocol 2 on both sides) and `pumice serve`'s status. A GPU load failure fails startup; a native failure that may leave the engine unusable retires a GPU process, withdrawing admission before the permit is released, while content and input errors keep serving (D1). The P14.1 temporary switches are replaced by the model-free `backends` command (`p14.1-windows-vulkan#3`). Implemented by Claude Opus 5.5 subagents; two Codex review rounds (round 1: content errors retired a healthy GPU, admission gap; round 2: name every error kind, fixed by the orchestrator); final review by the orchestrator. The real-GPU check is the owner's (#1).
