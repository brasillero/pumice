---
title: P14.1b: restrict the Windows DLL search order of the transcription bundle
state: In progress
ticket: —
branch: p14.1b-dll-search-order
mr: —
start: 2026-10-16
end: —
---

Fixes `p14.1-windows-vulkan#1`, the release gate before anyone but the owner runs the Windows transcription bundle. Windows must load the bundle's DLLs only from the bundle and System32, never from the current directory or PATH, including the DLLs the executable imports before `main` runs and the dependencies of modules loaded later. The owner chose npm packages carrying the bundle folder as the likely delivery (2026-10-16), which makes this gate the next step. Implementation by a Claude Opus 5.5 subagent, review by Codex, final review by the orchestrator.
