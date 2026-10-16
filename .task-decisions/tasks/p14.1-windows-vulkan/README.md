---
title: P14.1: Windows Vulkan transcription bundle foundation
state: Done
ticket: —
branch: p14.1-windows-vulkan
mr: —
start: 2026-10-16
end: 2026-10-16
---

First of the GPU transcription PRs (design: `reports/2026-10-16-gpu-design.md`; decisions D1–D6 and D8 cover the GPU series, D7 superseded). A `vulkan` Cargo feature builds `pumice-transcribe` for x86-64 Windows with the nine MSVC CPU variants plus one loadable `gpu/vulkan/ggml-vulkan.dll`, loaded after the chosen CPU module with the same CPU|VULKAN mask. A `bundle.json` manifest (sizes and SHA-256 of every staged file) is checked once at startup before any module registers; a damaged or unlisted module is an installation error, while a missing GPU module or a Vulkan driver without a device runs on CPU. The Windows CI job installs the pinned Vulkan SDK (D8), builds with the static CRT, audits every shipped DLL against an exact import allowlist (only the GPU module may import `vulkan-1.dll`), runs registration, corruption and tiny-model CPU smoke tests, and uploads the bundle (63 MB, 21.8 MB zipped) as a 7-day artifact for the owner's real-GPU check; nothing is released. Implemented by Claude Opus 5.5 subagents; two Codex review rounds (round 1: one major, one minor; round 2: approve with one script nit, fixed by the orchestrator). Open: DLL search-order hardening (#1, a release gate), a no-loader machine test (#2) and the temporary GPU switches (#3, P14.2).
