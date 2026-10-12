# CPU-only standalone transcription experiment

This isolated crate pins `transcribe-cpp` 0.3.1 with only the `serde` feature
and decodes WAV input with `hound`. Its empty `[workspace]` and separate
`Cargo.lock` keep the root Pumice package and dependency graph unchanged.
The executable requires an external GGUF model and WAV input; no native
runtime bundles, Python, or CMake are needed by the executable itself.
Python and CMake are build/CI tools only.

The binary accepts `stt-spike MODEL.gguf AUDIO.wav`, reports text, detected
language, load/session/transcription timings, and exits nonzero on errors
or an empty transcript. Input must be mono 16 kHz signed 16-bit PCM.
`fixtures/sample-pt.wav` is the owner's authorized 14.19-second Portuguese
recording (454,124 bytes). Language is detected automatically, never forced.

## CI

`.github/workflows/stt-spike.yml` runs only on manual dispatch and pull
requests changing this spike or its workflow. It builds the four release
targets, checks final executable dependencies, verifies the tiny model's
SHA256 even on a cache hit, and asserts non-empty text and language `pt`.
Each smoke run starts in a fresh empty directory. Job summaries report
size and timings; artifacts contain only the executable and expire in
three days. The model is cached separately by its SHA256.

The x86-64 builds explicitly require SSE4.2, AVX, AVX2, F16C, FMA and BMI2;
this experiment does not decide Pumice's production CPU baseline.
ARM64 uses CPU defaults with native tuning and system BLAS discovery disabled.
No GPU, shared libraries, dynamic backend modules, or OpenMP features are enabled.

Linux downloads cross-tools/musl-cross release `20261001` with the pinned
SHA256 in the workflow. `MUSL_TOOLCHAIN` must point at the extracted
`x86_64-unknown-linux-musl` directory. The linker wrapper replaces
`-lstdc++` with its static archive and rescans Rust's self-contained
libc/libunwind. Windows explicitly selects `+crt-static`; dumpbin rejects
CRT/ggml/transcribe DLLs and requires every import to match a system DLL
allowlist. macOS accepts only `/usr/lib/` and `/System/` dependencies.

## Local reproduction

Install the Rust musl target and provide the pinned musl C/C++ toolchain,
runner/build-host CMake, and the model verified by the smoke helper.
From the repository root:

```sh
export MUSL_TOOLCHAIN="/absolute/path/to/x86_64-unknown-linux-musl"
bash spikes/stt/ci/build-musl.sh
python3 spikes/stt/ci/verify-unix.py \
  spikes/stt/target/x86_64-unknown-linux-musl/release/stt-spike \
  x86_64-unknown-linux-musl
python3 spikes/stt/ci/smoke.py \
  spikes/stt/target/x86_64-unknown-linux-musl/release/stt-spike \
  /absolute/path/to/whisper-tiny-Q8_0.gguf spikes/stt/fixtures/sample-pt.wav
cargo test --locked
```

## Validation and remaining risks

Local Linux validation on 2026-10-12: release build passed with the CI
wrapper in 29.69 seconds. `file` reported `static-pie linked`; `ldd`
reported `statically linked`; readelf showed no INTERP or NEEDED entries.
The executable was 5,321,960 bytes. The tiny model's pinned SHA256 matched.
The empty-directory smoke run returned language `pt` and non-empty text;
model load took 0.063097 s and transcription took 0.298193 s (20 threads).
These are one-run measurements, not benchmark guarantees.
Root `cargo test --locked` passed: 569 tests across 25 suites, no failures.
Cargo metadata confirmed separate root and spike workspaces. YAML parsing
and trigger/matrix assertions passed; actionlint 1.7.12 also passed.
No GitHub workflow was dispatched.

- **Ubuntu:** CI download/build remains unverified. Toolchain musl 1.2.6
  headers are paired with Rust's self-contained libc (1.2.5 on the local
  toolchain); this sample does not establish exhaustive ABI compatibility.
  The runner must expose the requested AVX2 feature set.
- **Windows:** actual MSVC `/MT` propagation, final imports and execution
  remain unverified until CI. A legitimate new system DLL import will fail
  the conservative allowlist and require review. AVX2 feature availability
  is required.
- **macOS Intel:** compiler/SDK behavior, system-only linkage and execution
  remain unverified until CI; AVX2 feature availability is required.
- **macOS ARM64:** compiler/SDK behavior, CPU-only linkage and Portuguese
  language detection remain unverified until CI. Accelerate/libc++ may
  remain dynamic system dependencies, as permitted by the experiment.
