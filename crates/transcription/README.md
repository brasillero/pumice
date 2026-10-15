# pumice-transcription

Local speech-to-text transcription crate for Pumice.

## Building

By default the native engine is disabled so `cargo test` stays fast and offline:

```bash
cargo test -p pumice-transcription
```

To build with the real CPU transcription engine:

```bash
cargo build -p pumice-transcription --features engine
```

### Portable vs native CPU builds

The repository ships a portable x86-64 baseline build by default. This is configured in `.cargo/config.toml`:

```toml
[env]
TRANSCRIBE_CMAKE_ARGS = { value = "-DGGML_NATIVE=OFF -DTRANSCRIBE_X86_CONSERVATIVE=ON -DTRANSCRIBE_USE_SYSTEM_BLAS=OFF -DGGML_SSE42=OFF -DGGML_AVX=OFF -DGGML_AVX2=OFF -DGGML_AVX_VNNI=OFF -DGGML_AVX512=OFF -DGGML_AVX512_VBMI=OFF -DGGML_AVX512_VNNI=OFF -DGGML_AVX512_BF16=OFF -DGGML_FMA=OFF -DGGML_F16C=OFF -DGGML_BMI2=OFF -DGGML_AMX_TILE=OFF -DGGML_AMX_INT8=OFF -DGGML_AMX_BF16=OFF", force = false }
```

This disables host-specific CPU tuning (`-march=native`) and all x86 SIMD tiers so the binary runs on any x86-64 machine without SIGILL. Runtime CPU-variant selection is planned for a later phase.

To opt into a native build tuned to the host CPU, override the variable when invoking cargo. `force = false` in `.cargo/config.toml` means an environment variable takes precedence:

```bash
TRANSCRIBE_CMAKE_ARGS="-DGGML_NATIVE=ON" cargo build -p pumice-transcription --features engine
```

You can also selectively re-enable SIMD tiers, e.g. for the build host only:

```bash
TRANSCRIBE_CMAKE_ARGS="-DGGML_NATIVE=ON -DGGML_AVX2=ON -DGGML_F16C=ON" cargo build -p pumice-transcription --features engine
```

To avoid cached CMake flags from one build leaking into another, use a separate `CARGO_TARGET_DIR` for native builds:

```bash
CARGO_TARGET_DIR=target/native TRANSCRIBE_CMAKE_ARGS="-DGGML_NATIVE=ON" cargo build -p pumice-transcription --features engine
```

### Windows engine builds

Windows engine builds currently need to request a static C runtime from CMake so the resulting binary links consistently with the Rust `-C target-feature=+crt-static` setting:

```powershell
$env:TRANSCRIBE_CMAKE_ARGS="-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded -DGGML_NATIVE=OFF"
cargo build -p pumice-transcription --features engine
```

This is a documented limitation; Windows native builds come in a later phase.

## Usage

```bash
pumice-transcribe file audio.wav --model-file model.gguf --language pt --json
```

`--model-file` takes a direct GGUF path. You can also use `--model ID` to
select a model configured in `transcribe.yml`, or omit both flags to use the
configured `default_model`.

See [`docs/transcription.md`](../../docs/transcription.md) for the full guide:
configuration schema, model download recipes, JSON output, exit codes, and
supported formats.

## Supported formats

Formats are detected by content (magic bytes), not by file extension.

| Format | Container/codec | Limits |
| --- | --- | --- |
| WAV | RIFF/WAVE PCM / IEEE float / extensible | Max file 25 MB, max audio 120 s, sample rate 8–192 kHz, mono or stereo |
| MP3 | MPEG Audio Layer III | Same limits |
| FLAC | Free Lossless Audio Codec | Same limits |
| Ogg Vorbis | Ogg + Vorbis (one logical stream) | Same limits |
| Format | Container/codec | Limits |
| --- | --- | --- |
| WAV | RIFF/WAVE PCM / IEEE float / extensible | Max file 25 MB, max audio 120 s, sample rate 8–192 kHz, mono or stereo |
| MP3 | MPEG Audio Layer III | Same limits |
| FLAC | Free Lossless Audio Codec | Same limits |
| Ogg Vorbis | Ogg + Vorbis (one logical stream) | Same limits |
| Ogg Opus | Ogg + Opus | `engine` builds only; mono/stereo, one logical stream; same limits |
| WebM/Opus | WebM or Matroska + Opus | `engine` builds only; mono/stereo, one audio track, unlaced blocks; same limits |
| MP4/AAC | — | Not supported |

Opus decoding is only compiled into `engine` builds (the `opus` Cargo
feature, enabled by `engine`). Other builds reject Opus files with
`unsupported_format`. Opus always decodes at 48 kHz and is resampled to
16 kHz like any other input.

Known limit: a truncated file is rejected only when its container proves
completeness — a strict container check does this for WebM (every element
extent is validated against the actual file size) and for Ogg (per-page
checksums and the end-of-stream marker). A plain MP3 without a declared
frame count has no reliable end marker, so a cut-off file decodes as a
shorter recording instead of failing. As with any compressed format, a
WebM Opus stream recorded live (no duration, no cues) cannot prove that
whole final blocks were not lost at a clean boundary.

## Third-party licenses

This crate uses [Symphonia](https://github.com/pdeljanov/Symphonia) to decode
RIFF/WAVE, MP3, FLAC and Ogg Vorbis files. Symphonia is licensed under the
MPL-2.0. Distributing binaries that include Symphonia requires shipping its
notice and pointing to its exact source; this is enforced in the
release-packaging phase.

`engine` builds additionally decode Ogg/Opus and WebM/Opus with the
reference [libopus](https://opus-codec.org/) (BSD-3-Clause, statically linked
via `opus`/`opusic-sys`) and read WebM with
[webm-iterable](https://github.com/austinleroy/webm-iterable) (MIT, with
ebml-iterable, MIT). Binaries that include them must ship the libopus
BSD-3-Clause notice and the MIT licenses of webm-iterable/ebml-iterable.
