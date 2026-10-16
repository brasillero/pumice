---
title: P14.5: Linux Vulkan transcription bundle, checked on a real GPU
state: In progress
ticket: —
branch: p14.5-linux-vulkan
mr: —
start: 2026-10-16
end: —
---

Linux x86-64 (glibc) GPU transcription (design: `p14.1-windows-vulkan/reports/2026-10-16-gpu-design.md` §5, §7, §10 item 5; decisions `p14.1-windows-vulkan/D2`–`D5`, `p14.2-backend-policy/D1`, `arch-modules/D16`). The `vulkan` feature builds on Linux GNU too: the existing Linux CPU variants plus a loadable `gpu/vulkan/libggml-vulkan.so`, staged with a `bundle.json`, built and checked in CI (no device, software-only device, missing loader). Unlike Windows, this one gets a real-GPU check here: server-1 has an Intel UHD GPU with the Mesa Vulkan driver installed, so the orchestrator runs the CI bundle on it. Implementation by a Claude Opus 5.5 subagent, review by Codex, final review by the orchestrator.
