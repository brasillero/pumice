# P7 — Runtime CPU variants for transcription

Status: implemented on `p7-transcribe-cpu-variants` (local only, not merged).
Target and authorization: `.task-decisions/tasks/p7-transcribe-cpu-variants/D1`
(warm Turbo Q8_0 on the 14.19 s sample under 14 s on server-1, same
transcript, portable baseline kept).

## What was built

- New opt-in Cargo feature `cpu-variants` on `pumice-transcription`
  (`= ["engine", "transcribe-cpp/dynamic-backends"]`). The default `engine`
  build is unchanged: one static binary with the portable x86-64 baseline.
- With the feature, the engine links as shared libraries plus one ggml CPU
  module per x86 ISA tier (`GGML_CPU_ALL_VARIANTS`, 16 modules from baseline
  `x64` to `sapphirerapids`). At startup the binary loads the modules with
  `transcribe_cpp::init_backends_with(dir, BackendMask::CPU)` where `dir` is
  only the executable's own directory or its `lib/` subdirectory — never the
  current directory, PATH or ggml's default search paths. Registration is
  restricted to CPU backends, so no GPU driver can be pulled in.
- ggml scores every module against the running CPU and dlopens the best;
  modules the CPU cannot run score 0 and are skipped, with the plain `x64`
  module always scoring as the portable floor — a missing feature degrades
  instead of raising SIGILL. The score functions live in a separate object
  library compiled without arch flags (`ggml/src/ggml-cpu/CMakeLists.txt`),
  so scoring itself cannot execute an unsupported instruction.
- The selected variant is reported from ground truth: the ggml device
  description plus the mapped `libggml-cpu-*.so` file read from
  /proc/self/maps (the safe bindings expose no direct "which variant" query).
  It appears in `serve`'s startup line and as a `backend` field in
  `file --json`, e.g. `"CPU (Intel(R) Core(TM) i5-14500T) [alderlake]"`.
- `crates/transcription/scripts/build-cpu-variants.sh` builds this mode and
  stages a self-contained folder under `target/cpu-variants/stage/` (binary +
  every `.so` the build produced). The binary carries an `$ORIGIN` rpath
  emitted from `crates/transcription/build.rs`, gated on the feature, so the
  staged folder works from any location.
- The conservative portable baseline in `.cargo/config.toml`
  (`TRANSCRIBE_CMAKE_ARGS`, `force=false`) cannot suppress the variants: the
  script uses its own `CARGO_TARGET_DIR` (so CMake never reuses the static
  build's cache) and exports an explicit `TRANSCRIBE_CMAKE_ARGS` for this
  mode, which `force=false` lets win over the config value. The exported flags
  (`-DGGML_NATIVE=OFF -DTRANSCRIBE_USE_SYSTEM_BLAS=OFF`) are the safe subset;
  the feature itself sets `GGML_BACKEND_DL=ON`, `GGML_CPU_ALL_VARIANTS=ON`
  and `GGML_NATIVE=OFF`.
- CI: a new Linux job in `.github/workflows/transcription.yml` builds and
  stages the mode, runs the `ldd` check and the existing Whisper tiny smoke
  test against the staged binary (asserting the `backend` field). Existing
  jobs are unchanged.

## Method

- Machine: server-1, Intel Core i5-14500T (20 threads, AVX2 + AVX_VNNI, no
  AVX-512), 35 W T-class part. The live `pumice-transcribe serve` on port
  17568 and other services were running throughout — numbers are as measured
  on the live machine, not isolated. The CPU throttles under sustained
  all-core load, so consecutive runs drift upward; single isolated runs are
  faster than sweep medians.
- Model: `whisper-large-v3-turbo-Q8_0.gguf` (pinned revision `5eaf945c`).
  Sample: `crates/transcription/tests/fixtures/sample-pt.wav` (14.19 s).
- Command per run: `pumice-transcribe file SAMPLE --model-file MODEL
  --threads N --json`; timings are the envelope's own `model_load_ms` and
  `transcription_ms`. Each configuration ran 3 times, warm page cache (cache
  dropping needs root, unavailable). Sweep: N in {8, 12, 16, 20}.
- Reproduce: `crates/transcription/scripts/bench.sh --bin BIN --runs 3
  --threads "8 12 16 20"`.
- Expected transcript (exact-match checked on every run): "Esse aqui é um
  exemplo de texto em português. Então, eu estou dizendo que agora são
  meia-noite e 36, não está chovendo, está frio e não tem vento."

## Numbers

Warm transcription of the 14.19 s sample, Turbo Q8_0 (median of 3 runs;
`model_load_ms` ~350–395 in all configurations, both builds):

| Threads | Portable (ms) | CPU variants (ms) | Speedup |
| --- | --- | --- | --- |
| 8 | 31 054 (27 568–33 022) | 11 089 (11 030–11 092) | ~2.9× |
| 12 | 26 988 (26 830–27 010) | 8 850 (8 785–9 488) | ~3.0× |
| 16 | 27 010 (26 978–27 173) | 7 992 (7 926–8 007) | ~3.4× |
| 20 | 25 553 (25 120–25 400) | 7 431–13 133 (noisy) | ~3.4× |

Single isolated runs (nothing else starting up): portable 18 557 ms,
cpu-variants 6 306 ms (~2.9×) — matching the earlier "~19 s portable"
figure from `p6-transcribe-http` research.

Selected variant on server-1: `alderlake` (SSE4.2 + AVX + F16C + FMA + AVX2 +
BMI2 + AVX_VNNI — the best tier this CPU can run; the AVX-512 tiers scored 0).

Target check (D1: warm Turbo Q8_0 under 14 s): met at every thread count;
best configuration is 16 threads (~8.0 s, also the most consistent). 20
threads is the worst-scaling and noisiest setting on this hybrid P/E-core
CPU — more threads is not faster here. **Recommendation: run the server with
`threads: 16` on server-1.**

Transcript: exact match in all 24 runs of both sweeps.

## Evidence gathered on this machine

- All 16 modules (including the AVX-512/AMX tiers) were dlopen'd during
  scoring on this non-AVX-512 CPU with no SIGILL — the score-then-select path
  is exercised for real every startup.
- `ldd` on the staged binary resolves `libtranscribe.so.0.3`,
  `libggml.so.0` and `libggml-base.so.0` from the staged folder itself.
- `serve` on the staged binary prints
  `loaded model 'tiny' in 167 ms (backend: CPU (...) [alderlake])` and
  answers `/v1/audio/transcriptions` with 200 and non-empty text.

## Limits and what is unverified

- **Baseline fallback on a weaker CPU is not verified**: the mechanism is
  ggml's score-based selection (x64 always scores ≥ 1), but this machine runs
  `alderlake`, so degrading to `haswell`/`x64` on an older or AMD CPU was not
  exercised. Same for the "all variants score 0" corner (no such x86-64 CPU
  exists for the `x64` tier, so it cannot happen in practice).
- **SIGILL safety** rests on ggml's design (score functions compiled without
  arch flags; unsupported modules never register) plus the dlopen observation
  above; an actual pre-AVX2 CPU was not tested.
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
  hardened loaders) are untested.
