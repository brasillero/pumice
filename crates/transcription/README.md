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

## Third-party licenses

This crate uses [Symphonia](https://github.com/pdelbarrio/symphonia) to decode RIFF/WAVE files. Symphonia is licensed under the MPL-2.0. Distributing binaries that include Symphonia requires shipping its notice and pointing to its exact source; this is enforced in the release-packaging phase.
