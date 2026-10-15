# Backlog — p4b-transcribe-config

## Active

## Deferred

### #2 · Deferred · Release archive and Windows/macOS engine builds for `pumice-transcribe`
- type: Task
- area: distribution
- origin: #1
- ticket: —
- created: 2026-10-14
- reason: next PR, after PR #96 merges

**Context.** The rest of #1: `pumice-transcribe` has no release archive yet, and its engine is only built and smoke-tested on Linux in CI (`.github/workflows/transcription.yml`).

**To do.**
- [ ] Release archive for `pumice-transcribe`, including Symphonia's MPL-2.0 notice and source pointer.
- [ ] Windows engine builds with `-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded`; macOS engine builds.
- [ ] Installation guide section for the transcription archive.

## Closed

### #1 · Done · Config file, model registry, release archive and user docs
- type: Task
- area: transcription
- origin: p4-transcribe-cpu#1
- ticket: —
- created: 2026-10-12
- closed: 2026-10-14
- outcome: PR #96 (config, model list, user guide); the archive and platform builds continue in #2

**Context.** The plan's phase 4 also lists `transcribe.yml`, a local model registry, a CPU release archive and user documentation. This PR ships the CLI with flags only, a crate README and a Linux CI smoke test.

**To do.**
- [x] `transcribe.yml` and a local model registry.
- [ ] Release archive for `pumice-transcribe`, including Symphonia's MPL-2.0 notice and source pointer.
- [ ] Windows engine builds with `-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded`; macOS engine builds.
- [x] User documentation in `docs/`.
