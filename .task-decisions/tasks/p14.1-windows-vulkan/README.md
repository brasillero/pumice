---
title: P14.1: Windows Vulkan transcription bundle foundation
state: In progress
ticket: —
branch: p14.1-windows-vulkan
mr: —
start: 2026-10-16
end: —
---

First of the GPU transcription PRs (design: `reports/2026-10-16-gpu-design.md`; decisions D1–D7 cover the GPU series). Adds a `vulkan` build of `pumice-transcribe` for Windows x64 that ships the runtime CPU variants plus one loadable Vulkan module, built in CI with a pinned Vulkan SDK, with a static-CRT and DLL-dependency audit and no-device fallback tests. The result is a CI artifact for the owner's real-GPU check, not a public release. Implementation by a Claude Opus 5.5 subagent, review by Codex, final review by the orchestrator.
