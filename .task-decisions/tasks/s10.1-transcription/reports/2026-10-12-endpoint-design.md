# S10.1 transcription: design proposal for backlog #4 and #5

Research date: 2026-10-12. Repository inspected on `s10.1-transcription`, with a clean working tree. This is a proposal, not an owner decision or implementation. Repository paths below refer to `/home/brasillero/dev/personal/pumice`. Only this scratch document was written; no builds, model runs, AI CLI calls, service operations or connections to the production port were made.

## Recommendation

Implement a separate transcription service with one explicitly configured, CPU-only Whisper GGUF model; OpenAI multipart input; JSON and text output; WAV, MP3, FLAC and Ogg/Vorbis decoding; and one admitted transcription at a time, returning 429 while occupied. Keep the HTTP layer and fake-engine tests available without native code. Put `transcribe-cpp = "=0.3.1"` behind an **off-by-default Cargo feature**, explicitly enabled in every distributed build. Load the model once at startup only when YAML enables transcription. No downloader, formatting, translation, cleanup, automatic model aliases or relay fallback.

This follows D1–D3 and D5 ([decisions.md:10](.task-decisions/tasks/s10.1-transcription/decisions.md:10), [decisions.md:23](.task-decisions/tasks/s10.1-transcription/decisions.md:23), [decisions.md:36](.task-decisions/tasks/s10.1-transcription/decisions.md:36), [decisions.md:64](.task-decisions/tasks/s10.1-transcription/decisions.md:64)). Backlog #4 specifies multipart, optional language/response format, JSON output and explicit enablement; #5 requires an injectable engine ([backlog.md:59](.task-decisions/tasks/s10.1-transcription/backlog.md:59), [backlog.md:72](.task-decisions/tasks/s10.1-transcription/backlog.md:72)). The limits and codec subset proposed below need owner review for D2's intended client coverage.

## 1. OpenAI contract and the first supported subset

