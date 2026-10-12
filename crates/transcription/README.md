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
TRANSCRIBE_CMAKE_ARGS = { value = "-DGGML_NATIVE=OFF -DTRANSCRIBE_X86_CONSERVATIVE=ON -DTRANSCRIBE_USE_SYSTEM_BLAS=OFF", force = false }
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
