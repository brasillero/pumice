# Pipeline speed analysis (Codex, 2026-10-30, read-only)

Proposals, not owner decisions.

## Measured baseline

Runs inspected: Release preview 40980838699, 40980825178, 40446680228; CI 40980838739, 40980825298, 40983049578; Transcription engine 40980838776, 40980825218, 39604389611. Times exclude queue delays; runner minutes are summed job durations.

| Work | Time | Where it goes |
|---|---:|---|
| Release Windows bundle | 9m01–11m18 | 9m48 cargo/native build; setup and SDK under 40 s |
| Release Linux bundle | 5m35–8m46 | about 7m46 build, stage and audit |
| Release Metal bundle | 4m03–5m25 | build, stage and audit |
| Release core builds | Linux 2m28–2m41, Windows 4m14–5m40, ARM Mac about 3m, Intel Mac 4m36–8m14 | uncached release compile; Windows dependency check 47–72 s |
| npm staging | 35–42 s | packing 9–14 s, notices 5–6 s |
| npm smokes | Linux 16–28 s, Windows 44–59 s, Intel Mac 61–74 s, ARM Mac 98–115 s | macOS registry start 38–40 s; ARM backend smoke 35–42 s |
| CI | Linux 4m38–4m54, Mac 4m21–7m08, Windows 6m22–8m57 | compilation |
| Transcription CPU variants | 19m02–19m12 | clippy 355–364 s, tests about 355 s (5m42 compiling), staging 416–419 s |
| Transcription Metal | 18m25–19m06 | builds, backend smokes, 222–263 s informational diagnostics |

Release used 41.3–48.7 runner minutes, Transcription 64.7–68.6. Queue delays change the critical path (latest Release 17m59 elapsed against 13m08 on a quiet day).

## Cache bug

`Swatinem/rust-cache` runs `cargo metadata --all-features --format-version 1`, which fails with "depends on crate `transcribe-cpp-sys v0.3.1` multiple times with different names" (aliases documented at `crates/transcription/Cargo.toml:36`, declared at lines 75 and 82). The action catches the error, gets an empty package list and cleans every dependency artifact. Stored caches: CI about 1.5–2 KB per OS, bundles about 1.3–2.7 KB: hits on empty caches. The bundle jobs also default the cache to the parent `target/` despite a custom `CARGO_TARGET_DIR` (`.github/actions/windows-vulkan-bundle/action.yml:45`), which alone does not explain the empty caches. The core release builds have no Rust cache (`release.yml:116`).

## Ranked recommendations

| Rank | Change | Expected saving | Risk / effort / owner |
|---|---|---|---|
| 1 | Repair caching everywhere; add core release caches | warm runs 2–6 min off bundle builds, 1–3 min per CI OS; 10–20 runner minutes per Release | low risk with complete keys; medium; no owner decision |
| 2 | PR-scoped `concurrency` with `cancel-in-progress` | up to one whole obsolete run (Release 41–49, Transcription 65–69, CI 17–21 runner minutes) | low; no decision unless intermediate artifacts must be kept |
| 3 | Faster npm registry fixture (`scripts/npm_registry_fixture.py:49`) and Windows `dumpbin` lookup (`release.yml:156`) | about 30–35 s per macOS smoke, 40–65 s per Windows core build | low; no decision |
| 4 | Native compiler cache (sccache GHA backend or ccache; CMake launchers via `TRANSCRIBE_CMAKE_ARGS`; Windows generator to verify) | 2–5 min per warm native-heavy job | medium; no decision |
| 5 | Build the bundles once across Release and Transcription | 15–25 runner minutes when both run | medium/high; owner decision if the pinned/stable toolchain policy (`p14.3-npm-bundles/D4`) changes |
| 6 | Reuse native compilation in the CPU-variants job (`transcription.yml:151`, `build-cpu-variants.sh:33`) | 5–10 min of about 18m50 | medium; keep debug-profile coverage |
| 7 | Cache the Vulkan SDK only if a benchmark wins | at most about 20 s per job | low payoff |
| 8 | Narrower triggers or component reuse; main-only previews or less frequent Intel Mac | whole jobs | removes pre-merge proof; owner decision |

## Ideas not recommended now

- Main/release-only previews lose per-PR proof of packaged binaries, bundles, notices and installs. Path filters already exist (`release.yml:14`); narrowing `crates/**` risks missing shared dependencies.
- Intel Mac less often saves 5–8 runner minutes but is rarely on the critical path. ARM-to-Intel cross-compiling is worth a benchmark only if Intel still runs the archives and npm installs natively.
- Larger runners: no measured case; keep Ubuntu 22.04's glibc floor.
- nextest or test splitting: compilation dominates (66 of 106 s on Linux CI, 342 of 356 s in CPU variants); revisit after caching. Keep doctests and the `harness = false` test.
- Debug builds instead of release: shipped artifacts use LTO and one codegen unit (`Cargo.toml:125`); changing that changes what is validated.
- Moving the Metal diagnostics to their own job shortens the gating job by about 4 min but saves no minutes.

## Queued run 40983094232

Created 00:59:23 UTC; first job started 01:06:39 (7m16 queue); both ARM Mac jobs waited without runners. No repository concurrency setting explains it; account runner contention from overlapping runs is plausible but not established.