The live [transcription reference](https://developers.openai.com/api/reference/resources/audio/subresources/transcriptions/methods/create) and [generated official Python request schema](https://github.com/openai/openai-python/blob/main/src/openai/types/audio/transcription_create_params.py) were checked. Request: `POST /v1/audio/transcriptions`, `Content-Type: multipart/form-data; boundary=...`. `file` contains uploaded bytes, never a local path or URL; `model` is required. Arrays use repeated bracketed parts such as `timestamp_granularities[]=segment` and `include[]=logprobs` in official examples. Scalars are UTF-8 multipart text. Nested SDK form serialization can use bracketed keys; no nested object parser is needed when rejecting those options.

OpenAI's endpoint reference accepts **flac, mp3, mp4, mpeg, mpga, m4a, ogg, wav, webm**. The [speech-to-text guide](https://developers.openai.com/api/docs/guides/speech-to-text) says uploads are limited to **25 MB**, but lists fewer formats, omitting FLAC/Ogg. Prefer the endpoint-specific list; container support does not imply every codec inside that container. The docs do not specify the MB byte convention: Pumice should document **25,000,000 file bytes**, with a separate multipart overhead allowance. Real Handy/OpenWhispr *audio-upload* formats for this new endpoint are **UNVERIFIED**; the existing spike proves only a mono 16 kHz PCM WAV ([spikes/stt/src/main.rs:9](spikes/stt/src/main.rs:9)). D4 does not authorize restarting OpenWhispr integration.

| Multipart field | Current OpenAI meaning | Proposed first version |
| --- | --- | --- |
| `file` | Required audio upload | Require exactly one nonempty file; bounded decoding. Never open its filename. |
| `model` | Required model ID | Require exact configured `model_id`; no default or fallback. |
| `language` | Optional ISO-639-1 input language | Accept supported two-letter code; absent means detection. Reject unsupported codes; never translate. |
| `prompt` | Optional transcription context | Accept and forward unchanged to `RunOptions.prompt`, with a 16 KiB limit. Never add a Pumice prompt. |
| `response_format` | `json`, `text`, `srt`, `verbose_json`, `vtt`, `diarized_json`; support varies by model | Missing → `json`; support `json` and `text`; others → 400 naming supported formats. |
| `temperature` | Number in [0,1]; zero permits fallback | Accept finite [0,1], forward through Whisper run extension; absent → engine's zero/fallback defaults. |
| `timestamp_granularities[]` | `word` and/or `segment`, requiring `verbose_json` | Any supplied value → 400; timestamps deferred with verbose output. |
| `stream` | SSE where supported; ignored by OpenAI `whisper-1` | Accept absent/`false`; `true` → clear 400. Do not reuse chat's buffered SSE. |
| `chunking_strategy` | `auto` or `server_vad` configuration | Any supplied strategy or nested strategy part → 400; no VAD/normalization layer. |
| `include[]` | Currently `logprobs`, restricted to supported models/JSON | Any requested value → 400; do not manufacture confidence data. |
| `known_speaker_names[]`, `known_speaker_references[]` | Diarization reference pairs | Any supplied value → 400; no diarization. |
| `keywords[]`, `languages[]` | Newer `gpt-transcribe` vocabulary/language constraints | Any supplied value → 400; these are not equivalent to a single Whisper language hint. |

OpenAI currently lists `gpt-transcribe` in addition to Whisper/4o transcription models; Pumice must not advertise their capabilities merely because it shares the endpoint. “OpenAI-compatible” here means a documented supported subset, as model-specific subsets already exist upstream. Unlike OpenAI's `whisper-1`, rejecting `stream=true` is a deliberate local capability restriction; if the owner selects that ID and wants exact Whisper behavior, ignoring it is permissible **only as an explicit, documented compatibility choice**.

**What may be ignored:** omitted options; accepted no-op `stream=false`; an empty prompt (equivalent to none); bearer authorization headers on the existing unauthenticated loopback API. File name and MIME type are hints, not authority. Do not silently ignore a nonempty prompt, nonzero temperature or requested output feature. Unknown multipart fields → fixed 400, avoiding an unsupported option appearing to work. Reject duplicate scalar/file fields, malformed UTF-8/number/boolean values and unexpected file parts. Recognize bracketed roots when rejecting unsupported array/object options; cap fields at 32 and scalar fields at 16 KiB. These are proposed local limits.

**Success bodies:** all supported nonstreaming forms return 200. The [official response schema](https://developers.openai.com/api/reference/resources/audio/subresources/transcriptions/methods/create) and [verbose response type](https://github.com/openai/openai-python/blob/main/src/openai/types/audio/transcription_verbose.py) distinguish:

| Format | Body shape | First version |
| --- | --- | --- |
| `json` | JSON object with required `text`; optional model-specific `languages`, `logprobs`, `usage` | `application/json`, `{"text":"Hello."}`; no fabricated usage. |
| `text` | Raw transcript, without a JSON wrapper | `text/plain; charset=utf-8`; preserve engine text. |
| `srt` | Numbered cues, `HH:MM:SS,mmm --> HH:MM:SS,mmm`, text, blank separators | 400; requires verified segments. |
| `verbose_json` | Required `duration`, `language`, `text`; optional `segments`, `words`, `usage` | 400. |
| `vtt` | `WEBVTT` header and timed cues using decimal dots | 400; requires verified segments. |
| `diarized_json` | Duration, text, task and speaker-tagged timed segments, optional usage | 400. |

Verbose segments contain `id`, `seek`, `start`, `end`, `text`, `tokens`, `temperature`, `avg_logprob`, `compression_ratio`, `no_speech_prob`; words contain `word`, `start`, `end`, in seconds. The current verbose type does **not require `task`**; historical Whisper examples should not define the schema. SRT/VTT syntax above describes those established formats; exact OpenAI response MIME headers are **UNVERIFIED** and irrelevant until supported. Whisper in transcribe-cpp 0.3.1 exposes segment timestamps but explicitly lacks word-level DTW timing; do not promise all timestamp granularities merely because the API accepts them.

**Errors:** use JSON for every error, even when the requested response is text. The [official SDK ErrorObject](https://github.com/openai/openai-node/blob/master/src/resources/shared.ts) has `message`, `type`, nullable `param`, nullable `code`. Proposed example:

```json
{"error":{"message":"unsupported response_format; use json or text","type":"invalid_request_error","param":"response_format","code":"unsupported_parameter"}}
```

The [official error guide](https://developers.openai.com/api/docs/guides/error-codes) documents invalid-request errors, authentication/permission failures (401/403), rate/quota failures (429), processing errors (500), overload (503). [Official SDK status mapping](https://github.com/openai/openai-python/blob/main/src/openai/_client.py) also covers 400/404/409/422. The transcription reference does **not** publish a complete endpoint-specific status/error matrix: exact upstream mapping for oversized/corrupt uploads is **UNVERIFIED**, not an assertion that OpenAI always returns 413 or 415.

Pumice's proposed mapping: **400** invalid/missing/duplicate fields, unknown model, corrupt/empty audio, unsupported output options, excessive decoded duration; **415** unsupported request media type or recognized unsupported container/codec; **413** file/body byte cap; **408** body-read deadline; **429** occupied transcription slot (`rate_limit_error`, optionally `Retry-After: 1` as retry advice, not an ETA); **503** transcription disabled/unavailable; **504** total processing deadline; **500** engine failure, incomplete/empty output or worker panic. No credential/quota errors are invented. Messages must never contain uploaded text, filenames, paths or native error strings. Native failures may carry partial transcripts, so discard those on failure.

## 2. Audio decoding without end-user dependencies

Pin the versions chosen for implementation in Cargo.lock; versions below were verified from published crate manifests/docs on the research date. **Binary-size increments and four-target builds of the proposed decoder/resampler combination are UNVERIFIED**; no size estimates are presented as measurements.

| Component | Version / license | Build and format facts |
| --- | --- | --- |
| [Symphonia](https://docs.rs/symphonia/0.6.1/symphonia/) | 0.6.1 / MPL-2.0; Rust ≥1.85 | Pure Rust demux/decoders. Disable defaults; select codecs AND containers explicitly. No C compiler/library required by this stack. |
| [hound](https://docs.rs/crate/hound/3.5.1) | 3.5.1 / Apache-2.0 | Pure Rust WAV reader with no normal dependencies; smaller scope. Already used by the spike, but spike accepts only 16-bit mono 16 kHz. |
| [rubato](https://docs.rs/crate/rubato/5.0.1) | 5.0.1 / MIT OR Apache-2.0; Rust ≥1.87 | Pure Rust resampling. `default-features=false` removes default FFT dependencies; sinc resampling remains. |
| [opus-decoder](https://docs.rs/crate/opus-decoder/0.1.1) | 0.1.1 / MIT OR Apache-2.0, with libopus BSD provenance noted by upstream | Pure Rust packet decoder, no FFI; mono/stereo/multistream and float output. Upstream claims all 12 RFC 8251 vectors pass; independently UNVERIFIED. No container parser; upstream says no fuzz harness yet. |
| [opus](https://crates.io/crates/opus/0.4.0) / [opusic-sys](https://crates.io/crates/opusic-sys/0.7.3) | 0.4.0 / MIT OR Apache-2.0; sys 0.7.3 / BSD-3-Clause | Rust bindings to C libopus, **not pure Rust**. Current opus version uses opusic-sys, not audiopus_sys. Bundled sys builds a static archive using CMake; deployment can be one binary, but C build/CRT integration must be proved. |
| [symphonia-adapter-libopus](https://docs.rs/crate/symphonia-adapter-libopus/0.3.0) | 0.3.0 / MIT OR Apache-2.0; Rust ≥1.89 | Matches Symphonia 0.6; default `bundled` uses opusic-sys 0.7.3. Convenient mature C decoder path, outside the pure-Rust preference. Static musl/MSVC combination UNVERIFIED. |

The released [Symphonia 0.6.1 feature manifest](https://docs.rs/crate/symphonia/0.6.1/features) is authoritative: `wav` + `pcm`; `mp3`; `flac`; `ogg` + `vorbis`; `isomp4` + `aac`; `mkv` for WebM. `flac` includes native FLAC demuxing, so no imaginary `flac-format` feature. There is **no released Opus codec feature in 0.6.1**. The repository's main branch mentions an unfinished Opus codec; do not confuse that roadmap with the published crate.

Recommended first dependency:

```toml
symphonia = { version = "=0.6.1", default-features = false,
  features = ["wav", "pcm", "mp3", "flac", "ogg", "vorbis", "id3v2"] }
rubato = { version = "=5.0.1", default-features = false }
```

Accept PCM integer/float WAV, MP3, native FLAC and Ogg/Vorbis. Ogg/FLAC may be allowed once fixture-tested with these features. **Ogg is a container, not an Opus decoder.** Recognized Ogg/Opus must get 415: “Opus audio is not supported; upload WAV, MP3, FLAC or Ogg/Vorbis.” Do not ask users to rename a file. Unrecognized/corrupt content gets a safe 400; a recognized unsupported codec gets 415. MIME/extension hints must never override decoding evidence.

M4A/MP4 with AAC-LC is a straightforward next increment (`isomp4`, `aac`); Symphonia's published support is **AAC-LC**, not HE-AAC/HE-AACv2 or every Apple recording. First version: 415 with the supported list. Adding AAC-LC in the first PR is reasonable if an actual target client requires it; pure Rust adds no new runtime/build libraries, but adds fixtures and binary code. MPGA/MPEG-audio aliases should be accepted when bytes decode as the enabled MP3 codec, not treated as broad MPEG video support.

WebM/Opus requires `mkv` plus a decoder adapter; Ogg/Opus requires `ogg` plus that adapter. A pure Rust adapter around opus-decoder must map codec extradata/channel mapping, honor Ogg pre-skip/end trimming and WebM CodecDelay/DiscardPadding, handle lacing and correct output timing. Symphonia 0.6.1 demux sources recognize Opus, but complete adapter interoperability is **UNVERIFIED**. The C libopus adapter is an alternative if pure-Rust decoder validation is insufficient; statically linked C is compatible with the single-executable requirement, but is a separate build decision. Defer both Opus paths in this minimal task.

Decode incrementally from an in-memory seekable upload; choose exactly one audio track; reject unsupported channel layouts and midstream format changes. Initially support mono/stereo, 8–192 kHz; average stereo channels, then resample to 16 kHz f32 using band-limited sinc. Check finite values, normalize integer PCM correctly, and clamp finite out-of-range float PCM to [-1,1]. These transformations adapt audio to the engine, without semantic processing or loudness normalization. Bound allocations before trusting channel/sample-count headers; enforce decoded duration by actual frames, not metadata. Treat truncation/decoder errors as failure, rather than returning a transcript from partly decoded audio.

Use rubato 5's [Async sinc interface](https://docs.rs/rubato/latest/rubato/struct.Async.html), **not old `SincFixedIn` examples**. Process bounded blocks; flush the last partial block and account for filter delay so short clips neither lose endings nor gain padding. Its `process_all` helper handles delay for fully buffered inputs; incremental use needs equivalent bookkeeping and tests. Bypass resampling for 16 kHz input.

Static linkage for hound/Symphonia/rubato/opus-decoder is expected from their pure-Rust manifests; this is an inference, with the full Pumice musl/MSVC link checks still **UNVERIFIED**. No ffmpeg process, platform codec service, audio-device crate or end-user DLL is needed. Hound-only is the smallest implementation option, but WAV-only compatibility is narrower and switching frameworks later duplicates decoder work. Symphonia codec selection limits linked code; rubato without FFT avoids realfft/rustfft through that dependency. Exact savings depend on reachable code, LTO and target.

Measured engine-spike binaries are 5.3 MB musl, 3.7 MB Windows, 4.0 MB Intel macOS, 3.4 MB ARM macOS ([research:127](docs/research/S10.1-transcription.md:127)); these **are not incremental size costs for Pumice**. Compare release binaries with/without `stt` and codec feature sets using the same toolchain/profile; record bytes per target, dependency tree and linkage before merging. Existing size reporting is [ci.yml:45](.github/workflows/ci.yml:45). Preserve notices/license texts and source-availability information for MPL-covered dependencies in release/npm archives; [Mozilla's MPL FAQ](https://www.mozilla.org/en-US/MPL/2.0/FAQ/) explains file-level coverage and executable distribution. Pumice's own files can stay MIT; verify packaging obligations in the implementation review.

## 3. Fit with the existing Rust code

**Existing boundaries:** [src/api/mod.rs:31](src/api/mod.rs:31) holds Pipeline/log/debug/monitor state; its router is at [mod.rs:179](src/api/mod.rs:179). [handlers.rs:156](src/api/handlers.rs:156) limits **chat** to 10 MiB/5 s by manually reading Body; this is not a global audio limit. [types.rs:83](src/api/types.rs:83) currently has only error `message`/`type`; [handlers.rs:609](src/api/handlers.rs:609) builds fixed safe envelopes. `/v1/models` uses `Pipeline::model_ids()` ([handlers.rs:454](src/api/handlers.rs:454)).

[Config:26](src/config/mod.rs:26) defaults chat's total timeout to 30 s and parallelism to 4 (maximum 32). [Pipeline:284](src/pipeline.rs:284) queues provider runs FIFO within the request deadline; it does **not bound HTTP uploads or the number of waiters**. The startup phrase “up to 4 requests at once” ([main.rs:444](src/main.rs:444)) must become “up to 4 relay calls; 1 transcription” when enabled. Neither the Pipeline nor `Provider` abstraction should gain STT routing; it composes chat prompts and applies cleanup ([pipeline.rs:311](src/pipeline.rs:311)).

### YAML, identity and initialization

```yaml
transcription:
  enabled: true
  model_path: /path/to/whisper-large-v3-turbo-Q8_0.gguf
  model_id: whisper-large-v3-turbo
  threads: 20
  timeout_secs: 120
```

Section omitted → disabled. Section present → require explicit `enabled`; when true, require `model_path` and nonempty `model_id`. `threads` may default to `available_parallelism()` (fallback 1), with explicit positive override and an upper bound of 256; do not silently clamp an explicit setting. `timeout_secs` defaults to 120 and must be positive with checked deadline arithmetic. Defaults here are implementation proposals, not owner decisions. Model path resolves relative to YAML directory, never request filename; use a quoted Windows path where necessary and avoid implicit `~` expansion.

Add `RawTranscription` with `deny_unknown_fields`, a spanned enclosing section, and `Spanned` fields, following [raw.rs:29](src/config/raw.rs:29), [raw.rs:62](src/config/raw.rs:62). Reuse [config/mod.rs:125](src/config/mod.rs:125) and [mod.rs:157](src/config/mod.rs:157) for anchored paths, [ConfigError::at:28](src/config/error.rs:28) for `file:line:column`. Reject explicit null section/required values at their location; disabled settings still receive type/range validation but need no existing model file. Retain model-path source location through startup construction so corrupt/incompatible GGUF also reports the YAML line. `check-config` should validate/readability-check enabled model paths without allocating a full model.

Restrict the first engine adapter to Whisper-family GGUF; at load verify language detection/context prompt/cancellation capabilities as needed. A mismatched family is a config error, not a silent switch in semantics. Model files must be regular readable local files. Do not download or bundle them: #3 remains an owner question ([backlog.md:40](.task-decisions/tasks/s10.1-transcription/backlog.md:40)). No model lookup or load path is derived from the request's `model`.

`model_id` is a public exact-match name, independent of disk basename. Reject collisions with built-in/provider route names (case-insensitive against built-ins). The owner may explicitly choose `whisper-1` for a client with that name hardcoded, while documenting that the configured local model can be Turbo rather than OpenAI's Whisper V2. No implicit alias to every OpenAI ID. Missing/unknown request model → 400 with `param=model`. List exactly the enabled, successfully loaded model in `/v1/models` using existing `{id,object:"model",created:0,owned_by:"pumice"}`; no nonstandard capability fields are required. Chat should reject that ID; transcription should reject relay IDs.

**Prefer startup loading when enabled:** it costs ~0.35 s and ~1 GB for Turbo Q8_0 ([research:69](docs/research/S10.1-transcription.md:69)), avoids a cold first request and proves advertised readiness. Disabled transcription opens no model and uses no model RSS. Create one Model; create/drop a fresh Session per request so context cannot leak between dictations. Initialize outside Tokio's async workers, using `spawn_blocking` before serving. A bad enabled model fails startup as bad config; the implication for a relay-only user is intentional explicit enablement, and should be documented. Lazy loading would preserve low relay-only memory even with STT enabled, but adds single-flight load state/failure caching and cold deadlines; defer it unless the owner wants that behavior.

### HTTP resource policy and cancellation

Add Axum 0.8.9's `multipart` feature to [Cargo.toml:23](Cargo.toml:23); use the existing `Request` handler style to create a logging guard and acquire capacity **before** body extraction. Manually invoke `Multipart::from_request` and map both extractor and field errors to safe JSON. Axum's multipart default is 2 MiB (verified in its published `src/extract/multipart.rs`); apply route-local `DefaultBodyLimit::max(26_000_000)` rather than raising chat's limit. [DefaultBodyLimit docs](https://docs.rs/axum/0.8.9/axum/extract/struct.DefaultBodyLimit.html) distinguish extractor limits from raw Body reads.

Suggested fixed initial bounds: file ≤25,000,000 bytes; total multipart ≤26,000,000 bytes; scalar parts ≤16 KiB; ≤32 parts; actual decoded mono duration ≤120 s (1,920,000 f32 samples, 7.68 MB). Limit channels/sample rate as above and intermediate packet buffers too. The duration cap is an additional local restriction: compressed 25 MB inputs can represent hours. No upload audio files are written; read file chunks into one bounded buffer, release encoded bytes after normalization. Never allocate based only on Content-Length or audio header sample counts.

One owned semaphore permit covers **upload → decode → engine → release**, independent of chat's `max_parallel`. Immediate `try_acquire_owned()` failure → 429; **no queue initially**. This bounds memory/CPU and prevents many uploads spawning workers blocked on the model's internal mutex. A slow uploader occupies the one slot for at most a proposed 15 s body-read deadline; total transcription deadline starts at handler arrival and is `timeout_secs` (includes upload/decode/native compute). A bounded FIFO queue is a later alternative if clients cannot tolerate 429; it needs a separate admission cap before upload and queue deadlines, not merely `Semaphore::acquire()` on unbounded requests.

Run decoding/resampling and inference via `spawn_blocking` or one owned worker; never on async executor threads. Carry an app-owned cancellation flag plus deadline into decoding; map it to transcribe-cpp's `CancelToken` via `Session::set_cancel_token`. Request-drop guard cancels the flag; processing deadline → cancel and return 504. **Move capacity ownership into the worker** so HTTP timeout/disconnect cannot release the slot while native work still runs. Check cancellation before session/run and between decoder packets. Cooperative cancellation is not a hard kill: an in-process C++ call cannot safely be forcibly terminated, and abort latency is **UNVERIFIED**. Keep the slot occupied until return; drop late/partial output. On shutdown, stop admission, cancel active work, then drain the owned worker. A native segfault remains a process-level risk not solvable by this trait.

The published 0.3.1 sources were inspected locally (Cargo registry, not executed): `src/session.rs:35` has language/prompt, `src/family.rs:29` has `WhisperRunOptions.temperature`, `src/session.rs:138` installs cancellation, `src/session.rs:173` uses the per-model compute lock. `transcribe-cpp-sys/src/arch/whisper/capabilities.cpp:20` caps timestamps at segment; lines 28/32 enable cancellation/context prompt. Exact Pumice wiring and native cancellation behavior are **UNVERIFIED**. Versioned primary source packages: [transcribe-cpp 0.3.1](https://static.crates.io/crates/transcribe-cpp/transcribe-cpp-0.3.1.crate), [transcribe-cpp-sys 0.3.1](https://static.crates.io/crates/transcribe-cpp-sys/transcribe-cpp-sys-0.3.1.crate).

### Modules, fake seam and visibility

Suggested files:

- `src/transcription/mod.rs`: independent service, safe error enum, normalized options/result, capacity, deadlines, cancellation, injectable constructor.
- `src/transcription/engine.rs`: engine trait + production adapter under `#[cfg(feature="stt")]`; no native types escape the boundary.
- `src/transcription/audio.rs`: bounded demux/decode/downmix/resample; pure functions usable in model-free tests.
- `src/api/transcriptions.rs`: multipart validation, response serialization, error mapping and request lifecycle guard; router/state add `Option<Arc<TranscriptionService>>`.
- `tests/transcriptions.rs`, `tests/support/fake_engine.rs`, small generated/licensed audio fixtures: HTTP/service integration without model files.

A sufficient synchronous seam, called only on the worker:

```rust
pub trait Engine: Send + Sync {
    fn transcribe(
        &self,
        pcm_16khz_mono: &[f32],
        options: &TranscriptionOptions, // language, prompt, temperature
        control: &RunControl,          // app cancellation flag + deadline
    ) -> Result<Transcript, EngineError>;
}
```

`Transcript` initially holds only text and detected language; source duration is measured by the audio module. `EngineError` is a safe enum (`Cancelled`, `Failed`, `Incomplete`, etc.), with no user/native strings or partial transcript. The production adapter owns the already-loaded Model and thread count, sets `Backend::Cpu`, `Task::Transcribe`, no translation, and only client-supplied hints. Use default Whisper temperature fallback or its explicit extension; **never Debug-log RunOptions**, which derives Debug and contains prompt text. FakeEngine records PCM/options for assertions, returns arbitrary Unicode/whitespace text/errors, and can block on a test gate until cancellation. Injection must use the same validation/service path as production, not a magic `model=fake` public route or a packaged test engine.

At process startup, before Model::load, call `transcribe_cpp::disable_logging()` once. Its 0.3.1 `src/logging.rs:46` drops the native and ggml sink, including default stderr. Convert errors to typed safe metadata; native Display errors may contain prompt/partial dictation. Keep transcript/prompt-bearing app types without Debug, matching [api/types.rs:3](src/api/types.rs:3).

Plain-log example: `#42 transcription model=whisper-large-v3-turbo bytes=454000 audio_s=14.19 decode_ms=8 engine_ms=7080 status=200`. Log only validated configured ID, counts/timings/status and fixed failure codes. Do not log filename, raw multipart, prompt, audio metadata tags, arbitrary requested model, transcript, or native error detail. One terminal outcome per request, including rejection/drop, following [EntryGuard:38](src/api/handlers.rs:38).

The monitor is chat-shaped: [EventKind:78](src/monitor.rs:78) carries provider attempts/FormatOutcome; [DebugRecord:142](src/logging.rs:142) expects chat payload/raw_text. Add small transcription-specific parsed/started/finished event variants and a debug record rather than fabricating a chat attempt. Reuse shared request numbers, Arrived/Responded/Sent/drop concepts where their semantics fit; update TUI reducer exhaustiveness. With debug off, every text-bearing field stays absent. With debug on, allow capped prompt/transcript, masked headers and a file byte-count placeholder; **never dump lossy-UTF8 audio or base64 multipart**. Keep `/health` liveness lightweight and responsive while STT runs ([handlers.rs:471](src/api/handlers.rs:471)).

## 4. Cargo and build/workflow integration

```toml
[features]
default = []
stt = ["dep:transcribe-cpp"]

[dependencies]
transcribe-cpp = { version = "=0.3.1", optional = true,
  default-features = false }
```

The spike enables the crate's `serde` feature ([spikes/stt/Cargo.toml:10](spikes/stt/Cargo.toml:10)); main Pumice does not need it when mapping its own typed options. Never enable Metal, Vulkan, CUDA, shared libraries, dynamic-backends or OpenMP. Missing `stt` + `transcription.enabled=true` → located startup config error explaining the binary lacks STT; disabled mode still works. All releases/npm native packages must build with `--features stt`; default Cargo features affect compilation, **not YAML enablement**.

CPU/CMake requirements are proven in the spike ([research:52](docs/research/S10.1-transcription.md:52)): CMake ≥3.16, C/C++17, bundled native source, pregenerated bindings, no libclang or build-time model. Cold builds become longer; precise integrated build time is **UNVERIFIED**. Cache Cargo/native outputs keyed by target/toolchain/Cargo.lock/features/CMake flags; record cold/warm times and binary sizes. Do not assume Rust LTO makes the native archives smaller.

All x86-64 builds distributing STT must set `TRANSCRIBE_CMAKE_ARGS`:

```text
-DGGML_NATIVE=OFF -DTRANSCRIBE_USE_SYSTEM_BLAS=OFF
-DGGML_SSE42=ON -DGGML_AVX=ON -DGGML_AVX2=ON
-DGGML_F16C=ON -DGGML_FMA=ON -DGGML_BMI2=ON
```

For ARM macOS use `GGML_NATIVE=OFF`/BLAS OFF without x86 flags and without GPU features. For Windows append `-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded`; Rust's existing `+crt-static` ([.cargo/config.toml:1](.cargo/config.toml:1)) does not fix CMake's `/MD` objects by itself ([research:136](docs/research/S10.1-transcription.md:136)).

Before any native call on x86-64 STT builds, check SSE4.2, AVX, AVX2, F16C, FMA, BMI2 with Rust's runtime feature detection (AVX detection includes OS state support); give D5's clear startup error. Keep Rust/bootstrap code on its portable baseline; **do not set global `RUSTFLAGS=-C target-cpu=native` or `+avx2`**, which can defeat that check. Audit native static initialization for instructions before the guard; execution on an unsupported CPU is **UNVERIFIED** and needs an emulated/manual check or documented residual limitation.

**Concrete ci.yml changes** (current steps: [ci.yml:19](.github/workflows/ci.yml:19), [ci.yml:33](.github/workflows/ci.yml:33), [ci.yml:42](.github/workflows/ci.yml:42)):

1. Keep existing three-OS `cargo test --locked` and default clippy fast: no native feature/model. Add multipart/audio/FakeEngine cases to that suite.
2. Before feature builds, show/check CMake ≥3.16 and compiler availability; install a pinned user-local CMake only if absent/too old. On Windows prepare MSVC **before** the first native build (today dumpbin setup is after build).
3. Set explicit target CPU flags and Windows CMake CRT flag as above. Run `cargo clippy --locked --all-targets --features stt -- -D warnings` and `cargo test --locked --features stt`; these build native code but tests still use FakeEngine. Engine-specific tests can check a nonexistent/invalid tiny file and capability-independent wiring without loading a real model.
4. Change artifact release build to `cargo build --release --locked --features stt --bin pumice`. Keep size reporting, broaden Windows forbidden imports to ggml/transcribe/OpenMP/BLAS and verify system-only imports. Keep a default-feature build check if feature-enabled steps use different artifacts.
5. Distinguish core versus STT cache keys/profile outputs; add finite job timeout and compile-time/size summaries. A feature-enabled fake test proves the production feature compiles, not transcription quality.

**Concrete release.yml changes** (four targets: [release.yml:61](.github/workflows/release.yml:61); Linux tooling/build: [release.yml:85](.github/workflows/release.yml:85)):

1. Replace `musl-tools`-only installation with pinned, SHA-verified musl **C/C++** toolchain download/extraction from [stt-spike.yml:51](.github/workflows/stt-spike.yml:51). Proven archive: `cross-tools/musl-cross` release `20261001`, `x86_64-unknown-linux-musl.tar.xz`, SHA256 `faeaf3c7c377cb4351490988d9a1c8fed7d403a5b8066e5103af5d4dd96450fe`. No sudo/user runtime install is required.
2. Promote/generalize the helpers to release tooling. **Do not call `spikes/stt/ci/build-musl.sh` unchanged**: [line 3](spikes/stt/ci/build-musl.sh:3) changes into the spike and line 12 builds that crate. For main Pumice set target-specific `CC`, `CXX`, `AR`, PATH, Cargo linker, Rust `+crt-static`/`link-self-contained=yes`, then build root `--bin pumice --features stt`.
3. Use [musl-linker.py:9](spikes/stt/ci/musl-linker.py:9): resolve musl `libstdc++.a`, replace `-lstdc++`, append `-Wl,-Bstatic -lc -lunwind`. Preserve executable permission/Python shebang; release workflow already provisions Python 3.13.
4. Configure the same CMake CPU/BLAS/Windows CRT flags before every target build, check CMake/compilers, and add native-build cache/job timeout. macOS already has Apple C++ tools but must still prove CPU-only/system-library linkage.
5. Build **every** packaged executable with `--features stt`; retain --version/archive/npm install smoke checks. Extend Linux static `readelf` verification and Windows import checks, add macOS `otool -L` system-library verification from [spikes/stt/ci/verify-unix.py:1](spikes/stt/ci/verify-unix.py:1). No ffmpeg/libopus/model file ships.
6. Add promoted helper paths/audio-fixture paths/third-party-notice files to workflow path triggers; current filters only know old build helpers ([release.yml:13](.github/workflows/release.yml:13)). Update packaging to carry required license notices; keep the fake CLI/test engine out of archives. Add per-target size/time summaries.

**stt-spike.yml:** keep a manual optional real-engine smoke experiment, separate from the required #5 suite; consider removing its PR-triggered model download after integration. Its existing tiny-model cache/download ([stt-spike.yml:116](.github/workflows/stt-spike.yml:116)) must not be copied into regular CI. The four-target result is already recorded ([research:125](docs/research/S10.1-transcription.md:125)); older “not tried in CI” prose in research/backlog is stale relative to that section. This research did not rerun or independently inspect that CI run.

## 5. Focused PR/commit plan and model-free acceptance tests

Stay on this task branch and one focused S10.1 PR; do not mix provider refactors or formatter work. Suggested small Conventional Commit sequence (future implementation, no commits made here):

1. `feat: validate explicit transcription settings` — spanned YAML settings, anchored model path/ID/threads/deadline, defaults-off behavior, check-config/example YAML and exact-line tests; retain source location for load errors.
2. `feat: define transcription service and fake engine seam` — own safe types, admission, deadline/cancellation lifetime, worker ownership and FakeEngine tests; no public fake model route.
3. `feat: decode and normalize transcription uploads` — selected Symphonia/rubato features, deterministic fixtures, channel/duration/allocation bounds and resampling tests. Keep unsupported formats explicit.
4. `feat: expose OpenAI transcription multipart endpoint` — JSON/text, options validation, safe four-field error envelope, model discovery, logging/monitor/TUI events; HTTP tests with FakeEngine and all public failure paths.
5. `feat: integrate CPU transcribe-cpp backend` — optional pinned dependency, startup load, CPU guard, logging disabled, prompt/language/temperature mapping, native cancellation; feature-on compilation/no-model tests.
6. `ci: build static transcription releases on all targets` — generalize proven musl helpers, CMake flags/cache/link checks, native feature builds, notices packaging; no required model download.
7. `docs: document transcription compatibility and limits` — exact codec/field/response subset, local-model setup pending #3, capacity and timeout behavior, memory and AVX2 requirements. Separate task-notes commit **through the task-decisions skill only**; record only owner-approved decisions and do not mark the whole task Done while #1–#3 remain open.

Required tests should exercise public behavior, rather than mirror helper implementations:

- Multipart fields in arbitrary order (`file` before `model` too), boundaries split across HTTP chunks, no Content-Length, missing/duplicate fields, corrupt form, UTF-8 validation and wrong media type. Assert JSON error envelopes even for `response_format=text`; do not expose Axum rejection text.
- JSON default/text bodies, accented Portuguese/Unicode and significant whitespace returned unchanged. FakeEngine verifies language/prompt/temperature reach it without added instructions and `Task::Transcribe` mapping stays in the production adapter. No CLI is invoked.
- Small known audio fixtures: mono/stereo WAV at 16/44.1/48 kHz, integer normalization and float finite-value handling, MP3/FLAC/Vorbis with expected duration/sample tolerance; short/partial resampler tails and anti-alias behavior. Corrupt/truncated files and recognized AAC/Opus return correct safe errors; fixture codecs must actually match their extensions.
- Just-below/above file/body limits, too many/large parts, malicious audio metadata, actual decoded-duration overflow and unsupported channel/rate changes. Assert failure before engine invocation and bounded output allocation.
- Occupied capacity returns 429 without reading/allocating another audio body; timed-out/disconnected workers retain capacity until completion; cancellation does not return partial success. Use explicit fake gates/notifications, not sleeps; no reliance on Tokio's paused clock advancing inside a real blocking thread.
- Disabled/missing-feature mode, exact model-ID routing/collisions, `/v1/models` inclusion/exclusion; chat and health remain responsive during a blocked fake transcription. Monitor/debug/ordinary log capture proves prompt/transcript/native-like error text never appears with debug off and audio bytes are never dumped.
- YAML disabled/absent/enabled cases, nulls, invalid ranges/path/model ID with `file:line:column`; no model allocation when disabled. Feature-on no-model checks cover safe load-error mapping and build wiring.

Use ephemeral `127.0.0.1:0` listeners following [tests/monitor.rs:77](tests/monitor.rs:77), or in-process Router tests. All normal Linux/Windows/macOS tests need **no real model, internet, audio hardware or AI login**. Encoded fixtures are prepared once and committed with provenance, never generated via a runtime ffmpeg dependency in CI. Optional tiny-model smoke can prove actual inference later; it cannot replace fake tests of the failure/cancellation/logging contract. Correctness of the new real backend remains **UNVERIFIED** until implementation/manual or optional smoke verification.

## Owner questions before implementation choices become policy

1. **#3 model provenance:** user-supplied/local Handy model path, or a downloader with an explicit network-rule exception? Recommendation: local path first. Design must not settle the open download decision on the owner's behalf.
2. **#1 engine acceptance:** approve the proven 0.3.1 candidate as the engine choice; backlog still asks for a recorded owner decision. No alternative comparison is needed unless it fails.
3. **D2 client coverage:** approve JSON/text and WAV/MP3/FLAC/Ogg-Vorbis first, or require M4A/AAC-LC and/or Opus immediately? Actual intended client's upload/options must be captured before claiming adapter-free integration. No word timestamps/diarization/streaming promise.
4. **Capacity/limits:** is immediate 429 with one upload/run slot acceptable, and are 120 s audio/120 s total and 25 MB suitable for the first dictation use case? A queue/long recordings should be an explicit scope expansion.
5. **Memory/startup:** accept ~1 GB retained whenever transcription is enabled and startup failure for an invalid enabled model, or require lazy loading/independent relay availability on model-load failure?
6. **Public ID:** should the example expose `whisper-large-v3-turbo` or explicitly `whisper-1` for a particular client? If the latter, confirm `stream=true` compatibility behavior; do not silently claim a different model's identity/features.
7. **#2 quality/Windows:** owner's Handy comparison remains open; Windows accents need verification through UTF-8 JSON, as noted at [research:145](docs/research/S10.1-transcription.md:145). These are manual owner checks, not reasons to spend quota or install tools on Windows here. #6 extra CPU variants and #7 GPU stay deferred.
