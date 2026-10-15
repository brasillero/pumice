# Backlog — p8b-test-page-native

## Active

## Deferred

## Closed

### #1 · Done · Let the test page send the browser's own recording
- type: Improvement
- area: api
- origin: p8-transcribe-opus#2
- ticket: —
- created: 2026-10-15
- closed: 2026-10-15
- outcome: PR #101

**Context.** The test page builds a WAV in the browser because Opus used to be rejected. With Opus supported, it can send MediaRecorder's WebM/Opus as is, exactly like an app such as T3 would.

**To do.**
- [x] Add a "send native recording (WebM/Opus)" option to `tools/transcribe-test/index.html`.
