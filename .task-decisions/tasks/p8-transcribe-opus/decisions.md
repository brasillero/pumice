# Decisions — p8-transcribe-opus

### D1 · Active · Opus is decoded with bundled static libopus
- date: 2026-10-15
- decided by: claude (orchestrator), brasillero (authorized)
- origin: reports/2026-10-15-opus-research.md
- area: transcription
- revisit if: a pure-Rust Opus decoder proves conformance, fuzzing and speed comparable to libopus

**Decision.** Opus packets are decoded by the reference libopus (BSD-3-Clause), built from source and linked statically through `opus` 0.4.0 / `opusic-sys` 0.7.5 (`bundled`).

**Why.** Mature reference decoder; about 40 ms for the 14 s sample against about 4.2 s for the best-tested pure-Rust decoder; adds about 290 KB; native code is allowed in transcription (`arch-modules/D2`).

**Rejected.** `opus-decoder` (100× slower, no fuzzing); the newest pure-Rust decoders (days old, unverified); `symphonia-adapter-libopus` (drops pre-skip and padding details).

### D2 · Active · WebM is read by webm-iterable behind strict validation, Ogg/Opus by Symphonia
- date: 2026-10-15
- decided by: claude (orchestrator), brasillero (authorized)
- origin: reports/2026-10-15-opus-research.md
- area: transcription
- revisit if: Symphonia is replaced, or its Matroska reader exposes DiscardPadding and handles live recordings

**Decision.** WebM/Opus goes through `webm-iterable` 0.7.1 after a bounded EBML pre-validation of the whole file; Ogg/Opus keeps Symphonia's Ogg reader with added strict page checks. Opus decoding sits behind Pumice-owned packet types, independent of Symphonia.

**Why.** Symphonia 0.6.1's Matroska reader drops DiscardPadding and fails on Chrome's live recordings; `webm-iterable` reads them but accepts truncated files on its own, so validation is ours.

**Rejected.** Symphonia's `mkv` feature; a hand-written WebM scanner (failed twice on live clusters).

### D3 · Active · Opus support is enabled together with the engine
- date: 2026-10-15
- decided by: claude (orchestrator), brasillero (authorized)
- origin: reports/2026-10-15-opus-research.md
- area: distribution
- revisit if: Windows or macOS engine builds are taken up

**Decision.** Opus decoding is a Cargo feature (`opus`) that the `engine` feature enables. Builds without it reject Opus files as unsupported.

**Why.** Engine builds run on Linux only today; bundled libopus on Windows MSVC needs extra static-runtime setup (`OPUS_STATIC_RUNTIME`), so the default Windows and macOS CI test builds should not compile it yet.

**Rejected.** Making libopus a dependency of every build.

### D4 · Active · MP4/AAC is outside this task
- date: 2026-10-15
- decided by: claude (orchestrator), brasillero (authorized)
- origin: reports/2026-10-15-opus-research.md
- area: transcription
- revisit if: a client that can only send MP4/AAC needs Pumice

**Decision.** This task adds only WebM/Opus and Ogg/Opus; MP4/AAC (older Safari recordings) is not included.

**Why.** `p6-transcribe-http/D7` approves Opus; Safari 18.4 and later can record WebM/Opus.

**Rejected.** Adding MP4/AAC now, which would double the demux and validation work.
