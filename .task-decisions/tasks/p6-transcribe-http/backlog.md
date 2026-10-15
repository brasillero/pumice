# Backlog — p6-transcribe-http

## Active

## Deferred

### #2 · Deferred · Test that serve() drains an active worker on shutdown
- type: Tech debt
- area: transcription
- origin: reports/2026-10-15-implementation-order.md
- ticket: —
- created: 2026-10-15
- reason: review round 3 follow-up; the drain was verified externally, the committed test covers only an idle shutdown

**Context.** `serve()` waits for the admission permit after axum stops, so a worker still running after a disconnect or deadline finishes before the process exits. Codex confirmed this with an external harness, but `serve()` does not expose its bound port or admission, so the in-repo test only shuts down an idle server.

**To do.**
- [ ] Let tests bind a given listener (or read the bound address) and hold a gated worker through shutdown.

### #3 · Deferred · Fewer copies of the uploaded audio
- type: Improvement
- area: transcription
- origin: reports/2026-10-15-implementation-order.md
- ticket: —
- created: 2026-10-15
- reason: review round 3 follow-up; 131 MiB peak for a 25 MB WAV is acceptable next to the ~1 GB model

**Context.** The body is buffered, then decoded into samples, resampled and normalized; each stage keeps its own buffer. With Turbo loaded the process peaked at about 1,034 MiB.

**To do.**
- [ ] Measure where the peak comes from and drop buffers as soon as the next stage has its data.

### #4 · Deferred · Accept WebM/Opus uploads
- type: Improvement
- area: transcription
- origin: conversation
- ticket: —
- created: 2026-10-15
- reason: approved by the owner (D7); scheduled after the CPU speed step

**Context.** Browsers' `MediaRecorder` produces WebM/Opus (or MP4/AAC), which the server rejects, so an app like T3 recording in the browser could not send its audio as is. The test page works around it by building a WAV itself. Symphonia 0.6.1 has no Opus decoder.

**To do.**
- [ ] Owner decides whether Opus is needed and how (a pure-Rust decoder, or with the Symphonia replacement).

## Closed

### #1 · Moved · Browser test page that talks to the transcription endpoint
- type: Task
- area: api
- origin: conversation
- ticket: —
- created: 2026-10-15
- closed: 2026-10-15
- outcome: → p6b-transcribe-test-page#1

**Context.** After the endpoint is done, the owner wants a simple HTML page that records from the microphone, sends the audio to `pumice-transcribe serve` and shows the result, to simulate an app like T3 integrating with Pumice. Codex writes it.

**To do.**
- [x] Codex writes the page once the endpoint is approved.
- [ ] Owner tries it by talking to it.
