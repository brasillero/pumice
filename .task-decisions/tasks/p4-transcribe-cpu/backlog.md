# Backlog — p4-transcribe-cpu

## Active

## Deferred

### #1 · Deferred · Config file, model registry, release archive and user docs
- type: Task
- area: transcription
- origin: arch-modules#1
- ticket: —
- created: 2026-10-12
- reason: kept out to keep this PR small; taken up in the next transcription phases (endpoint, downloader, packaging)

**Context.** The plan's phase 4 also lists `transcribe.yml`, a local model registry, a CPU release archive and user documentation. This PR ships the CLI with flags only, a crate README and a Linux CI smoke test.

**To do.**
- [ ] `transcribe.yml` and a local model registry.
- [ ] Release archive for `pumice-transcribe`, including Symphonia's MPL-2.0 notice and source pointer.
- [ ] Windows engine builds with `-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded`; macOS engine builds.
- [ ] User documentation in `docs/`.

## Closed
