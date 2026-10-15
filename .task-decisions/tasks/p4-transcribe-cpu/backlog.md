# Backlog — p4-transcribe-cpu

## Active

## Deferred

## Closed

### #1 · Moved · Config file, model registry, release archive and user docs
- type: Task
- area: transcription
- origin: arch-modules#1
- ticket: —
- created: 2026-10-12
- closed: 2026-10-14
- outcome: → p4b-transcribe-config#1

**Context.** The plan's phase 4 also lists `transcribe.yml`, a local model registry, a CPU release archive and user documentation. This PR ships the CLI with flags only, a crate README and a Linux CI smoke test.

**To do.**
- [ ] `transcribe.yml` and a local model registry.
- [ ] Release archive for `pumice-transcribe`, including Symphonia's MPL-2.0 notice and source pointer.
- [ ] Windows engine builds with `-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded`; macOS engine builds.
- [ ] User documentation in `docs/`.
