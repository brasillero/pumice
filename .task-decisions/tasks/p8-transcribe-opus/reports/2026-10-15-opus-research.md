# Opus support research — 2026-10-15

**Recommendation:** use bundled static **libopus 1.6.1** through `opus 0.4.0` / pinned `opusic-sys 0.7.5`; use **webm-iterable 0.7.1** for a separate WebM/Opus path, with mandatory bounded EBML pre-validation. Keep Symphonia for existing formats and Ogg demuxing for now. Keep packet decoding/container timing behind Pumice-owned types so neither the Opus decoder nor WebM path depends on Symphonia's eventual replacement.

**Scope:** research only; repository stayed on `main`, with its pre-existing untracked `spikes/` untouched. Read AGENTS, task README, p6 decisions/backlog, arch D2/D13, audio.rs, transcription docs, and the earlier endpoint report from `origin/s10.1-transcription`. No repository edits, AI CLI calls, model inference, installation in `~/.cargo/bin`, or service changes. Scratch HTTP used only 127.0.0.1:17842; recorded PID 327546 exited under `timeout`. Protected ports were unused.

## 1. Decoder choices and evidence

Versions/licenses below are the latest stable releases returned by crates.io on the research date; downloaded manifests, READMEs, build scripts and registry JSON are retained in `sources/`. Conformance descriptions are **upstream claims**, not independent certification; the full RFC vector suites were **UNVERIFIED** here.

