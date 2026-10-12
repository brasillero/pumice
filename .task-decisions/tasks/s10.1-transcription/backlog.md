# Backlog — s10.1-transcription

## Active

### #1 · To do · Choose the transcription engine
- type: Task
- area: transcription
- origin: D1
- ticket: —
- created: 2026-10-12

**Context.** The engine must be built into the Rust binary and run on the CPU (D3). Leading candidate: the `transcribe-cpp` crate (0.3.1, MIT, from `handy-computer/transcribe.cpp` on ggml), which Handy itself uses for Whisper-family GGUF models, per Handy's `src-tauri/Cargo.toml`. On Windows x64 Handy ships extra DLLs (runtime CPU modules, Vulkan), so a single static executable is not proven.

**To do.**
- [x] Build a CPU-only `transcribe-cpp` spike on Linux and transcribe the sample (glibc; see `docs/research/S10.1-transcription.md`).
- [ ] Build it as a fully static `x86_64-unknown-linux-musl` executable, like the Linux release.
- [ ] Check that a CPU-only build links into one executable on Windows (MSVC, `+crt-static`) and macOS, through CI.
- [ ] Compare with alternatives only if it fails (for example `whisper-rs`).

**Acceptance criteria.**
- An engine choice with its evidence, recorded as a decision by the owner.

### #2 · To do · Measure speed and quality on the CPU
- type: Task
- area: transcription
- origin: D3
- ticket: —
- created: 2026-10-12

**Context.** The owner's 14 s sample (16 kHz mono WAV) is the first test input. server-1 has an i5-14500T (20 threads); the owner's Windows machine will differ. The model should be the one the owner already uses in Handy (#3).

**To do.**
- [x] Time a transcription of the sample per model/quantization tried (turbo Q8_0: 5.9 s native, 7.1 s AVX2, 20.5 s plain x86-64).
- [ ] Compare the text with what Handy produces for the same audio.
- [ ] Owner decides the x86-64 CPU baseline for releases (AVX2 or plain x86-64).

**Acceptance criteria.**
- Timings and transcripts recorded in a research note.

### #3 · To do · Decide where the model file comes from
- type: Task
- area: distribution
- origin: D1
- ticket: —
- created: 2026-10-12

**Context.** Models are 44 MB to 6 GB and cannot be bundled. `AGENTS.md` allows no outbound calls other than the AI CLIs, so a Pumice download would need an owner exception. Handy's catalog pins Hugging Face repos (`handy-computer/*-gguf`) with sha256 per file.
**To do.**
- [x] Confirm the owner's current Handy model: Whisper Large v3 Turbo Q8_0 (`whisper-large-v3-turbo-Q8_0.gguf`, 886 MB). For research it is in `~/pumice-models/` on server-1, downloaded at Handy's pinned revision `5eaf945c` with the sha256 checked, plus `whisper-tiny-Q8_0.gguf` for quick tests.
- [ ] Owner decides: the user supplies the file, or Pumice downloads it (exception to the network rule).

### #4 · To do · Minimal `/v1/audio/transcriptions` endpoint
- type: Task
- area: api
- origin: D2
- ticket: —
- created: 2026-10-12

**Context.** Accept OpenAI's multipart upload (`file`, `model`, optional `language`, `response_format`) and return `{"text": "..."}`. Explicitly enabled in the YAML, like providers.

**To do.**
- [ ] Check OpenAI's current request and response shape.
- [ ] Implement it behind the chosen engine (#1).

### #5 · To do · Test transcription in CI without a real model
- type: Task
- area: transcription
- origin: D2
- ticket: —
- created: 2026-10-12

**Context.** `AGENTS.md` keeps real CLI calls out of the test suite; the same reasoning applies to a 500 MB model. A fake engine, like the fake CLIs, can test the endpoint; a tiny model (Whisper tiny, about 44 MB) may serve an optional engine check.

**To do.**
- [ ] Define the engine interface so it can be faked.

## Deferred

## Closed
