# CPU-variants native build reuse (2026-10-30)

Scope: ci-speed #6, PR https://github.com/brasillero/pumice/pull/134, base `ci-speed`. Code measured at `d70c1b68a1965bd35aefcd8e8dd0b0fd436a4258`.

## Change and coverage

Build and stage the release binary first with the existing, unchanged `build-cpu-variants.sh`. Read that invocation's exact `transcribe-cpp-sys` install prefix with the existing `transcribe_sys_out` helper; pass it to debug clippy/tests through the pinned engine's supported `TRANSCRIBE_DIR` path. The pinned native build script already selects CMake Release in both Rust profiles. Its prebuilt path emits native link metadata without invoking CMake. Only the test step gets `LD_LIBRARY_PATH` for the shared engine; the relocation smoke gets no additional library search path.

The Cargo cache build identity changes to separate this job layout. PR A's cache key still includes the runner, Rust/native compiler, dependency lock and CMake settings. No compiler cache or launcher is added, so compiler-cache hit statistics do not apply.

Coverage removed: none. Debug Rust tests (including doctests and the custom managed harness), clippy `--all-targets -- -D warnings`, every release CPU variant, release LTO/one codegen unit, read-only staging and model smokes remain. No bundle scripts, staging audits, `bundle.json` validators, or other jobs change. The CPU-variants job itself has no `bundle.json` validation step; those checks remain in the untouched bundle jobs.

## Baseline

Source: `reports/2026-10-30-pipeline-speed.md` and fresh `gh run view <id> --json jobs,createdAt` reads. Seconds are GitHub job/step timestamps, excluding queue time. Queue is workflow creation to this job's start, not account-wide queue attribution.

| Run | Job | Queue | Clippy | Debug tests | Release staging |
| --- | ---: | ---: | ---: | ---: | ---: |
| 40980825218 | 1142 s | 2 s | 355 s | 355 s | 419 s |
| 40980838776 | 1152 s | 3 s | 364 s | 356 s | 416 s |
| 40983049588 | 1076 s | 384 s | 334 s | 334 s | 389 s |
| 40987501189 (PR A, same-day cold) | 690 s | 2 s | 209 s | 214 s | 251 s |

The old main CPU cache entry was 2299 bytes (`v0-rust-cpu-variants-transcription-cpu-variants-Linux-x64-01b81e85-7dcacfff`), consistent with the empty-cache bug in PR A.

## Cold and warm measurements

Run 40987714155 was superseded while local validation found the test loader-path requirement; it is excluded from the comparison. Corrected cold run: 40988030401, attempt 1, CPU job 123843739598. After that job passed and saved its cache, the remaining jobs of this PR's measurement run were canceled to permit a CPU-only warm rerun; GitHub refuses job reruns while the enclosing workflow is running. No other branches were canceled. This is a CPU-job measurement, not a passing full-workflow claim.

| Measurement | Cold | Warm |
| --- | ---: | ---: |
| Job (queue excluded) | 521 s (8m41) | 94 s (1m34) |
| Queue (attempt start to job start for rerun) | 47 s | 4 s |
| Cargo cache restore | 1 s, miss | 5 s, exact hit |
| Release staging | 419 s | 56 s |
| Clippy (debug) | 37 s | 3 s |
| Tests (debug) | 46 s | 19 s |
| Cargo cache prune/save | 7 s | 0 s (immutable exact hit) |

Cold restore log: `exact hit false, restored nothing`. Cargo output: release 6m58, dev 37.43 s, test compilation 32.97 s; the test step also runs the full suite. The job pruned `target` from 2134 to 1315 MiB and saved 338929087 compressed bytes (323.2 MiB) under `cargo-v1-transcription-cpu-variants-ubuntu26-X64-a43f7891bc60f834-0730c84d605224fc`. The separate 39107796-byte model cache was an exact hit.

Cold saving: 621–631 s against the report's 1142–1152 s baseline, 555 s against the recent main run, and 169 s against PR A's concurrent 690 s cold run. The same-day PR A build was substantially faster than the historical native builds; do not attribute the whole historical-to-new difference solely to this patch. Warm improvement versus this cold run: 427 s (7m07); against the historical baseline: 1048–1058 s (17m28–17m38). This combines native reuse with PR A's repaired dependency cache. No PR A warm CPU result was available for an isolated warm comparison.

Warm: same run, attempt 2, CPU job 123847002785, rerun with `gh run rerun 40988030401 --job 123843739598`. Attempt start 02:00:20 UTC, job start 02:00:24, complete 02:01:58. The exact 338929087-byte Cargo cache restored; no save was needed. Cargo release/dev/test compile times were 55.01/3.27/7.93 s. The 37 MiB model cache also hit. Both CPU attempts passed the staged-layout and read-only model smokes. Logs and timings can be retrieved with `gh run view --attempt 1` / `--attempt 2` (job logs also via the Actions job logs API).

Implementation is complete and measured; merge remains with the supervisor. The final notes-only push triggers fresh PR checks. Canceled unrelated jobs in the measurement attempt are not claimed as validated by this experiment.

## Local validation

- `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo test --workspace`: passed (1202 tests across harness summaries).
- Release staging: passed; 14 CPU variant directories; native CMake Release, all variants on, native tuning and system BLAS off. The staged binary registered a CPU backend successfully.
- Debug CPU-variants clippy: passed in 6.61 s using the release native install; tests: 464 passed, including the managed harness and doctest invocation. Both debug sys build outputs link the same release native prefix and contain no CMake build tree.
- Repeated release staging after the real cache-pruning helper: passed, compiling only the workspace Rust crates and retaining the installed native libraries (523 MiB before pruning, 398 MiB after).
- actionlint 1.7.12: passed. shellcheck was unavailable. Task-notes shape check: passed.
