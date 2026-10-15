---
title: P4: standalone CPU transcription CLI
state: Done
ticket: —
branch: p4-transcribe-cpu
mr: PR #94
start: 2026-10-12
end: 2026-10-14
---

Phase 4 of `arch-modules` (D11): new crate `crates/transcription` with the `pumice-transcribe` program, which transcribes a local WAV file on the CPU with `transcribe-cpp` 0.3.1 (portable x86-64 baseline build), behind an off-by-default `engine` feature with a fake engine for tests. WAV input is validated, rebuilt canonically and decoded with Symphonia under 25 MB / 120 s limits; output is plain text or one JSON envelope. Implemented by Kimi; five Codex review rounds, final approve. Merged through PR #94 on 2026-10-14; the deferred config, archive, platform-build and docs item moved to `p4b-transcribe-config#1`.
