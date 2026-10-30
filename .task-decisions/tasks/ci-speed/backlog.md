# Backlog — ci-speed

## Active

### #1 · In progress · Repair dependency caching and add caches to the core release builds
- type: Tech debt
- area: distribution
- origin: p14.3-npm-bundles#3
- ticket: —
- created: 2026-10-17

**Context.** Moved from `p14.3-npm-bundles#3`, which saw `v0-rust-linux-vulkan-…` cache entries of about 2.6 KiB against 304 MiB on 2026-10-16; under `p14.3-npm-bundles/D4` the release jobs also no longer share caches with `transcription.yml`. Cause, found on 2026-10-30: `Swatinem/rust-cache` runs `cargo metadata --all-features`, which fails because `crates/transcription/Cargo.toml` (lines 36, 75, 82) depends on `transcribe-cpp-sys` twice under different names; the action then cleans the cache with an empty package list, so every CI and bundle cache stores about 1–3 KB. The core release builds have no Rust cache at all (`release.yml:116`). Codex's measurements: `reports/2026-10-30-pipeline-speed.md`.

**To do.**
- [x] Replace the failing cleanup with explicit `actions/cache` entries (registry and dependency build dirs), or fix the metadata clash without changing the backend features.
- [x] Key by OS/arch, rustc, target, profile/features, native compiler/SDK, CMake flags and `Cargo.lock`; bump the cache namespace.
- [x] Map the bundle jobs' custom `CARGO_TARGET_DIR` explicitly.
- [x] Add caches to the core release builds.

**Acceptance criteria.**
- Caches store real dependency artifacts and a warm run is measurably faster (estimate 2–6 min off bundle builds, 1–3 min per CI OS).

### #2 · In progress · Cancel superseded runs of the same PR
- type: Improvement
- area: distribution
- origin: reports/2026-10-30-pipeline-speed.md
- ticket: —
- created: 2026-10-30

**Context.** No workflow declares `concurrency`, so every push starts a full CI, Release preview and Transcription engine run while the previous one keeps going (up to about 40–70 runner minutes each), and the queue delays the latest run.

**To do.**
- [x] PR-scoped `concurrency` with `cancel-in-progress` in the three workflows; never cancel `main`, other PRs or manual release runs.

### #3 · In progress · Speed up the npm registry fixture and the Windows dependency check
- type: Improvement
- area: distribution
- origin: reports/2026-10-30-pipeline-speed.md
- ticket: —
- created: 2026-10-30

**Context.** `scripts/npm_registry_fixture.py:49` scans whole tarballs (`getmembers()`) and re-reads manifests, costing about 38–40 s per macOS smoke; the Windows dependency step searches `**\dumpbin.exe` recursively (47–72 s, `release.yml:156`). Same checks, faster lookups.

**To do.**
- [x] Stream until `package.json` and reuse the index.
- [x] Use a targeted MSVC tool path, keeping the failure checks and import audit.

### #4 · In progress · Native compiler cache for the transcription engine builds
- type: Improvement
- area: distribution
- origin: reports/2026-10-30-pipeline-speed.md
- ticket: —
- created: 2026-10-30

**Context.** Most bundle time is compiling transcribe.cpp/ggml through CMake. `RUSTC_WRAPPER=sccache` does not cover CMake's C/C++; launchers must go through `TRANSCRIBE_CMAKE_ARGS`. Windows uses the Visual Studio generator, so launcher support must be verified (maybe Ninja), and `/O2`, static CRT and the audits rechecked.

**To do.**
- [ ] Trial sccache (GHA backend) or ccache on Linux and macOS, measure hit rates, then Windows.

### #5 · To do · Stop building the transcription bundles twice
- type: Improvement
- area: distribution
- origin: reports/2026-10-30-pipeline-speed.md
- ticket: —
- created: 2026-10-30

**Context.** Release preview and Transcription engine build the same three bundles separately (about 15–25 runner minutes when both run). Release pins Rust, Transcription follows stable (`p14.3-npm-bundles/D4`), so sharing needs an exact build identity.

**To do.**
- [x] Owner: may the two workflows share bundles when the toolchain matches, keeping stable coverage when it differs? Yes (D1).
- [ ] Share the bundles by exact build identity.

### #6 · To do · Reuse native compilation inside the CPU-variants job
- type: Improvement
- area: transcription
- origin: reports/2026-10-30-pipeline-speed.md
- ticket: —
- created: 2026-10-30

**Context.** Clippy, tests and staging compile the engine three times with different settings (about 19 min in total; `transcription.yml:151`, `build-cpu-variants.sh:33`). Align them or link debug and release Rust against one native build, keeping debug-profile coverage.

**To do.**
- [ ] Measure, then align settings or share the native install.

### #8 · To do · Run fewer previews or Intel Mac builds
- type: Improvement
- area: distribution
- origin: reports/2026-10-30-pipeline-speed.md
- ticket: —
- created: 2026-10-30

**Context.** Limiting Release preview to `main`/release branches, narrower path filters, or building Intel Mac less often save runner minutes but drop per-PR proof of packaged binaries, bundles and installs. Intel Mac costs about 5–8 runner minutes but is rarely on the critical path.

**To do.**
- [x] Owner: decide once the cache and concurrency fixes are measured. Authorized (D1).
- [ ] Pick the variant from the measurements of #1–#4.

## Deferred

### #7 · Deferred · Cache the Vulkan SDK install
- type: Improvement
- area: distribution
- origin: reports/2026-10-30-pipeline-speed.md
- ticket: —
- created: 2026-10-30
- reason: at most about 20 s per job; only worth it if a benchmark wins

**Context.** The SDK install takes about 19 s on Windows and 23 s on Linux, before the cost of restoring a cache.

**To do.**
- [ ] Benchmark a cached install against the current one.

## Closed
