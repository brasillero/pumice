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
the [supported formats and limits](../../docs/transcription.md#supported-formats-and-limits).

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

`models download` uses [ureq](https://github.com/algesten/ureq) (MIT OR
Apache-2.0) as its blocking HTTP client, with TLS from
[rustls](https://github.com/rustls/rustls) (Apache-2.0 OR ISC OR MIT) whose
crypto provider is [ring](https://github.com/briansmith/ring)
(Apache-2.0 AND ISC). ring compiles some C and assembly through `cc`, so
building this crate needs a C toolchain on every platform; the stock GitHub
Actions runners (Windows, macOS, Linux) already provide one. WebPKI roots
ship via [webpki-roots](https://github.com/rustls/webpki-roots)
(CDLA-Permissive-2.0). SHA-256 comes from
[sha2](https://github.com/RustCrypto/hashes) (MIT OR Apache-2.0, pure Rust)
and the per-model download lock from
[fd-lock](https://github.com/yoshuawuyts/fd-lock) (MIT OR Apache-2.0; rustix
on Unix, windows-sys on Windows). These dependencies are only compiled into
`pumice-transcribe`, never into the `pumice` core.
