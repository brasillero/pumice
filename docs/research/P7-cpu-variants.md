# P7 — Runtime CPU variants for transcription

Status: implemented on `p7-transcribe-cpu-variants` (local only, not merged).
Target and authorization: `.task-decisions/tasks/p7-transcribe-cpu-variants/D1`
(warm Turbo Q8_0 on the 14.19 s sample under 14 s on server-1, same
transcript, portable baseline kept).

## What was built

- New opt-in Cargo feature `cpu-variants` on `pumice-transcription`
  (`= ["engine", "transcribe-cpp/dynamic-backends"]`). The default `engine`
  build stays a single static binary with the portable x86-64 baseline.
- With the feature, the engine links as shared libraries plus one ggml CPU
  module per x86 ISA tier (`GGML_CPU_ALL_VARIANTS`, 14 modules in a non-MSVC
  x86-64 build, from baseline `x64` up to `sapphirerapids`). At startup the
  binary loads the modules with `transcribe_cpp::init_backends_with(dir,
  BackendMask::CPU)` where `dir` is a private per-process directory inside
  the executable's own directory or its `lib/` subdirectory — never the
  current directory, PATH, a temp dir or ggml's default search paths.
  Registration is restricted to CPU backends, so no GPU driver can be pulled
  in.
- **The variant is chosen by pumice, not by ggml.** ggml's pinned scoring
  reads CPUID without checking OS-enabled state (its own FIXME in
  transcribe-cpp-sys-0.3.1 `ggml/src/ggml-cpu/arch/x86/cpu-feats.cpp:264`),
  so a CPU or guest advertising AVX/AVX-512 with the XSAVE state disabled
  could win ggml's best-score probe and fault at inference. Pumice instead
  maps each ggml variant name to the exact feature set its CMake table
  compiles in (`ggml/src/CMakeLists.txt`, `ggml_add_cpu_backend_variant`) and
  detects features with Rust's `is_x86_feature_detected!`, which verifies OS
  support via OSXSAVE/XGETBV for `avx`/`avx2`/`avx512*`/`avxvnni`. The best
  fully-usable tier wins and the plain `x64` module is the portable floor.
- AMX (`sapphirerapids`) is never selected on stable Rust: the AMX detection
  intrinsics are still unstable (`x86_amx_intrinsics`) and AMX additionally
  needs a per-process OS permission grant, so selecting it could not be
  verified safe. Sapphire Rapids CPUs fall back to the `zen4` tier, the same
  ISA without AMX.
- transcribe-cpp 0.3.1 exposes only directory-based backend loading, and
  ggml scores whatever the directory holds. To make ggml load exactly the
  chosen module and nothing else, the module is copied alone into a private
  per-process directory (`.pumice-cpu-variant-<pid>`) inside the trusted
  install dir and removed right after registration (after `dlopen` the
  mapping stays valid). ggml's CPUID-only scoring still runs on that one
  module but has nothing else it could pick instead.
- The selected variant is reported from ground truth: the ggml device
  description plus the mapped `libggml-cpu-*.so` file read from
  /proc/self/maps (the full pathname is preserved, spaces and the
  " (deleted)" suffix included). It appears in `serve`'s startup line and as
  a `backend` field in `file --json`, e.g. `"CPU (Intel(R) Core(TM)
  i5-14500T) [alderlake]"`.
- Init failures distinguish modules missing / staging failed / no usable CPU
  module, each with a recovery hint, in fixed text with no native error
  strings.
- `crates/transcription/scripts/build-cpu-variants.sh` builds this mode and
  stages a self-contained folder under `target/cpu-variants/stage/` (binary +
  every `.so` the build produced). The script resolves the exact
  `transcribe-cpp-sys` `out_dir` of the build just run (from `cargo
  --message-format=json` build-script-executed messages) so a fresh
  executable is never paired with stale cached libraries, and validates that
  libtranscribe, libggml, libggml-base and the x64 module are all present.
  The binary carries an `$ORIGIN` rpath emitted from
  `crates/transcription/build.rs`, gated on the feature, so the staged folder
  works from any location.
