---
title: P14.1b: restrict the Windows DLL search order of the transcription bundle
state: Done
ticket: —
branch: p14.1b-dll-search-order
mr: PR #117
start: 2026-10-16
end: 2026-10-16
---

Fixed `p14.1-windows-vulkan#1`. On Windows, `pumice-transcribe.exe` delay-loads `transcribe.dll` (MSVC cpu-variants builds) so no engine DLL is resolved before `main`; `main` first calls `SetDefaultDllDirectories(APPLICATION_DIR | SYSTEM32)`, which governs every later lookup (ggml's CPU and Vulkan modules, their dependencies, `vulkan-1.dll`); after the `bundle.json` check, `transcribe.dll` is preloaded by full path with `LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | SYSTEM32`, and a failure is a typed installation error. A Windows CI gate plants decoy DLLs separately in the current directory and on PATH (complete bundle, missing core listed and unlisted, a late-loaded module importing a unique absent dependency); with the protection no decoy loads, and negative controls showed loads from both places with the protection off (run 38945964968) and with only `SetDefaultDllDirectories` off (run 38948335671). No system file is touched; the no-loader machine and real-GPU checks stay with the owner (`p14.1-windows-vulkan#2`, `p14.2-backend-policy#1`, checklist in `docs/transcription.md`). Also fixed a managed-child race where `stopping` could be skipped (`request_stop` now announces before cancelling). Implemented by Claude Opus 5.5 subagents; three Codex review rounds (round 1: System32 mutation, combined decoy placements; round 2: no case for the process-wide default alone, checklist decoy in the app directory, the race; round 3: approve); final review by the orchestrator.
