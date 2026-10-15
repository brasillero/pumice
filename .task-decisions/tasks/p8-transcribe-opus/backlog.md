# Backlog — p8-transcribe-opus

## Active

## Deferred

### #1 · Deferred · Opus on Windows engine builds
- type: Tech debt
- area: distribution
- origin: D3
- ticket: —
- created: 2026-10-15
- reason: Windows engine builds are not scheduled (`p6-transcribe-http/D2`)

**Context.** Bundled libopus selects the DLL C runtime on MSVC (`OPUS_STATIC_RUNTIME=OFF`), which `opusic-sys` does not let us override; a static-runtime Windows build needs a CMake toolchain setting or a patch, checked through PE imports in CI.

**To do.**
- [ ] Solve `OPUS_STATIC_RUNTIME` when Windows engine builds are taken up.

### #2 · Deferred · Let the test page send the browser's own recording
- type: Improvement
- area: api
- origin: conversation
- ticket: —
- created: 2026-10-15
- reason: follows this task; small change to `tools/transcribe-test/`

**Context.** The test page builds a WAV in the browser because Opus used to be rejected. With Opus supported, it can send MediaRecorder's WebM/Opus as is, exactly like an app such as T3 would.

**To do.**
- [ ] Add a "send native recording (WebM/Opus)" option to `tools/transcribe-test/index.html`.

## Closed