| Option | Current version / license | Maturity, conformance and suitability |
| --- | --- | --- |
| [opus-decoder](https://docs.rs/crate/opus-decoder/0.1.1) | 0.1.1; MIT OR Apache-2.0, plus upstream libopus BSD provenance | Released March 2026; `forbid(unsafe_code)`, CELT/SILK/Hybrid, mono/stereo/multistream. Claims all 12 RFC 8251 vectors pass. README admits no fuzz harness. Float API actually decodes through i16 and converts to f32. Good measured fidelity, much slower here; not preferred for exposed uploads yet. |
| [ruopus](https://docs.rs/ruopus/0.1.2/ruopus/) | 0.1.2; MIT | July 2026; encoder/decoder; claims 12 RFC 8251 vectors score 99.2–100%. Pure Rust with optional Python/analysis features, which are unnecessary. Implementation, speed and size **UNVERIFIED** here. |
| [veloce-opus](https://docs.rs/veloce-opus/0.1.7/veloce_opus/) | 0.1.7; MIT OR Apache-2.0 | Released October 11, only days old; clean-room RFC 6716/8251 decoder, no unsafe/C dependencies. Full-vector conformance claim not found in published crate; **UNVERIFIED** quality, portability and size. |
| [ps-rusty-opus](https://docs.rs/ps-rusty-opus/1.0.0/rusty_opus/) | 1.0.0; BSD-3-Clause | Released October 11; pure Rust encoder/decoder, SIMD, claims bit-exact results on all 12 RFC 8251 vectors and fuzz/Miri coverage. Very new despite 1.0; claims, speed and size **UNVERIFIED** independently. |
| [oxideav-opus](https://docs.rs/crate/oxideav-opus/0.0.14) | 0.0.14; MIT | September release explicitly calls itself a clean-room rebuild in progress. Detailed SILK/CELT/Hybrid differential claims, not an independently verified complete conformance result. Avoid as first production decoder; size **UNVERIFIED**. |
| [opus](https://docs.rs/opus/0.4.0/opus/) + [opusic-sys](https://docs.rs/crate/opusic-sys/0.7.5) | 0.4.0 MIT/Apache-2.0 + 0.7.5 BSD-3-Clause | Safe Rust API over mature reference C libopus, currently bundled **1.6.1**, not audiopus_sys. Bundled build explicitly emits `static=opus`. libopus's README supplies RFC 8251 vector instructions. Recommended maturity/performance tradeoff; native code is allowed by arch D2 and remains outside core per D13. |
| [audiopus_sys](https://docs.rs/crate/audiopus_sys/0.2.2) | 0.2.2; ISC; bundled libopus BSD-3-Clause | Last release 2021, bundles Opus 1.3. Static feature and pkg-config bypass needed to make bundling deterministic; glibc defaults dynamic, musl/Windows/macOS default static. Older CMake minimum can conflict with CMake 4; actual build **UNVERIFIED**. Prefer current opusic-sys. |
| [symphonia-adapter-libopus](https://docs.rs/crate/symphonia-adapter-libopus/0.3.0) | 0.3.0; MIT OR Apache-2.0 | Matches Symphonia 0.6; default `bundled` uses opusic-sys. Published source handles only 1–2 channels, silently defaults malformed pre-skip to zero, drops residual pre-skip after the first packet, and ignores output gain/channel mapping. It cannot recover WebM padding that the demuxer discarded. Do not use unchanged; incremental size **UNVERIFIED**. |

**Build/platform matrix:** all pure-Rust options need only Rust for their decoder, with no C compiler/CMake/FFI; glibc, musl, MSVC `+crt-static` and macOS should work in principle, but only opus-decoder/glibc was built here. Other target claims remain **UNVERIFIED**. Native options require a target C compiler plus CMake at build time; opusic-sys 0.7.5's bundled Opus requires CMake ≥3.16. Bindings are pregenerated; libclang is unnecessary unless binding regeneration is enabled. Bundled libopus is BSD, so ship its notice.

For recommended opusic-sys, Linux glibc and musl static-libopus linkage were **measured** below. Windows MSVC needs extra CRT integration: cc recognizes Rust `crt-static`, **but bundled Opus CMake defaults `OPUS_STATIC_RUNTIME=OFF` and explicitly selects the DLL runtime**. opusic-sys does not expose that define. Set Rust `-Ctarget-feature=+crt-static` and inject `OPUS_STATIC_RUNTIME=ON` through a CMake toolchain/cache setting or a small sys-build patch; inspect generated flags and PE imports in CI. RUSTFLAGS alone is insufficient evidence. Actual MSVC/static-CRT result **UNVERIFIED**, a packaging risk. macOS should compile its static archive with Apple clang/CMake and link only OS libraries; actual x86_64/arm64 result **UNVERIFIED**. End users need none of these build tools. Disable no safety defaults; leave DRED/OSCE off and retain runtime CPU detection.

**Symphonia:** released [0.6.1 manifest](https://docs.rs/crate/symphonia/0.6.1/source/Cargo.toml.orig) has **no Opus decoder/feature**. Its README still recommends an external adapter. `mkv` supplies container parsing, not Opus decoding; do not confuse development-branch plans with a release.

## 2. Containers and timing requirements

**WebM/Matroska in Symphonia 0.6.1:** source `symphonia-format-mkv/src/codecs.rs` recognizes `A_OPUS` and preserves CodecPrivate as codec `extra_data` (the full OpusHead). `segment.rs` parses CodecDelay, SeekPreRoll and DiscardPadding. However, `demuxer.rs:519–524` keeps only BlockGroup data/duration, **dropping DiscardPadding**; `next_packet` constructs packets without trim fields. CodecDelay is subtracted from timestamps in `lacing.rs`, in track time units, but is not exported as Track.delay. SeekPreRoll is parsed but not retained in public track state. These fields are therefore not all exposed to an adapter. Source URLs: [demuxer](https://docs.rs/crate/symphonia-format-mkv/0.6.1/source/src/demuxer.rs), [lacing](https://docs.rs/crate/symphonia-format-mkv/0.6.1/source/src/lacing.rs).

The initial Symphonia-MKV prototype decoded ordinary WebM after using a genuinely seekable `Cursor<Vec<u8>>`; the repo's `ReadOnlySource<Cursor<_>>` hides length/seeking. Live unknown-size files still ended in `UnexpectedEof`. After the same live EOF failure recurred, that approach was abandoned. A hand-written metadata scanner also failed twice on Chrome's repeated unknown-size clusters and was abandoned. No unconditional “UnexpectedEof means success” workaround is acceptable.

**Independent WebM route:** [webm-iterable 0.7.1](https://github.com/austinleroy/webm-iterable) and ebml-iterable 0.7.1 are MIT; expose typed CodecPrivate, CodecDelay, SeekPreRoll, signed DiscardPadding, SimpleBlock/BlockGroup and lacing. They decoded the live/browser fixtures below. Set `set_max_allowable_tag_size(Some(actual_file_len))` (default is **4 GB**), keep error recovery/relaxation off, and validate element extents first. **Critical measured caveat:** default EOF master closing accepted two truncated files; this parser alone does not satisfy Pumice's no-partial-results rule. [Iterator configuration docs](https://docs.rs/ebml-iterable/0.7.1/ebml_iterable/struct.TagIterator.html).

**Ogg in Symphonia 0.6.1:** `mappings/opus.rs` recognizes OpusHead, sets 48 kHz, preserves full header and exposes pre-skip as Track.delay. `logical.rs` calculates packet end trims from granules. With an external decoder plus pre-skip and packet.trim_end, the fixture length is exact. Its page/sequence/continuation and mapping code includes warning/recovery paths; an adapter must add strict Ogg validation rather than assume every packet loss becomes an error. [Opus mapping](https://docs.rs/crate/symphonia-format-ogg/0.6.1/source/src/mappings/opus.rs).

A correct adapter must follow [RFC 7845](https://www.rfc-editor.org/rfc/rfc7845.html) and [Matroska's Opus mapping](https://www.matroska.org/technical/codec_specs.html):

- Validate OpusHead magic/version/length, channels, mapping, gain, stream/coupled counts and mappings. The original-input sample-rate field is informational; decode **48 kHz**, then existing stereo-average/downmix and rubato sinc resampling to **16 kHz**. Apply mandatory signed Q8 output gain; do not add optional loudness normalization.
- Consume pre-skip across as many packets as necessary, exactly once. Ogg granule accounting includes pre-skip; subtract it consistently and trim the final page to its end granule. Validate initial granule offsets/cropped streams and reject impossible backwards granules.
- For WebM, honor CodecDelay/OpusHead consistently without double skipping; validate disagreement. Positive DiscardPadding trims the end of its block, negative trims its beginning; distribute over laced frames, not just one guessed packet. Convert ns to samples with checked rational arithmetic. SeekPreRoll is decoder warm-up for seeking (normally 80 ms), not extra initial audio to discard; sequential decoding does not seek.
- Keep existing mono/stereo policy: family 0 supports 1–2 channels; family 1 requires valid canonical mono/stereo mapping or a multistream decoder (`opus::MSDecoder`). Reject >2 channels and unsupported families explicitly; never interpret multistream as an ordinary packet. Family 1/negative-padding/lacing correctness is **UNVERIFIED** by this prototype.
- Validate Ogg CRC, serial/sequence/continuations, complete packets, headers, EOS and end granules; reject chained/multiplexed streams. Validate WebM EBML IDs/sizes/hierarchy, known-size parent ends, complete block payloads, track consistency and timestamp continuity; accept unknown Segment/Cluster boundaries without duration/cues. Reject encryption/compression and unsupported track/codec changes. Never recover, skip bad packets, enable FEC or request PLC with empty input.
- Opus packets have **≤120 ms**, i.e. at most 5,760 frames/channel at 48 kHz; this is separate from Pumice's **120-second total** cap. Inspect packet duration with libopus before decoding and use a fixed 11,520-f32 stereo buffer. Matroska packet durations use track timebase, often milliseconds: `packet.block_dur().get()` is not directly a PCM frame count there.
- Fail before engine invocation on any detected corruption, missing data or cancellation. Unknown-duration WebM with no checksum/end marker cannot prove that whole final blocks were not lost at a clean boundary; even libopus cannot detect every valid-looking bit alteration. This inherent limit must not become a blanket EOF-success rule or a claim of complete integrity verification.

## 3. Scratch prototype and measurements

Reproduce with `timeout 1200 cargo build --release --bin webm --manifest-path ~/scratch/opus-research/Cargo.toml`; run `bin/combined <fixture>` or built `target/release/webm`. The prototype uses the repo library for existing audio paths/resampling, direct libopus packets, Symphonia Ogg, and independent WebM parsing. It is **not production-ready validation code**: family 0, nonnegative padding and unlaced WebM only; no strict envelope validator; Ogg helper tests zero gain only.

Source WAV: 227,040 mono 16 kHz samples, **14.190 s**, RMS **0.0094633070**. Fixtures were generated with downloaded ffmpeg **7.0.2-static**, used **only to generate fixtures**. No decoder runs ffmpeg. Commands/hashes/source provenance: `fixture-commands.json`, `fixture-provenance.json`; encoded originals and source are retained.

Generation: `ffmpeg -i source.wav -ac 1 -ar 48000 -c:a libopus -b:a 64k sample.opus` and the same command with `sample.webm`; live variant adds `-live 1 -cluster_time_limit 1000`. Stereo duplicate uses `-af 'pan=stereo|c0=c0|c1=c0' -ar 48000 -c:a libopus -b:a 96k -live 1 -cluster_time_limit 1000`. The earlier `-ac 2` fixture is also retained; ffmpeg's default upmix reduced each channel's level, so it is not the amplitude reference used below.

| Fixture / decoder | 48 kHz frames → 16 kHz samples | RMS at 16 kHz | Decode + resample |
| --- | --- | --- | --- |
| Ogg/Opus 64 kb/s, libopus | 681,120 → 227,040 | 0.0094542735 | 44.6 ms |
| WebM/Opus 64 kb/s, libopus | 681,120 → 227,040 | 0.0094542735 | 43.0 ms |
| Live WebM/Opus, libopus | 681,120 → 227,040 | 0.0094542735 | 43.3 ms |
| Live stereo duplicate 96 kb/s, libopus | 681,120 → 227,040 | 0.0094756185 | 47.2 ms |
| Actual Chrome MediaRecorder, libopus | 679,680 → 226,560 | 0.0094989115 | 40.1 ms |
| Same Ogg, opus-decoder 0.1.1 | 681,120 → 227,040 | 0.0094542654 | 4,203 ms |

Release/LTO/one-codegen-unit/stripped Rust 1.99, Linux x86_64; native medians of 5 processes, pure median of 3. Timers include demux/downmix/resampling, exclude file read and output write. CPU contention varied during research; these are local observations, not portable throughput guarantees. Mono ffmpeg Ogg/WebM outputs match each other sample-for-sample; against source, RMS difference **−0.0955%**, RMSE **0.0009609**, SNR **19.87 dB**, correlation **0.99484**, and **zero length error**. These measure lossy audio fidelity, not transcript accuracy. Pure-vs-libopus RMSE **4.58e-6**, SNR **66.29 dB**; full conformance remains **UNVERIFIED**.

Ignoring WebM's 3.5 ms DiscardPadding reproduced **681,288 frames / 227,096 samples**: 56 extra 16 kHz samples. Generated files declare pre-skip 312, CodecDelay 6.5 ms and SeekPreRoll 80 ms. Live variants have unknown Segment size, no Duration or Cues, and 15 clusters.

**Actual browser evidence:** T3's Chrome **154.0.0.0 Linux** ran WebAudio `decodeAudioData(source.wav) → MediaStreamAudioDestinationNode → new MediaRecorder(stream)`, `start(1000)`, stop on source end; no microphone permission needed. Result MIME **audio/webm;codecs=opus**, 211,862 bytes, 14 blobs, **14 unknown-size clusters**, 236 60-ms stereo packets, no Duration/Cues, OpusHead pre-skip **0**, no CodecDelay/SeekPreRoll/DiscardPadding. Chromium documents live recordings without seek metadata [here](https://chromium.googlesource.com/chromium/src/+/refs/heads/main/third_party/blink/renderer/modules/mediarecorder/README.md).

Chrome retained 14.160 s instead of source 14.190 s; best alignment was 104 samples (6.5 ms) at 16 kHz, aligned correlation **0.99546**, RMSE **0.0009049**. This capture/encoder boundary cannot be reconstructed from absent trim metadata; do not invent a decoder trimming heuristic. Browser setup and binary are retained in `browser-result.json` / `chrome-mediarecorder.webm`.

**Binary delta:** identical scratch audio-tool baseline (existing repo decoder/resampler), not the full server/engine binary:

| Linux target | Baseline bytes | Combined recommended prototype bytes | Delta |
| --- | --- | --- | --- |
| glibc | 1,094,344 | 1,386,504 | **+292,160 B** |
| musl | 1,211,520 | 1,498,272 | **+286,752 B** |

Both decoded Ogg/live WebM successfully. `readelf` proves no dynamic libopus in glibc (only libc/libm/libgcc_s), and **no NEEDED entries/interpreter** in musl. The alternative Opus+Symphonia-MKV glibc prototype was 1,534,592 B (+440,248 B); pure Rust with the same demux approach was 1,551,360 B (+457,016 B). Full product/engine and Windows/macOS size deltas, audiopus_sys/adapter/other-Rust-decoder sizes are **UNVERIFIED**.

**Negative evidence:** removing 17 bytes from ordinary and live WebM still returned success in webm-iterable; live output fell to 226,776 samples. Default auto-closing of masters can hide missing final data. The same silent-truncation failure was recorded twice and not retried. Removing the final Ogg page errored. See `negative-results.json`; strict pre-validation and no partial results remain implementation acceptance gates, not claims fulfilled by this scratch code.

## 4. Proposed integration and acceptance gates

Exact transcription-only dependency additions (keep current Symphonia features, **do not add mkv** for the recommended path):

```toml
opus = "=0.4.0"
opusic-sys = { version = "=0.7.5", default-features = false, features = ["bundled"] }
webm-iterable = { version = "=0.7.1", default-features = false }
# unchanged: symphonia = { version = "=0.6.1", default-features = false,
#   features = ["wav", "pcm", "mp3", "flac", "ogg", "vorbis"] }
```

Add EBML magic detection (`1A 45 DF A3`) in `AudioFormat::detect`; require WebM/Matroska doc type and exactly one supported audio track. Route this to a private `opus`/WebM module. In Ogg's existing branch, inspect actual codec parameters and route only Opus packets to that same decoder; retain Vorbis behavior. A Pumice-owned packet type carries bytes, skip and trim at 48 kHz; keep libopus calls independent of Symphonia's AudioDecoder trait. The rest of `DecodedAudio`, downmix, rubato and HTTP/fake-engine flow stays shared.

Before merge: preserve **25,000,000 bytes**, cancellation, typed errors, finite-value checks and the **120 s cap checked around every packet**, including actual output counts. Never reserve PCM from duration/granule/header claims. Parse EBML extents against the actual bounded slice with checked offsets/depth/count limits before any parser request can allocate from a claimed length; handle unknown masters by schema boundaries. Validate known parents against physical EOF and complete blocks before accepting default iterator EOF behavior. Reject lacing initially with a typed error, or implement and test all three lace modes with checked arithmetic; library coercion helpers contain unchecked slices and must not receive unvalidated hostile blocks.

Add strict Ogg page/envelope checks and header validation before demux; count actual packets/frames and compare trustworthy expected end information. Buffer the result until all checks finish, then call the engine once. Do not add Opus or native libraries to the core/relay manifests. CI must prove glibc/musl and MSVC `/MT` (including Opus's `OPUS_STATIC_RUNTIME=ON`)/macOS linking; no real AI/model work is needed.

Commit small generated fixtures with a provenance README, exact commands/tool versions, source SHA-256 and resulting SHA-256: short mono Ogg/Opus, ordinary WebM with nonzero end padding, live WebM without duration/cues, explicit duplicate stereo, actual Chrome multi-cluster recording, pre-skip spanning packets, family-1 mono/stereo (or explicit unsupported tests), short tails, corruption/truncation/page loss, oversized headers, >120 s compressed silence, channel/track changes and cancellation. Derive tiny malformed inputs in tests where clearer; keep browser fixture output with a note that MediaRecorder timing/bytes are not deterministic. No ffmpeg runtime/test-suite dependency.

**MP4/AAC: later, separate increment.** D7 specifically approves Ogg/WebM Opus; adding MP4 now enlarges the demux/validation matrix. Safari historically recorded MP4/AAC ([WebKit](https://webkit.org/blog/11353/mediarecorder-api/)), but Safari 18.4 added explicit WebM/Opus recording ([WebKit](https://webkit.org/blog/16574/webkit-features-in-safari-18-4/)); current default MIME was **UNVERIFIED** here. Clients should feature-detect/request WebM where available. If the owner requires older/default Safari capture, add `isomp4` + `aac` with fragmented-MP4 MediaRecorder fixtures and explicit AAC-LC limits; ordinary MP4 playback support alone does not prove live Safari recording support.
