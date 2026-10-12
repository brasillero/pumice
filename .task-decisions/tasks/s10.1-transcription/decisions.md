# Decisions — s10.1-transcription

### D1 · Active · The transcription layer comes before the formatter
- date: 2026-10-12
- decided by: brasillero
- origin: conversation
- area: transcription
- revisit if: —

**Decision.** Pumice is split into independent layers: the relay (exists), transcription (audio to text) and a formatter (prompt presets). Work starts on transcription only; the formatter is set aside entirely.

**Why.** The relay already works with Handy; transcription is the next layer that unlocks something new, and the layers do not depend on each other.

**Rejected.** Building the formatter first: set aside by the owner.

### D2 · Active · Transcription follows the OpenAI-compatible audio API
- date: 2026-10-12
- decided by: brasillero
- origin: conversation
- area: api
- revisit if: —

**Decision.** The transcription layer is exposed as `POST /v1/audio/transcriptions`, with OpenAI's multipart request and response shape.

**Why.** Client apps must be able to talk to Pumice easily, without adapters.

**Rejected.** A Pumice-specific endpoint or format.

### D3 · Active · Transcription runs on the CPU
- date: 2026-10-12
- decided by: brasillero
- origin: conversation
- area: transcription
- revisit if: the CPU path works and GPU acceleration is taken up as an improvement

**Decision.** The first transcription build is CPU only; GPU backends (Vulkan, Metal, CUDA) are left out.

**Why.** A CPU build is simpler and keeps the single executable; GPU is an improvement once the CPU path works.

**Rejected.** Shipping GPU backends from the start: more build variants and runtime libraries.

### D4 · Active · OpenWhispr integration is skipped in favour of own transcription
- date: 2026-10-12
- decided by: brasillero
- origin: conversation
- area: process
- revisit if: the owner takes up `openwhispr-support` again

**Decision.** Work on OpenWhispr support is not pursued; effort goes to validating Pumice's own transcription instead.

**Why.** Handy already proves the relay; OpenWhispr would only be another client of the same layer.

**Rejected.** Debugging OpenWhispr's request format first.

**Consequences.** `openwhispr-support` stays Paused. `plugin-contract` still matters for a future formatter.

### D5 · Provisional · x86-64 releases target the AVX2 baseline
- date: 2026-10-12
- decided by: brasillero
- origin: #2
- area: distribution
- revisit if: users need CPUs without AVX2, or GPU support (#7) is in place and other CPU builds (#6) are taken up

**Decision.** x86-64 release builds of the transcription engine use the AVX2 instruction set (`GGML_NATIVE=OFF` with SSE4.2, AVX, AVX2, F16C, FMA and BMI2 turned on explicitly).

**Why.** On the owner's sample, AVX2 took 7.1 s against 20.5 s for plain x86-64, which is slower than real time; AVX2 covers nearly every x86-64 PC from about 2013 onward.

**Rejected.** Plain x86-64: about 3 times slower for every user. Native builds: cannot be distributed. Per-CPU runtime modules as Handy ships them: they break the single executable.

**Consequences.** CPUs without AVX2 cannot run the release build; a clear startup error is wanted instead of a crash.