- The conservative portable baseline in `.cargo/config.toml`
  (`TRANSCRIBE_CMAKE_ARGS`, `force=false`) cannot suppress the variants: the
  script uses its own `CARGO_TARGET_DIR` (so CMake never reuses the static
  build's cache) and exports an explicit `TRANSCRIBE_CMAKE_ARGS` for this
  mode, which `force=false` lets win over the config value. The exported flags
  (`-DGGML_NATIVE=OFF -DTRANSCRIBE_USE_SYSTEM_BLAS=OFF`) are the safe subset;
  the feature itself sets `GGML_BACKEND_DL=ON`, `GGML_CPU_ALL_VARIANTS=ON`
  and `GGML_NATIVE=OFF`.
- CI: a Linux job in `.github/workflows/transcription.yml` clippy-checks the
  feature, runs its full test suite (module discovery, variant chooser, maps
  parsing), then builds and stages the mode, runs the `ldd` check and the
  Whisper tiny smoke test against the staged binary (asserting the `backend`
  field). Existing jobs are unchanged.

## Method

- Machine: server-1, Intel Core i5-14500T (20 threads, AVX2 + AVX_VNNI, no
  usable AVX-512), 35 W T-class part. The live `pumice-transcribe serve` on
  port 17568 and other services were running throughout — numbers are as
  measured on the live machine, not isolated. The CPU throttles under
  sustained all-core load, so consecutive runs drift upward.
- Model: `whisper-large-v3-turbo-Q8_0.gguf` (pinned revision `5eaf945c`).
  Sample: `crates/transcription/tests/fixtures/sample-pt.wav` (14.19 s).
- Command per run: `pumice-transcribe file SAMPLE --model-file MODEL
  --threads N --json`; timings are the envelope's own `model_load_ms` and
  `transcription_ms`. Each configuration ran 3 times, warm page cache (cache
  dropping needs root, unavailable).
- Reproduce: `crates/transcription/scripts/bench.sh --bin BIN --runs 3
  --threads "16"` (the 16-thread recommendation from the first sweep; the
  full sweep was `--threads "8 12 16 20"`).
- Expected transcript (exact-match checked on every run): "Esse aqui é um
  exemplo de texto em português. Então, eu estou dizendo que agora são
  meia-noite e 36, não está chovendo, está frio e não tem vento."

## Numbers

Warm transcription of the 14.19 s sample, Turbo Q8_0, 16 threads, both builds
re-measured after the fix (median of 3 runs; the staged cpu-variants binary
and the portable `engine` release binary, run alternately):

| Build | transcription_ms (3 runs) | median | model_load_ms (median) | Selected variant |
| --- | --- | --- | --- | --- |
| cpu-variants (staged) | 6925 / 6903 / 6965 | 6925 | 349 | alderlake |
| portable | 22941 / 24532 / 25900 | 24532 | 348 | — |

Speedup at 16 threads: 24532 / 6925 ≈ 3.5×. (The portable numbers drift
upward across consecutive runs; the throttling CPU was warmer by then. Both
builds ran back-to-back, 3 runs each, after the fix.)

Selected variant on server-1: `alderlake` (SSE4.2 + AVX + F16C + FMA + AVX2 +
BMI2 + AVX_VNNI — the best tier this CPU and OS can run; the AVX-512 tiers
are not usable on this CPU).

Target check (D1: warm Turbo Q8_0 under 14 s): met — 6.9 s median at 16
threads on the re-measured staged build (3.5× the portable median). The first
sweep's full table (8/12/16/20 threads) is in the git history; its qualitative
findings stand: **run the server with `threads: 16` on server-1** (20 threads
was the worst-scaling and noisiest setting on this hybrid P/E-core CPU).

Transcript: exact match in all runs of both builds.

## Evidence gathered on this machine

- Only the chosen module is dlopen'd: the startup line prints
  `cpu-variants: selected ggml CPU module libggml-cpu-alderlake.so`, and
  /proc/self/maps shows exactly one `libggml-cpu-*.so` mapped. The private
  staging directory is removed before the first request.
- The variant chooser is unit-tested with injected feature sets, independent
  of the host, including "CPU advertises AVX-512 but the OS has the state
  disabled" (the AVX-512/AMX tiers are not chosen) and "AVX state disabled"
  (VEX tiers are not chosen).
- `ldd` on the staged binary resolves `libtranscribe.so.0.3`,
  `libggml.so.0` and `libggml-base.so.0` from the staged folder itself.
- `serve` on the staged binary prints
  `loaded model 'tiny' in 167 ms (backend: CPU (...) [alderlake])` and
  answers `/v1/audio/transcriptions` with 200 and non-empty text.

## Limits and what is unverified

- **Verified vs unverified SIGILL safety**: on this host, detection and
  selection ran correctly and no module the CPU cannot execute was ever
  loaded. The OS-state check is std's `is_x86_feature_detected!`, which is
  exercised by the unit tests only through an injected detector; a real
  machine or guest with OS-disabled AVX state was not tested. Residual risk
  is a CPU/microcode that reports XGETBV state it does not honor.
- **Baseline fallback on a weaker CPU is not verified**: the mechanism
  (feature-gated table with the x64 floor) was unit-tested with synthetic
  feature sets down to plain x64, but no pre-AVX2 machine was exercised.
- **AMX is never selected** (see above); the `sapphirerapids` module is
  staged but unreachable on stable Rust.
- **Cold-cache numbers** (page cache dropped) were not measured; all runs are
  warm. Model load stays ~0.35–0.4 s either way.
- **Variance**: medians of 3 on a live, throttling machine. No p95, no
  sustained-load test; a long dictation session may look different from
  back-to-back 14 s clips.
- **Windows/macOS are out of scope** for this mode (no CI matrix changes);
  the feature is off by default and existing builds are untouched. The
  feature compiles on other OSes but only prints a warning that runtime
  selection is wired up on Linux; the Windows/macOS staging layout is not
  built or tested.
- **Only the `file` and `serve` flows were exercised**; cancellation and
  client-disconnect paths were not re-tested on the staged build (they do not
  touch backend selection).
- ABI mismatch / loader edge cases beyond this machine (glibc versions,
  hardened loaders) are untested. The init error categories cover missing or
  unloadable modules with fixed text, but cannot prove build identity of the
  staged native components beyond the script's staging-time validation.
