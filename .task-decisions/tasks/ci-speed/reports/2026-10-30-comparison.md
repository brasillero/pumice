# CI speed comparison (2026-10-30)

All eight items have measured outcomes: #1–#3 in PR #133, #4 discarded after a compiler-cache trial, #7 implemented in PR #136, #5/#8 implemented as exact-identity component reuse in PR #137, and #6 measured separately in PR #134. PRs #133, #134 and #136 are merged; PR #137 finishes the task with the review fixes described in "Final head of PR #137". Coverage removed: **none**.

## Measurement method and limits

Wall time runs from first job start to last job completion, including contention between jobs. Initial queue is attempt creation to first job start and is shown separately. Runner minutes sum job service times, without OS billing multipliers. The service critical path sums dependent job durations, excluding queue gaps; the coordinator's producer wait counts as service and runner time. Per-job ready-to-start queues and API step durations appear in the appendix. Durations are integer seconds from GitHub timestamps; composites are outer steps, with internal cache/compiler details taken from logs. Each row is one attempt, never a mixture of retries.

These are single samples, not controlled benchmarks. `ubuntu-latest` moved to the ubuntu26 image between the historical baseline and these experiments. CPU variants fell from 19m12 to 11m30 already in A's cold run, before #6. Host speed, toolchain updates, scheduler contention, immutable-cache save races and eviction confound comparisons. Linux Vulkan bundles retain their explicit Ubuntu 22.04 runner and glibc floor.

A cold means all Cargo keys missed. A warm restores all seven Release and all five Transcription Cargo keys; its Mac CI cache missed because the original test failure prevented saving it. B/C first runs are **mixed-cache**, not clean cold experiments. B's cold compiler-cache jobs were completed within superseded workflows, so they are reported as jobs rather than successful full workflows. Extra full cold rebuilds were not forced near the cache limit. B/C warm Release attempts reuse the same code heads and SDKs; individual Rust cache eviction is reported below.

## Workflow measurements

| Sample | Run / attempt | Wall | Runner min | Initial queue | Service critical path |
| --- | --- | ---: | ---: | ---: | ---: |
| Baseline CI | [40980838739 / 1](https://github.com/brasillero/pumice/actions/runs/40980838739/attempts/1) | 8m57 | 20.98 | 0m02 | 8m57 |
| Baseline Release | [40980838699 / 1](https://github.com/brasillero/pumice/actions/runs/40980838699/attempts/1) | 17m56 | 45.33 | 0m02 | 14m06 |
| Baseline Transcription | [40980838776 / 1](https://github.com/brasillero/pumice/actions/runs/40980838776/attempts/1) | 19m13 | 68.63 | 0m02 | 19m12 |
| A cold CI (failed) | [40987501204 / 1](https://github.com/brasillero/pumice/actions/runs/40987501204/attempts/1) | 9m11 (failed) | 17.23 (failed) | 0m02 | excluded |
| A cold Release | [40987501170 / 1](https://github.com/brasillero/pumice/actions/runs/40987501170/attempts/1) | 13m56 | 46.33 | 0m02 | 13m44 |
| A cold Transcription | [40987501189 / 1](https://github.com/brasillero/pumice/actions/runs/40987501189/attempts/1) | 14m37 | 57.90 | 0m02 | 14m35 |
| A warm CI | [40987501204 / 3](https://github.com/brasillero/pumice/actions/runs/40987501204/attempts/3) | 12m39 | 10.42 | 2m26 | 4m31 |
| A warm Release | [40987501170 / 2](https://github.com/brasillero/pumice/actions/runs/40987501170/attempts/2) | 17m38 | 24.28 | 3m32 | 7m29 |
| A warm Transcription | [40987501189 / 2](https://github.com/brasillero/pumice/actions/runs/40987501189/attempts/2) | 14m08 | 26.75 | 0m56 | 14m01 |
| A DNS-fix CI | [40989357682 / 2](https://github.com/brasillero/pumice/actions/runs/40989357682/attempts/2) | 5m37 | 13.52 | 0m29 | 5m32 |
| A DNS-fix Release | [40989357683 / 2](https://github.com/brasillero/pumice/actions/runs/40989357683/attempts/2) | 15m45 | 22.18 | 0m02 | 6m13 |
| A DNS-fix Transcription | [40989357726 / 2](https://github.com/brasillero/pumice/actions/runs/40989357726/attempts/2) | 18m16 | 28.92 | 0m21 | 15m11 |
| B first CI | [40993089644 / 1](https://github.com/brasillero/pumice/actions/runs/40993089644/attempts/1) | 6m26 | 12.57 | 0m57 | 6m26 |
| B first Release | [40993089663 / 1](https://github.com/brasillero/pumice/actions/runs/40993089663/attempts/1) | 11m20 | 32.92 | 1m10 | 11m12 |
| B first Transcription | [40993089668 / 1](https://github.com/brasillero/pumice/actions/runs/40993089668/attempts/1) | 18m10 | 38.18 | 0m17 | 17m31 |
| B warm Release | [40993089663 / 2](https://github.com/brasillero/pumice/actions/runs/40993089663/attempts/2) | 6m28 | 23.37 | 0m02 | 6m13 |
| C first CI | [40993159760 / 1](https://github.com/brasillero/pumice/actions/runs/40993159760/attempts/1) | 9m17 | 16.62 | 0m51 | 9m17 |
| C first Release | [40993159759 / 1](https://github.com/brasillero/pumice/actions/runs/40993159759/attempts/1) | 14m41 | 31.38 | 0m38 | 14m27 |
| C first Transcription | [40993159761 / 1](https://github.com/brasillero/pumice/actions/runs/40993159761/attempts/1) | 18m42 | 47.70 | 1m18 | 16m25 |
| C warm Release | [40993159759 / 2](https://github.com/brasillero/pumice/actions/runs/40993159759/attempts/2) | 8m44 | 23.08 | 0m02 | 8m31 |

### Slow-job service times (seconds)

Queues are excluded here and listed per job in the appendix. B/C first samples have mixed caches; a dash means that workflow was not rerun.

**CI**

| Job | Baseline | A cold | A warm | B first | B warm | C first | C warm |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| windows-latest | 537 | 551 | 271 | 386 | — | 557 | — |
| macos-latest | 428 | 199 | 185 | 194 | — | 242 | — |
| ubuntu-latest | 294 | 284 | 169 | 174 | — | 198 | — |

**Release preview**

| Job | Baseline | A cold | A warm | B first | B warm | C first | C warm |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| aarch64-apple-darwin | 183 | 152 | 162 | 153 | 173 | 200 | 175 |
| Transcription bundle (linux-x64, Vulkan) | 526 | 506 | 111 | 565 | 97 | 63 | 55 |
| x86_64-unknown-linux-musl | 148 | 150 | 126 | 105 | 120 | 123 | 158 |
| Transcription bundle (darwin-arm64, Metal) | 243 | 264 | 121 | 268 | 164 | 29 | 24 |
| x86_64-apple-darwin | 276 | 476 | 275 | 298 | 264 | 303 | 390 |
| Transcription bundle (win32-x64, Vulkan) | 678 | 671 | 177 | 159 | 184 | 81 | 54 |
| x86_64-pc-windows-msvc | 340 | 239 | 170 | 205 | 178 | 175 | 280 |
| Locate shared transcription bundles | — | — | — | — | — | 678 | 5 |

**Transcription engine**

| Job | Baseline | A cold | A warm | B first | B warm | C first | C warm |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Transcription runtime CPU variants (Linux) | 1152 | 690 | 99 | 99 | — | 674 | — |
| Transcription Vulkan preview bundle (Linux x64) | 775 | 753 | 214 | 670 | — | 772 | — |
| Transcription Metal preview bundle (macOS arm64) | 1105 | 871 | 841 | 1051 | — | 985 | — |
| Transcription Vulkan preview bundle (Windows x64) | 852 | 875 | 382 | 384 | — | 337 | — |
| Transcription CPU engine smoke (Linux) | 234 | 285 | 69 | 87 | — | 94 | — |


The historical service critical path was the Windows CI job; Release's Windows bundle → npm packaging/install; and Transcription's CPU-variants job. A warm shifts Release to core archives/npm installs and Transcription to Metal backend/runtime checks. The first C Release path is the producer coordinator → consumers → npm packaging/install. The warm coordinator takes only five seconds, so core archives dominate again. Low runner minutes do not guarantee low observed wall time when later jobs queue.

### Cache evidence for the measured attempts

Counts refer to explicit Cargo-cache restores, not model/SDK keys. Skipped consumer restores are reported separately.

| Attempt | Cargo exact hits / restores | Other proof |
| --- | --- | --- |
| A cold | 0 across all three workflows | all keys missed |
| A warm CI | 2/3 | Mac original failed before saving |
| A warm Release | 7/7 | real core/native dependency artifacts |
| A warm Transcription | 5/5 | Release-first immutable entries can lack debug artifacts |
| B first Transcription | 3/5 | Linux/Metal missed; both SDKs restored |
| B warm Release | 7/7 | both installed SDK keys restored |
| C first Transcription | 2/5 | Windows/CPU-engine hit; Linux/Metal/CPU-variants missed |
| C warm Release | 2/4 core; all 3 bundle restores skipped | Linux/Windows core evicted; 3/3 artifact reuse; SDKs hit |

The C warm consumer run is warm for shared artifacts and SDKs, but mixed for Rust core caches. In particular, Intel core service rose 264 → 390 seconds despite an exact hit, and two other core caches missed. This explains why the 307-second changed-portion saving yields only about 17 seconds lower whole-workflow runner use and slower observed wall time.

## PR A: repaired caching, concurrency and fixture work

PR [#133](https://github.com/brasillero/pumice/pull/133), code head `9c04118a3f45beb3efc5b52e847ad99d23549cf1`, repairs the broken metadata-based cache cleanup with explicit caches and adds core release caches. Keys include Rust/native toolchains, backend, SDK/configuration, scripts and Cargo.lock. Real compressed dependencies replace the old approximately 2 KB entries. Cold A used `04e22cb`; its warm reruns are CI attempt 3, Release/Transcription attempt 2 above. Linux and Windows CI hit, Mac missed; Release 7/7 and Transcription 5/5 Cargo keys hit exactly. Cold A keys all missed.

A warm vs historical baseline reduces runner time CI 20.98 → 10.42 min, Release 45.33 → 24.28 min, Transcription 68.63 → 26.75 min. Observed wall times were 12m39, 17m38 and 14m08: queue noise prevents treating the runner-time savings as the same wall-time reduction. The Mac CI miss makes the CI row a partially warm result.

The first fixture patch left Mac startup at 35–36 seconds. Investigation confirmed HTTPServer.server_bind invokes reverse DNS through socket.getfqdn. Commit 336ad58 binds numeric loopback through TCPServer and retains the fixture's URL/server fields; f9e461d adds a no-DNS regression check. At A's final head, **both** ARM and Intel Mac registry startup steps take **2 seconds**. Targeted dumpbin lookup takes 0–1 seconds versus 47–72 seconds historically, while keeping the actual CRT import audit.

The original macOS failure in eof_during_a_cooperative_inference_drains_and_exits_cleanly took 2.03 seconds against the two-second GRACE. Old-head warm CI and current-head CI both passed, as did B/C and local workspace tests; no repeat was observed. Nothing in A changes inference shutdown or GRACE. Evidence supports a timing flake; the test was not loosened.

### Concurrency retry caveat

Re-running older 04e22cb attempts joined the same PR concurrency group and canceled newer-head runs at 336ad58/9c04118. The older warm attempts were allowed to finish, then all three 9c04118 attempts were rerun successfully. No pushes to A were made during those reruns. Retain PR-scoped cancellation: adding github.sha would prevent normal pushes canceling older commits, while splitting run_attempt would allow outdated retries to survive newer pushes. There is no cheap group-only change preserving both semantics. Operational constraint: serialize historical measurement reruns and finish by validating the current head. No main or #134 run was canceled by this work.

## PR B: compiler-cache trial and installed SDK cache

PR [#136](https://github.com/brasillero/pumice/pull/136), final code head `a5590aea3ebd8d43591a880105d211b9ecdd80cf`, retains only the installed Vulkan SDK improvement. #4 is **Discarded with measurements**; the final workflows have no compiler-cache action, launcher or native-cache storage.

| Compiler-cache sample | Calls | Hits | Misses | Evidence |
| --- | ---: | ---: | ---: | --- |
| Cold Linux Release | 531 | 0 | 531 | run 40989214048, job 123847338621 |
| Cold Metal Release | 160 | 0 | 160 | run 40989214048, job 123847338668 |
| Warm Linux Release | 0 | 0 | 0 | run 40992098961, job 123857206433 |
| Warm Linux Transcription | 0 | 0 | 0 | run 40992098958, job 123857059029 |

Linux's 1,062 storage misses are lookup operations, not compiler calls. Compressed trial caches were about 22 MiB Linux / 8 MiB Metal. Cold setup added about 14 seconds on Linux. Warm production jobs restored the native install from Cargo and made no C/C++ calls, so compiler caching supplied no measured incremental win. The Linux/macOS launcher trial used TRANSCRIBE_CMAKE_ARGS with full compiler/backend/SDK/script identities. Windows retained Visual Studio because launchers require Makefile/Ninja; no generator switch, /O2 or CRT change was justified after the Linux result. See [native-cache outcome](2026-10-30-native-cache-outcome.md). The full cold trial was superseded; its completed jobs are not presented as a green workflow.

#7 is **Done**: benchmark run 40987788551 found Linux archive restore plus unpack/verify 26 s → installed restore 6 s, and Windows installer restore plus install/verify 35 s → installed restore 11 s (+2 s required-file probe). Compressed old/installed cache bytes: Linux 313,376,180 / 505,196,271; Windows 304,812,970 / 437,240,281. Cold saves cost about 8/10 seconds. Misses still download and SHA-256 verify pinned SDKs; hits retain tool/file preflight checks.

**Windows restore proof:** run 40991092167, job 123853796385, restored the installed SDK at 02:22:36 UTC. The logged conditional was `if ('true' -ne 'true')`, so Start-Process/installer was skipped. Cargo completed optimized compilation at 02:24:24 and DLL/static CRT audit passed at 02:24:33; job succeeded. Linux restored-SDK builds passed in jobs 123857206433 and 123857059029. Final B full workflows and warm Release also passed using the unchanged SDK-cache implementation. PR #135 was closed unmerged, ci-speed-bench deleted, and both bench entries 9734650628/9734667330 removed (942,436,552 bytes).

## PR C: #5 and #8 component reuse

PR [#137](https://github.com/brasillero/pumice/pull/137), measured code head `edee02f24c24ebe717be11930b6f6844cbf08b57`, publishes each audited Transcription bundle before its later checks finish. A separate Release coordinator locates the same PR/head producer, leaving core archive builds independent. Consumers compare the entire source tree plus Rust/Cargo, target/backend/features, native compiler, SDK, CMake/environment, scripts and runner-image identity. Only exact matches are extracted. Absent producers, failed/canceled producers without a complete artifact set, and identity differences build locally. Producer tests that fail after early upload still block merge even if Release has already reused the audited stage. Release remains pinned; Transcription follows stable under p14.3-npm-bundles/D4. When stable diverges, the identities differ and both toolchains compile independently. Offline tests cover identity changes, manual runs, artifact discovery and failed-producer fallback. Manual previews build independently.

All three consumers reused successfully in both measured C Release attempts: build and Cargo-restore substeps were skipped. The producer's remaining tests still gate the PR; every native archive/package/install check still runs. First-run shared artifact bytes: Linux 24,267,896, Metal 4,448,983, Windows 21,896,434; artifacts expire after two days. Windows path normalization in a3ac96c fixed an initial tar D:-as-remote interpretation before successful final runs.

### Net #5/#8 savings, including coordinator

First (mixed caches): B bundle service **992 s** versus C consumers **173 s + 678 s coordinator = 851 s**: net **141 s (2.35 runner min)** saved in the changed portion. Whole Release runner time 32.92 → 31.38 min; wall 11m20 → 14m41. Other-job and queue variation explains the difference between changed-portion and whole-workflow deltas.

Warm Release: B bundle service **445 s** versus C consumers **133 s + 5 s coordinator = 138 s**: net **307 s (5.12 runner min)** saved in the changed portion. Whole Release runner time 23.37 → 23.08 min; wall 6m28 → 8m44. Other-job and queue variation explains the difference between changed-portion and whole-workflow deltas.

The first C coordinator spent 11m18 waiting for producers; observed Release wall time worsened despite a small runner saving. Only a re-run of the same head avoids that wait, because its producer artifacts already exist; every new push starts a new producer, and the coordinator waits for it again (11m12 at c4960e1, see below), even when a consumer then finds a different identity and builds locally. This is a cache/coordinator tradeoff, not an unconditional wall-time win. Paired whole-workflow sums must also account for producer cache states: B Transcription was 38.18 runner min versus C 47.70, with C's CPU-variants job cold (674 s versus B 99 s) although its code is unchanged. That difference cannot be attributed to component reuse. No combined all-warm B/C Transcription rerun was forced near eviction; the first fully successful Transcription runs and warm consumer proof are provided.

#8 is **Done through the same measured reuse**, with no additional saving double-counted. No preview is removed, Intel is still built and installed on an Intel runner, and ARM→Intel cross-compilation was not introduced. Precise filtering was not needed. Explicit dependency map: release.yml retains Rust crates/root source/manifests, .cargo, packaging/notices/npm helpers, shared actions and fixtures; it adds transcription.yml and the sharing helper/tests because producer behavior affects release consumers. transcription.yml retains its own transcription/manifests/.cargo/fixture/action inputs and adds sharing helper/tests. Every applicable PR keeps its proof before merge.

## Final head of PR #137

Review of c4960e1 found that the key change it made for bundle provenance (runner image revision and CMake in every Cargo key) reached all twelve Cargo caches, that fork PRs could never find a producer, that a failed coordinator job would skip the bundle jobs and npm packing, and that the docs still described Release previews as self-contained. The final head changes:

- **Cache keys.** Only the three bundle caches (`bundle-linux-vulkan`, `bundle-windows-vulkan`, `bundle-macos-metal`, `provenance: true` in their composite actions, `ci_cargo_cache.py key --provenance`) include the image revision and CMake; CI ×3, Release core ×4, the CPU engine and the CPU variants keep keys that survive weekly runner-image releases. The bundle identity is derived from the same `cache_identity(..., provenance=True)`, so a bundle cache and the identity it is published under cannot drift apart. Tests: a non-bundle key ignores `ImageVersion` and CMake; the layered old-image/old-CMake regression test still passes for the bundle caches; the bundle identity asks for provenance.
- **Fork PRs.** GitHub lists no `pull_requests` for runs from forks, so the coordinator matches the producer run by head commit and head repository (`head_sha`, `head_repository.full_name` against the event's `pull_request.head`). The identity check still guards what is reused. Test: a fork producer is found among another fork's and a deleted fork's runs at the same commit.
- **Coordinator failure.** `coordinate()` turns any exception into local builds with a `::warning::`; the locate step has `continue-on-error` and a 22-minute step timeout (the lookup's own deadline is 20 minutes in all) inside the job's 25; the job itself has `continue-on-error`. The bundle jobs run on `!cancelled() && needs.package-tests.result == 'success'`, so a lost or timed-out coordinator means an empty producer and local builds. Because `success()` also counts indirect needs, `npm-pack` checks its direct needs (still every bundle job, `checksums` and `package-tests`), and both npm smoke jobs check `npm-pack`. Tests cover unexpected errors and the workflow conditions.
- **Docs.** `docs/install.md` documents `actions: read`, bundle reuse on PR previews, and that releases are cut only from a manual-dispatch run (the coordinator returns no producer for any event but `pull_request`, so those runs build every bundle).

At c4960e1 (runs 40999873850, 40999873892 and 40999873873, attempt 1, a new push): CI 8m38 wall / 17.6 runner min, Release preview 17m57 / 37.7, Transcription engine 20m55 / 56.1. The coordinator waited **11m12** for the producer. Linux and Windows Vulkan reused the producer's stages (34 s and 56 s jobs); Metal built locally (4m46, Cargo cache miss) because the two macOS 15 jobs landed on different image revisions during a rollout: producer `20260907.0337.1` with CMake 4.4.3, consumer `20261028.0489.1` with CMake 4.4.4, Rust 1.99.1 on both. That is the intended fallback.

The final head's own runs are reported with PR #137 rather than here, because recording them in these notes would change the head they measure.

## #6: separate approved CPU-variants PR

PR [#134](https://github.com/brasillero/pumice/pull/134), measured code `d70c1b68a1965bd35aefcd8e8dd0b0fd436a4258`. Source: origin/ci-speed-cpu-variants:.task-decisions/tasks/ci-speed/reports/2026-10-30-cpu-variants.md (read without editing that branch). This change is not integrated into B/C's measured source, so no synthetic combined workflow result is claimed.

| CPU-variants measurement | Historical cold | A same-day cold | #6 cold | #6 warm | A warm comparator |
| --- | ---: | ---: | ---: | ---: | ---: |
| Job | 19m02–19m12 | 11m30 | 8m41 | 1m34 | 1m39 |
| Queue | 2–3 s | 2 s | 47 s | 4 s | see appendix |
| Release staging | 416–419 s | 251 s | 419 s | 56 s | — |
| Debug clippy | 355–364 s | 209 s | 37 s | 3 s | — |
| Debug tests | 355–356 s | 214 s | 46 s | 19 s | — |

Cold/warm samples are run 40988030401 attempts 1/2, jobs 123843739598/123847002785. Cache 338,929,087 bytes (323.2 MiB), exact warm hit. Native installation is reused across Rust profiles while debug tests remain debug. Other jobs in the experimental workflow were canceled by its owner, so these are successful job measurements, not full-workflow claims. Isolated same-day cold saving versus A is 169 s; warm saving only 5 s. The older-image 19-minute baseline is not a causal estimate for #6.

## Cache sizes, cleanup and steady-state budget

API `repos/brasillero/pumice/actions/caches` snapshots grew from 8,790,736,759 bytes / 92 entries to over 10 GB. Cleanup removed 50 obsolete broken/merged-PR entries (654,624,475 bytes), three native trial entries (54,659,003), and eight superseded manual/trial Cargo entries (1,487,601,673): **2,196,885,151 bytes**, plus the earlier **942,436,552-byte** benchmark cleanup. An additional empty superseded native trial entry was removed. Current-head entries were retained, #134 caches were left alone, and no run on main was canceled. Removing the unhelpful compiler-cache implementation also removes rolling native-key growth.

After that cleanup, usage was 8,875,399,345 bytes / 43 entries; new current-head caches then brought a later snapshot to **10,788,066,953 bytes / 56 entries**. Final snapshots may change as GitHub evicts entries. Scope totals at that later snapshot: main 852,835,863; A 2,767,763,381; #134 216,947,945; B 3,710,658,606; C 3,239,861,158 bytes. Missing previously saved C Windows/engine keys demonstrate eviction; a nominal warm rerun is not automatically an exact hit. Cache scopes cannot inherit from a stacked PR's base branch; after merging to main, normal PRs can inherit main's immutable entries.

| Steady-state storage component (one source/toolchain set) | Compressed estimate | Workflow owner |
| --- | ---: | --- |
| Three OS CI dependencies | 1.15 GB | CI |
| Four release core targets | 0.41 GB | Release |
| Three native bundles (larger producer caches) | 0.53 GB | Transcription; shared with Release |
| CPU engine | 0.335 GB | Transcription |
| CPU variants after #6 | 0.339 GB | Transcription |
| Installed SDKs | 0.942 GB | shared Linux/Windows bundles |
| Model fixture versions across OS/cache paths | 0.313 GB | shared smokes/engine |
| Bun | 0.036 GB | Release |
| Native compiler cache | 0 | discarded |
| **Unique total** | **about 4.06 GB (3.78 GiB)** | sum without double-counting |

Per-workflow footprint: CI ~1.15 GB; Release core/Bun/npm fixture copies ~0.53 GB plus shared SDK/bundle/model entries; Transcription ~1.20 GB Cargo plus SDK/model entries. Release and Transcription together use the same SDK/native-bundle identities after merge; their figures are not additive. The original immutable Release-first bundle caches (110–141 MiB) lacked producer debug artifacts; producer-first caching in C can store larger Linux (~156 MiB) and Metal (~207 MiB) entries, included above.

Main plus one/two ordinary code-only PRs should reuse main's keys rather than multiply all storage. Main plus two distinct Rust/dependency sets with unchanged SDK/model pins is approximately **9.6 GB**, narrowly within the 10 GB budget with little headroom. Three entirely independent compiler/SDK/source-key sets cost approximately **12.2 GB**, so they **will not fit**. These are forecasts from compressed snapshots, not guarantees. SDK pin changes, compiler/image churn and obsolete versions can trigger eviction sooner.

**Image revisions.** c4960e1 put the runner image revision and CMake in every Cargo key, so each weekly image release would have cold-started all twelve Cargo caches (about 2.8 GB of the estimate above) and, during a rollout, kept two key sets side by side. The final head limits that to the three bundle caches (about 0.53 GB): a new image revision cold-starts only them, and a rollout can hold at most one extra entry per bundle cache until the old one is evicted. A snapshot after the c4960e1 runs showed **11,709,334,131 bytes / 53 entries** (PR #137 scope 6.42 GB, main 4.53 GB), above the 10 GB budget, so GitHub is evicting the oldest entries; deleting the PR #137 scope after merge (below) returns most of it.

Cheap implemented controls: explicit immutable keys rather than per-head rolling Cargo caches; image revision and CMake only in the bundle keys; removal of native compiler caches; deletion of obsolete trial/merged-PR caches. Proposed operational pruning after merge: delete closed PR cache scopes and superseded SDK/toolchain identities once their replacement has a successful run; retain newest current-head entries. Do not weaken toolchain keys to fit. Automatic cross-PR pruning would require broader cache-write permissions and race handling, and is not added here. The supervisor should prune these stacked PR scopes after merging to avoid keeping temporary duplicates. Historic main SDK archives (~0.62 GB) become obsolete after the installed-cache version lands; they were retained while A still used them.

## Coverage and validation

Coverage kept: tests on Linux/Windows/macOS; debug-profile tests and clippy; release-profile shipped artifacts with LTO and one codegen unit; static musl/static Windows CRT/OS-only Mac core checks; bundle.json validation; ELF/DLL/Mach-O audits; notices; native Intel and ARM archives and npm installs; npm/pnpm/Bun smokes; explicit Ubuntu 22.04 Vulkan glibc floor. Release pinned Rust and Transcription stable remain distinct contracts, with independent fallback when identity differs. All Transcription checks remain required even after early upload. No unique test, platform, package-manager, audit or preview coverage was removed. On an identity hit, Release skips its duplicate build/stage/real-bundle audit execution and relies on the successful producer audit of those exact bytes; audit self-tests and later packaging/install checks still execute. All producer tests and audits must pass before merge.

Local cargo fmt --check, cargo clippy --workspace --all-targets -- -D warnings and cargo test --workspace passed. Packaging/helper Python suite: 104 tests run, one optional skip, no failures; at the final head, 115 tests, the same optional skip (Bun not installed), no failures. actionlint 1.7.12 passed. shellcheck was unavailable; changed Bash was reviewed and parsed with bash -n; Linux/Metal audit self-tests (14/23 cases) passed. Task shape checks passed. No real AI model calls, releases or publication were used.

Remaining limitations are measurement confidence and cache headroom, not pending implementation: only single samples; mixed cold/warm identities after eviction; Linux image change; coordinator can add cold-path wall time; image rollouts and stable/pinned divergence intentionally disable sharing. Possible follow-up, not built: the producer could upload each bundle identity before building, so a consumer whose identity differs stops waiting at once. The coordinator runs on Linux and cannot compute the Windows and macOS identities, so this needs a per-consumer check or a wait on the consumers' own (more expensive macOS and Windows) runners; it is not simple enough to add here. The final notes-only commit changes the source-tree identity and therefore gets its own checks; measured code-head results above are not relabeled as that later head.

## Per-job and per-step appendix

Every API job duration and ready queue is listed below, followed by step service times. Skipped steps are retained to show reuse and coverage decisions. Initial queue and job-ready queue are different measurements. Post steps remain part of runner time. API composite timing includes setup, restore, build and audits; internal compiler/cache evidence is described above.

<details>
<summary>Baseline CI — 40980838739 attempt 1</summary>

| Job | Service seconds | Ready-to-start queue seconds | Result |
| --- | ---: | ---: | --- |
| windows-latest | 537 | 2 | success |
| macos-latest | 428 | 4 | success |
| ubuntu-latest | 294 | 2 | success |

| Job / step | Service seconds | Result |
| --- | ---: | --- |
| **windows-latest** | 537 | success |
| Set up job | 2 | success |
| Run actions/checkout@v5 | 6 | success |
| Run dtolnay/rust-toolchain@stable | 8 | success |
| Run Swatinem/rust-cache@v2 | 3 | success |
| cargo fmt --check | 0 | skipped |
| cargo clippy | 149 | success |
| cargo test | 165 | success |
| cargo build --release --bin pumice | 136 | success |
| cargo build --release --bin pumice-relay | 35 | success |
| Report release binary size | 0 | success |
| Set up MSVC environment | 6 | success |
| Verify release exe has no CRT DLL dependencies | 1 | success |
| Upload release binary | 2 | success |
| Post Run Swatinem/rust-cache@v2 | 18 | success |
| Post Run actions/checkout@v5 | 3 | success |
| Complete job | 0 | success |
| **macos-latest** | 428 | success |
| Set up job | 3 | success |
| Run actions/checkout@v5 | 3 | success |
| Run dtolnay/rust-toolchain@stable | 11 | success |
| Run Swatinem/rust-cache@v2 | 2 | success |
| cargo fmt --check | 0 | skipped |
| cargo clippy | 54 | success |
| cargo test | 141 | success |
| cargo build --release --bin pumice | 147 | success |
| cargo build --release --bin pumice-relay | 39 | success |
| Report release binary size | 0 | success |
| Set up MSVC environment | 0 | skipped |
| Verify release exe has no CRT DLL dependencies | 0 | skipped |
| Upload release binary | 3 | success |
| Post Run Swatinem/rust-cache@v2 | 16 | success |
| Post Run actions/checkout@v5 | 2 | success |
| Complete job | 4 | success |
| **ubuntu-latest** | 294 | success |
| Set up job | 2 | success |
| Run actions/checkout@v5 | 1 | success |
| Run dtolnay/rust-toolchain@stable | 1 | success |
| Run Swatinem/rust-cache@v2 | 2 | success |
| cargo fmt --check | 2 | success |
| cargo clippy | 49 | success |
| cargo test | 106 | success |
| cargo build --release --bin pumice | 95 | success |
| cargo build --release --bin pumice-relay | 26 | success |
| Report release binary size | 0 | success |
| Set up MSVC environment | 0 | skipped |
| Verify release exe has no CRT DLL dependencies | 0 | skipped |
| Upload release binary | 2 | success |
| Post Run Swatinem/rust-cache@v2 | 4 | success |
| Post Run actions/checkout@v5 | 1 | success |
| Complete job | 0 | success |

</details>

<details>
<summary>Baseline Release — 40980838699 attempt 1</summary>

| Job | Service seconds | Ready-to-start queue seconds | Result |
| --- | ---: | ---: | --- |
| Packaging helper tests | 13 | 2 | success |
| aarch64-apple-darwin | 183 | 423 | success |
| Transcription bundle (linux-x64, Vulkan) | 526 | 207 | success |
| x86_64-unknown-linux-musl | 148 | 259 | success |
| Transcription bundle (darwin-arm64, Metal) | 243 | 610 | success |
| x86_64-apple-darwin | 276 | 518 | success |
| Transcription bundle (win32-x64, Vulkan) | 678 | 224 | success |
| x86_64-pc-windows-msvc | 340 | 250 | success |
| Aggregate checksums | 9 | 3 | success |
| Stage and pack npm packages | 40 | 2 | success |
| npm install smoke (darwin-x64) | 63 | 2 | success |
| pnpm/Bun install smoke (linux-x64) | 20 | 1 | success |
| npm install smoke (linux-x64) | 20 | 2 | success |
| npm install smoke (win32-x64) | 46 | 2 | success |
| npm install smoke (darwin-arm64) | 115 | 4 | success |

| Job / step | Service seconds | Result |
| --- | ---: | --- |
| **Packaging helper tests** | 13 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 0 | success |
| Run packaging helper unit tests | 7 | success |
| Read the pinned Rust version | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 1 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **aarch64-apple-darwin** | 183 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 1 | success |
| Run dtolnay/rust-toolchain@master | 6 | success |
| Install musl tools | 0 | skipped |
| cargo build --release --locked --bin pumice | 128 | success |
| cargo build --release --locked --bin pumice-relay | 35 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 0 | skipped |
| Verify Linux binary is statically linked | 0 | skipped |
| Generate the core third-party notices | 0 | success |
| Package archive | 1 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 3 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 1 | success |
| **Transcription bundle (linux-x64, Vulkan)** | 526 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 1 | success |
| Run ./.github/actions/linux-vulkan-bundle | 509 | success |
| Check the stage starts | 0 | success |
| Pack the bundle | 4 | success |
| Upload the bundle | 3 | success |
| Post Run ./.github/actions/linux-vulkan-bundle | 3 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 0 | success |
| **x86_64-unknown-linux-musl** | 148 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 2 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 0 | success |
| Run dtolnay/rust-toolchain@master | 12 | success |
| Install musl tools | 8 | success |
| cargo build --release --locked --bin pumice | 93 | success |
| cargo build --release --locked --bin pumice-relay | 25 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 0 | skipped |
| Verify Linux binary is statically linked | 0 | success |
| Generate the core third-party notices | 1 | success |
| Package archive | 1 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 2 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **Transcription bundle (darwin-arm64, Metal)** | 243 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 2 | success |
| Run ./.github/actions/macos-metal-bundle | 226 | success |
| Check the stage starts | 0 | success |
| Pack the bundle | 1 | success |
| Upload the bundle | 2 | success |
| Post Run ./.github/actions/macos-metal-bundle | 7 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 1 | success |
| **x86_64-apple-darwin** | 276 | success |
| Set up job | 3 | success |
| Run actions/checkout@v7 | 3 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 1 | success |
| Run dtolnay/rust-toolchain@master | 17 | success |
| Install musl tools | 0 | skipped |
| cargo build --release --locked --bin pumice | 200 | success |
| cargo build --release --locked --bin pumice-relay | 43 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 0 | skipped |
| Verify Linux binary is statically linked | 0 | skipped |
| Generate the core third-party notices | 0 | success |
| Package archive | 1 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 3 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 1 | success |
| **Transcription bundle (win32-x64, Vulkan)** | 678 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 7 | success |
| Run ./.github/actions/windows-vulkan-bundle | 648 | success |
| Check the stage starts | 0 | success |
| Upload the bundle | 5 | success |
| Post Run ./.github/actions/windows-vulkan-bundle | 11 | success |
| Post Run actions/checkout@v7 | 2 | success |
| Complete job | 1 | success |
| **x86_64-pc-windows-msvc** | 340 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 6 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 1 | success |
| Run dtolnay/rust-toolchain@master | 13 | success |
| Install musl tools | 0 | skipped |
| cargo build --release --locked --bin pumice | 191 | success |
| cargo build --release --locked --bin pumice-relay | 45 | success |
| Smoke test binary --version | 1 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 72 | success |
| Verify Linux binary is statically linked | 0 | skipped |
| Generate the core third-party notices | 2 | success |
| Package archive | 0 | success |
| Smoke test packaged archive | 1 | success |
| Upload release archive | 2 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 3 | success |
| Complete job | 0 | success |
| **Aggregate checksums** | 9 | success |
| Set up job | 1 | success |
| Run actions/download-artifact@v8 | 2 | success |
| Generate SHA256SUMS | 0 | success |
| Upload release preview | 3 | success |
| Complete job | 0 | success |
| **Stage and pack npm packages** | 40 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 4 | success |
| Run dtolnay/rust-toolchain@master | 8 | success |
| Download release preview archives | 1 | success |
| Download the transcription bundles | 2 | success |
| Assemble the bundle stages | 0 | success |
| Generate the third-party notices | 5 | success |
| Stage and pack npm tarballs | 11 | success |
| Report package sizes | 0 | success |
| Upload npm preview | 3 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 0 | success |
| **npm install smoke (darwin-x64)** | 63 | success |
| Set up job | 4 | success |
| Run actions/checkout@v7 | 3 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 2 | success |
| Extract crate version | 1 | success |
| Download npm preview | 3 | success |
| Start loopback test registry | 39 | success |
| Install pumice through the test registry | 2 | success |
| Run installed pumice --version | 1 | success |
| Executable bits after install | 0 | success |
| pumice transcribe (version and backends without a model) | 0 | skipped |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | success |
| Cache Whisper tiny Q8_0 model | 0 | skipped |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 0 | skipped |
| npx through the test registry | 2 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | success |
| Stop test registry | 0 | success |
| Post Run actions/setup-node@v7 | 1 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 1 | success |
| **pnpm/Bun install smoke (linux-x64)** | 20 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 1 | success |
| Run actions/setup-node@v7 | 0 | success |
| Run pnpm/action-setup@v6 | 6 | success |
| Run oven-sh/setup-bun@v2 | 1 | success |
| Extract crate version | 0 | success |
| Download npm preview | 1 | success |
| Start loopback test registry | 4 | success |
| Install and run with pnpm | 1 | success |
| Install and run with Bun | 0 | success |
| Stop test registry | 0 | success |
| Post Run oven-sh/setup-bun@v2 | 0 | success |
| Post Run pnpm/action-setup@v6 | 1 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **npm install smoke (linux-x64)** | 20 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 1 | success |
| Extract crate version | 2 | success |
| Download npm preview | 2 | success |
| Start loopback test registry | 5 | success |
| Install pumice through the test registry | 1 | success |
| Run installed pumice --version | 0 | success |
| Executable bits after install | 0 | success |
| pumice transcribe (version and backends without a model) | 0 | success |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | skipped |
| Cache Whisper tiny Q8_0 model | 1 | success |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 1 | success |
| npx through the test registry | 1 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | skipped |
| Stop test registry | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 1 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **npm install smoke (win32-x64)** | 46 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 7 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 7 | success |
| Extract crate version | 2 | success |
| Download npm preview | 1 | success |
| Start loopback test registry | 5 | success |
| Install pumice through the test registry | 5 | success |
| Run installed pumice --version | 0 | success |
| Executable bits after install | 0 | skipped |
| pumice transcribe (version and backends without a model) | 1 | success |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | skipped |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 2 | success |
| npx through the test registry | 8 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | skipped |
| Stop test registry | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 3 | success |
| Complete job | 0 | success |
| **npm install smoke (darwin-arm64)** | 115 | success |
| Set up job | 3 | success |
| Run actions/checkout@v7 | 3 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 1 | success |
| Extract crate version | 1 | success |
| Download npm preview | 7 | success |
| Start loopback test registry | 38 | success |
| Install pumice through the test registry | 1 | success |
| Run installed pumice --version | 1 | success |
| Executable bits after install | 0 | success |
| pumice transcribe (version and backends without a model) | 42 | success |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | skipped |
| Cache Whisper tiny Q8_0 model | 8 | success |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 2 | success |
| npx through the test registry | 1 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | success |
| Stop test registry | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 1 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 1 | success |

</details>

<details>
<summary>Baseline Transcription — 40980838776 attempt 1</summary>

| Job | Service seconds | Ready-to-start queue seconds | Result |
| --- | ---: | ---: | --- |
| Transcription runtime CPU variants (Linux) | 1152 | 3 | success |
| Transcription Vulkan preview bundle (Linux x64) | 775 | 3 | success |
| Transcription Metal preview bundle (macOS arm64) | 1105 | 6 | success |
| Transcription Vulkan preview bundle (Windows x64) | 852 | 3 | success |
| Transcription CPU engine smoke (Linux) | 234 | 2 | success |

| Job / step | Service seconds | Result |
| --- | ---: | --- |
| **Transcription runtime CPU variants (Linux)** | 1152 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 1 | success |
| Run dtolnay/rust-toolchain@stable | 1 | success |
| Run Swatinem/rust-cache@v2 | 2 | success |
| cargo clippy -p pumice-transcription --features cpu-variants | 364 | success |
| cargo test -p pumice-transcription --features cpu-variants | 356 | success |
| Build and stage cpu-variants | 416 | success |
| Assert staged layout | 0 | success |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 0 | skipped |
| Verify cached model | 0 | success |
| Smoke test staged binary with tiny model (read-only install) | 2 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run Swatinem/rust-cache@v2 | 4 | success |
| Post Run actions/checkout@v5 | 0 | success |
| Complete job | 0 | success |
| **Transcription Vulkan preview bundle (Linux x64)** | 775 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 1 | success |
| Run ./.github/actions/linux-vulkan-bundle | 530 | success |
| cargo clippy -p pumice-transcription --features vulkan | 18 | success |
| cargo test -p pumice-transcription --features vulkan | 98 | success |
| Record bundle sizes | 0 | success |
| Install the runtime test packages (Vulkan loader, Mesa) | 22 | success |
| Write the test helpers | 0 | success |
| Backend registration without a Vulkan device (backend cpu, auto and gpu) | 0 | success |
| Bundle manifest checks (damaged, unlisted, missing) | 2 | success |
| Cache Whisper tiny Q8_0 model | 0 | success |
| Download and verify model | 2 | success |
| Verify cached model | 0 | skipped |
| Smoke test the staged bundle with tiny model (no Vulkan device; backend cpu, auto, gpu) | 2 | success |
| Ubuntu 22.04 without a Vulkan loader (container) | 5 | success |
| Software Vulkan device (Mesa lavapipe; auto stays on the CPU, gpu uses it) | 79 | success |
| Pack the preview bundle | 5 | success |
| Upload the preview bundle | 2 | success |
| Post Cache Whisper tiny Q8_0 model | 1 | success |
| Post Run ./.github/actions/linux-vulkan-bundle | 4 | success |
| Post Run actions/checkout@v5 | 0 | success |
| Complete job | 0 | success |
| **Transcription Metal preview bundle (macOS arm64)** | 1105 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 1 | success |
| Run ./.github/actions/macos-metal-bundle | 214 | success |
| cargo clippy -p pumice-transcription --features metal | 14 | success |
| cargo test -p pumice-transcription --features metal | 107 | success |
| Record bundle sizes | 0 | success |
| Write the test helpers | 0 | success |
| Backend registration without a model (backend cpu, auto and gpu) | 74 | success |
| Bundle manifest checks (damaged, unlisted, missing, bad manifest) | 34 | success |
| Cache Whisper tiny Q8_0 model | 1 | success |
| Download and verify model | 2 | success |
| Verify cached model | 0 | skipped |
| Smoke test a copied bundle with tiny model (path with spaces; backend cpu, auto, gpu) | 189 | success |
| Build pumice (orchestrator) | 140 | success |
| backend auto never loads Metal (fresh folder, corrupted module; file, managed serve, pumice serve) | 5 | success |
| Metal diagnostics: backend gpu without flash attention (text recorded only) | 123 | success |
| Metal diagnostics: ggml test-backend-ops on MTL0 (recorded only) | 140 | success |
| Quarantined copy (Gatekeeper) and the documented fix | 34 | success |
| Pack the preview bundle | 0 | success |
| Upload the preview bundle | 2 | success |
| Post Cache Whisper tiny Q8_0 model | 1 | success |
| Post Run ./.github/actions/macos-metal-bundle | 12 | success |
| Post Run actions/checkout@v5 | 2 | success |
| Complete job | 6 | success |
| **Transcription Vulkan preview bundle (Windows x64)** | 852 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 6 | success |
| Run ./.github/actions/windows-vulkan-bundle | 637 | success |
| cargo clippy -p pumice-transcription --features vulkan | 25 | success |
| cargo test -p pumice-transcription --features vulkan | 143 | success |
| Record bundle sizes | 0 | success |
| Backend registration without a model (backend cpu, auto and gpu) | 1 | success |
| Bundle manifest checks (damaged, unlisted, missing) | 1 | success |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 0 | success |
| Smoke test the staged bundle with tiny model (backend cpu, auto, gpu) | 4 | success |
| DLL search order (decoy DLLs in the current directory and on PATH) | 13 | success |
| Upload the preview bundle | 4 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run ./.github/actions/windows-vulkan-bundle | 11 | success |
| Post Run actions/checkout@v5 | 1 | success |
| Complete job | 0 | success |
| **Transcription CPU engine smoke (Linux)** | 234 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 1 | success |
| Run dtolnay/rust-toolchain@stable | 3 | success |
| Run Swatinem/rust-cache@v2 | 1 | success |
| cargo clippy -p pumice-transcription --features engine | 76 | success |
| cargo test -p pumice-transcription --features opus | 29 | success |
| cargo build --release -p pumice-transcription --features engine | 114 | success |
| Cache Whisper tiny Q8_0 model | 1 | success |
| Download and verify model | 0 | skipped |
| Verify cached model | 0 | success |
| Smoke test transcription with tiny model | 1 | success |
| Backend policy without a GPU build (static engine) | 1 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run Swatinem/rust-cache@v2 | 3 | success |
| Post Run actions/checkout@v5 | 0 | success |
| Complete job | 0 | success |

</details>

<details>
<summary>A cold CI (failed) — 40987501204 attempt 1</summary>

| Job | Service seconds | Ready-to-start queue seconds | Result |
| --- | ---: | ---: | --- |
| macos-latest | 199 | 4 | failure |
| ubuntu-latest | 284 | 2 | success |
| windows-latest | 551 | 2 | success |

| Job / step | Service seconds | Result |
| --- | ---: | --- |
| **macos-latest** | 199 | failure |
| Set up job | 2 | success |
| Run actions/checkout@v5 | 2 | success |
| Run dtolnay/rust-toolchain@stable | 11 | success |
| Run ./.github/actions/cargo-cache | 5 | success |
| cargo fmt --check | 0 | skipped |
| cargo clippy | 52 | success |
| cargo test | 123 | failure |
| cargo build --release --bin pumice | 0 | skipped |
| cargo build --release --bin pumice-relay | 0 | skipped |
| Report release binary size | 0 | skipped |
| Set up MSVC environment | 0 | skipped |
| Verify release exe has no CRT DLL dependencies | 0 | skipped |
| Upload release binary | 0 | skipped |
| Run ./.github/actions/cargo-cache-save | 0 | skipped |
| Post Run actions/checkout@v5 | 1 | success |
| Complete job | 1 | success |
| **ubuntu-latest** | 284 | success |
| Set up job | 2 | success |
| Run actions/checkout@v5 | 1 | success |
| Run dtolnay/rust-toolchain@stable | 1 | success |
| Run ./.github/actions/cargo-cache | 1 | success |
| cargo fmt --check | 1 | success |
| cargo clippy | 37 | success |
| cargo test | 103 | success |
| cargo build --release --bin pumice | 93 | success |
| cargo build --release --bin pumice-relay | 25 | success |
| Report release binary size | 1 | success |
| Set up MSVC environment | 0 | skipped |
| Verify release exe has no CRT DLL dependencies | 0 | skipped |
| Upload release binary | 1 | success |
| Run ./.github/actions/cargo-cache-save | 14 | success |
| Post Run actions/checkout@v5 | 1 | success |
| Complete job | 0 | success |
| **windows-latest** | 551 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 5 | success |
| Run dtolnay/rust-toolchain@stable | 8 | success |
| Run ./.github/actions/cargo-cache | 4 | success |
| cargo fmt --check | 0 | skipped |
| cargo clippy | 94 | success |
| cargo test | 193 | success |
| cargo build --release --bin pumice | 172 | success |
| cargo build --release --bin pumice-relay | 53 | success |
| Report release binary size | 0 | success |
| Set up MSVC environment | 4 | success |
| Verify release exe has no CRT DLL dependencies | 1 | success |
| Upload release binary | 1 | success |
| Run ./.github/actions/cargo-cache-save | 10 | success |
| Post Run actions/checkout@v5 | 2 | success |
| Complete job | 0 | success |

</details>

<details>
<summary>A cold Release — 40987501170 attempt 1</summary>

| Job | Service seconds | Ready-to-start queue seconds | Result |
| --- | ---: | ---: | --- |
| Packaging helper tests | 15 | 2 | success |
| Transcription bundle (linux-x64, Vulkan) | 506 | 2 | success |
| Transcription bundle (win32-x64, Vulkan) | 671 | 2 | success |
| Transcription bundle (darwin-arm64, Metal) | 264 | 2 | success |
| aarch64-apple-darwin | 152 | 6 | success |
| x86_64-apple-darwin | 476 | 4 | success |
| x86_64-pc-windows-msvc | 239 | 2 | success |
| x86_64-unknown-linux-musl | 150 | 2 | success |
| Aggregate checksums | 6 | 1 | success |
| Stage and pack npm packages | 46 | 2 | success |
| npm install smoke (darwin-arm64) | 92 | 8 | success |
| npm install smoke (darwin-x64) | 68 | 3 | success |
| pnpm/Bun install smoke (linux-x64) | 17 | 2 | success |
| npm install smoke (linux-x64) | 19 | 2 | success |
| npm install smoke (win32-x64) | 59 | 3 | success |

| Job / step | Service seconds | Result |
| --- | ---: | --- |
| **Packaging helper tests** | 15 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 2 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 0 | success |
| Run packaging helper unit tests | 7 | success |
| Read the pinned Rust version | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 0 | success |
| **Transcription bundle (linux-x64, Vulkan)** | 506 | success |
| Set up job | 0 | success |
| Run actions/checkout@v7 | 2 | success |
| Run ./.github/actions/linux-vulkan-bundle | 489 | success |
| Check the stage starts | 1 | success |
| Pack the bundle | 4 | success |
| Upload the bundle | 2 | success |
| Run ./.github/actions/cargo-cache-save | 4 | success |
| Post Run ./.github/actions/linux-vulkan-bundle | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **Transcription bundle (win32-x64, Vulkan)** | 671 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 5 | success |
| Run ./.github/actions/windows-vulkan-bundle | 653 | success |
| Check the stage starts | 0 | success |
| Upload the bundle | 3 | success |
| Run ./.github/actions/cargo-cache-save | 5 | success |
| Post Run ./.github/actions/windows-vulkan-bundle | 0 | success |
| Post Run actions/checkout@v7 | 3 | success |
| Complete job | 0 | success |
| **Transcription bundle (darwin-arm64, Metal)** | 264 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 2 | success |
| Run ./.github/actions/macos-metal-bundle | 250 | success |
| Check the stage starts | 0 | success |
| Pack the bundle | 1 | success |
| Upload the bundle | 1 | success |
| Run ./.github/actions/cargo-cache-save | 5 | success |
| Post Run ./.github/actions/macos-metal-bundle | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 2 | success |
| **aarch64-apple-darwin** | 152 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 2 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 0 | success |
| Run dtolnay/rust-toolchain@master | 6 | success |
| Install musl tools | 0 | skipped |
| Run ./.github/actions/cargo-cache | 3 | success |
| cargo build --release --locked --bin pumice | 98 | success |
| cargo build --release --locked --bin pumice-relay | 30 | success |
| Smoke test binary --version | 1 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 0 | skipped |
| Verify Linux binary is statically linked | 0 | skipped |
| Generate the core third-party notices | 0 | success |
| Package archive | 1 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 2 | success |
| Run ./.github/actions/cargo-cache-save | 4 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 0 | success |
| **x86_64-apple-darwin** | 476 | success |
| Set up job | 6 | success |
| Run actions/checkout@v7 | 6 | success |
| Run actions/setup-python@v7 | 1 | success |
| Extract crate version | 1 | success |
| Run dtolnay/rust-toolchain@master | 35 | success |
| Install musl tools | 0 | skipped |
| Run ./.github/actions/cargo-cache | 15 | success |
| cargo build --release --locked --bin pumice | 331 | success |
| cargo build --release --locked --bin pumice-relay | 66 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 0 | skipped |
| Verify Linux binary is statically linked | 0 | skipped |
| Generate the core third-party notices | 2 | success |
| Package archive | 1 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 2 | success |
| Run ./.github/actions/cargo-cache-save | 5 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 1 | success |
| **x86_64-pc-windows-msvc** | 239 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 6 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 2 | success |
| Run dtolnay/rust-toolchain@master | 13 | success |
| Install musl tools | 0 | skipped |
| Run ./.github/actions/cargo-cache | 2 | success |
| cargo build --release --locked --bin pumice | 162 | success |
| cargo build --release --locked --bin pumice-relay | 35 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 1 | success |
| Verify exe has no CRT DLL dependencies | 0 | success |
| Verify Linux binary is statically linked | 0 | skipped |
| Generate the core third-party notices | 1 | success |
| Package archive | 1 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 2 | success |
| Run ./.github/actions/cargo-cache-save | 5 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 2 | success |
| Complete job | 0 | success |
| **x86_64-unknown-linux-musl** | 150 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 2 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 0 | success |
| Run dtolnay/rust-toolchain@master | 12 | success |
| Install musl tools | 7 | success |
| Run ./.github/actions/cargo-cache | 0 | success |
| cargo build --release --locked --bin pumice | 95 | success |
| cargo build --release --locked --bin pumice-relay | 25 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 0 | skipped |
| Verify Linux binary is statically linked | 0 | success |
| Generate the core third-party notices | 0 | success |
| Package archive | 1 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 1 | success |
| Run ./.github/actions/cargo-cache-save | 4 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **Aggregate checksums** | 6 | success |
| Set up job | 1 | success |
| Run actions/download-artifact@v8 | 1 | success |
| Generate SHA256SUMS | 0 | success |
| Upload release preview | 1 | success |
| Complete job | 0 | success |
| **Stage and pack npm packages** | 46 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 1 | success |
| Run dtolnay/rust-toolchain@master | 9 | success |
| Download release preview archives | 2 | success |
| Download the transcription bundles | 5 | success |
| Assemble the bundle stages | 0 | success |
| Generate the third-party notices | 4 | success |
| Stage and pack npm tarballs | 14 | success |
| Report package sizes | 0 | success |
| Upload npm preview | 4 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **npm install smoke (darwin-arm64)** | 92 | success |
| Set up job | 3 | success |
| Run actions/checkout@v7 | 2 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 1 | success |
| Extract crate version | 0 | success |
| Download npm preview | 4 | success |
| Start loopback test registry | 36 | success |
| Install pumice through the test registry | 1 | success |
| Run installed pumice --version | 0 | success |
| Executable bits after install | 0 | success |
| pumice transcribe (version and backends without a model) | 37 | success |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | skipped |
| Cache Whisper tiny Q8_0 model | 1 | success |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 2 | success |
| npx through the test registry | 1 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | success |
| Stop test registry | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 0 | success |
| **npm install smoke (darwin-x64)** | 68 | success |
| Set up job | 3 | success |
| Run actions/checkout@v7 | 3 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 2 | success |
| Extract crate version | 1 | success |
| Download npm preview | 7 | success |
| Start loopback test registry | 36 | success |
| Install pumice through the test registry | 3 | success |
| Run installed pumice --version | 0 | success |
| Executable bits after install | 1 | success |
| pumice transcribe (version and backends without a model) | 0 | skipped |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | success |
| Cache Whisper tiny Q8_0 model | 0 | skipped |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 0 | skipped |
| npx through the test registry | 3 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | success |
| Stop test registry | 1 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 2 | success |
| Complete job | 3 | success |
| **pnpm/Bun install smoke (linux-x64)** | 17 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 1 | success |
| Run pnpm/action-setup@v6 | 5 | success |
| Run oven-sh/setup-bun@v2 | 1 | success |
| Extract crate version | 1 | success |
| Download npm preview | 1 | success |
| Start loopback test registry | 1 | success |
| Install and run with pnpm | 1 | success |
| Install and run with Bun | 0 | success |
| Stop test registry | 0 | success |
| Post Run oven-sh/setup-bun@v2 | 0 | success |
| Post Run pnpm/action-setup@v6 | 0 | success |
| Post Run actions/setup-node@v7 | 1 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **npm install smoke (linux-x64)** | 19 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 2 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 1 | success |
| Extract crate version | 0 | success |
| Download npm preview | 3 | success |
| Start loopback test registry | 1 | success |
| Install pumice through the test registry | 2 | success |
| Run installed pumice --version | 0 | success |
| Executable bits after install | 0 | success |
| pumice transcribe (version and backends without a model) | 0 | success |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | skipped |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 0 | success |
| npx through the test registry | 2 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | skipped |
| Stop test registry | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **npm install smoke (win32-x64)** | 59 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 8 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 9 | success |
| Extract crate version | 5 | success |
| Download npm preview | 3 | success |
| Start loopback test registry | 2 | success |
| Install pumice through the test registry | 8 | success |
| Run installed pumice --version | 1 | success |
| Executable bits after install | 0 | skipped |
| pumice transcribe (version and backends without a model) | 1 | success |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | skipped |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 1 | success |
| npx through the test registry | 11 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | skipped |
| Stop test registry | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 1 | success |
| Post Run actions/checkout@v7 | 2 | success |
| Complete job | 0 | success |

</details>

<details>
<summary>A cold Transcription — 40987501189 attempt 1</summary>

| Job | Service seconds | Ready-to-start queue seconds | Result |
| --- | ---: | ---: | --- |
| Transcription Vulkan preview bundle (Windows x64) | 875 | 2 | success |
| Transcription runtime CPU variants (Linux) | 690 | 2 | success |
| Transcription Vulkan preview bundle (Linux x64) | 753 | 3 | success |
| Transcription CPU engine smoke (Linux) | 285 | 2 | success |
| Transcription Metal preview bundle (macOS arm64) | 871 | 8 | success |

| Job / step | Service seconds | Result |
| --- | ---: | --- |
| **Transcription Vulkan preview bundle (Windows x64)** | 875 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 5 | success |
| Run ./.github/actions/windows-vulkan-bundle | 656 | success |
| cargo clippy -p pumice-transcription --features vulkan | 27 | success |
| cargo test -p pumice-transcription --features vulkan | 153 | success |
| Record bundle sizes | 0 | success |
| Backend registration without a model (backend cpu, auto and gpu) | 1 | success |
| Bundle manifest checks (damaged, unlisted, missing) | 2 | success |
| Cache Whisper tiny Q8_0 model | 0 | success |
| Download and verify model | 1 | success |
| Smoke test the staged bundle with tiny model (backend cpu, auto, gpu) | 4 | success |
| DLL search order (decoy DLLs in the current directory and on PATH) | 14 | success |
| Upload the preview bundle | 3 | success |
| Run ./.github/actions/cargo-cache-save | 4 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run ./.github/actions/windows-vulkan-bundle | 0 | success |
| Post Run actions/checkout@v5 | 2 | success |
| Complete job | 0 | success |
| **Transcription runtime CPU variants (Linux)** | 690 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 1 | success |
| Run dtolnay/rust-toolchain@stable | 1 | success |
| Run ./.github/actions/cargo-cache | 1 | success |
| cargo clippy -p pumice-transcription --features cpu-variants | 209 | success |
| cargo test -p pumice-transcription --features cpu-variants | 214 | success |
| Build and stage cpu-variants | 251 | success |
| Assert staged layout | 0 | success |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 0 | skipped |
| Verify cached model | 0 | success |
| Smoke test staged binary with tiny model (read-only install) | 1 | success |
| Run ./.github/actions/cargo-cache-save | 6 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run actions/checkout@v5 | 0 | success |
| Complete job | 0 | success |
| **Transcription Vulkan preview bundle (Linux x64)** | 753 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 1 | success |
| Run ./.github/actions/linux-vulkan-bundle | 520 | success |
| cargo clippy -p pumice-transcription --features vulkan | 18 | success |
| cargo test -p pumice-transcription --features vulkan | 91 | success |
| Record bundle sizes | 0 | success |
| Install the runtime test packages (Vulkan loader, Mesa) | 15 | success |
| Write the test helpers | 0 | success |
| Backend registration without a Vulkan device (backend cpu, auto and gpu) | 1 | success |
| Bundle manifest checks (damaged, unlisted, missing) | 1 | success |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 0 | skipped |
| Verify cached model | 1 | success |
| Smoke test the staged bundle with tiny model (no Vulkan device; backend cpu, auto, gpu) | 2 | success |
| Ubuntu 22.04 without a Vulkan loader (container) | 5 | success |
| Software Vulkan device (Mesa lavapipe; auto stays on the CPU, gpu uses it) | 80 | success |
| Pack the preview bundle | 5 | success |
| Upload the preview bundle | 2 | success |
| Run ./.github/actions/cargo-cache-save | 3 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run ./.github/actions/linux-vulkan-bundle | 0 | success |
| Post Run actions/checkout@v5 | 1 | success |
| Complete job | 0 | success |
| **Transcription CPU engine smoke (Linux)** | 285 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 2 | success |
| Run dtolnay/rust-toolchain@stable | 0 | success |
| Run ./.github/actions/cargo-cache | 1 | success |
| cargo clippy -p pumice-transcription --features engine | 88 | success |
| cargo test -p pumice-transcription --features opus | 37 | success |
| cargo build --release -p pumice-transcription --features engine | 138 | success |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 0 | skipped |
| Verify cached model | 0 | success |
| Smoke test transcription with tiny model | 1 | success |
| Backend policy without a GPU build (static engine) | 1 | success |
| Run ./.github/actions/cargo-cache-save | 10 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run actions/checkout@v5 | 1 | success |
| Complete job | 0 | success |
| **Transcription Metal preview bundle (macOS arm64)** | 871 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 2 | success |
| Run ./.github/actions/macos-metal-bundle | 218 | success |
| cargo clippy -p pumice-transcription --features metal | 17 | success |
| cargo test -p pumice-transcription --features metal | 103 | success |
| Record bundle sizes | 0 | success |
| Write the test helpers | 0 | success |
| Backend registration without a model (backend cpu, auto and gpu) | 76 | success |
| Bundle manifest checks (damaged, unlisted, missing, bad manifest) | 36 | success |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 0 | skipped |
| Verify cached model | 1 | success |
| Smoke test a copied bundle with tiny model (path with spaces; backend cpu, auto, gpu) | 80 | success |
| Build pumice (orchestrator) | 108 | success |
| backend auto never loads Metal (fresh folder, corrupted module; file, managed serve, pumice serve) | 4 | success |
| Metal diagnostics: backend gpu without flash attention (text recorded only) | 68 | success |
| Metal diagnostics: ggml test-backend-ops on MTL0 (recorded only) | 114 | success |
| Quarantined copy (Gatekeeper) and the documented fix | 27 | success |
| Pack the preview bundle | 1 | success |
| Upload the preview bundle | 2 | success |
| Run ./.github/actions/cargo-cache-save | 4 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run ./.github/actions/macos-metal-bundle | 0 | success |
| Post Run actions/checkout@v5 | 1 | success |
| Complete job | 2 | success |

</details>

<details>
<summary>A warm CI — 40987501204 attempt 3</summary>

| Job | Service seconds | Ready-to-start queue seconds | Result |
| --- | ---: | ---: | --- |
| ubuntu-latest | 169 | 151 | success |
| macos-latest | 185 | 629 | success |
| windows-latest | 271 | 55 | success |

| Job / step | Service seconds | Result |
| --- | ---: | --- |
| **ubuntu-latest** | 169 | success |
| Set up job | 2 | success |
| Run actions/checkout@v5 | 1 | success |
| Run dtolnay/rust-toolchain@stable | 2 | success |
| Run ./.github/actions/cargo-cache | 6 | success |
| cargo fmt --check | 2 | success |
| cargo clippy | 12 | success |
| cargo test | 67 | success |
| cargo build --release --bin pumice | 52 | success |
| cargo build --release --bin pumice-relay | 21 | success |
| Report release binary size | 0 | success |
| Set up MSVC environment | 0 | skipped |
| Verify release exe has no CRT DLL dependencies | 0 | skipped |
| Upload release binary | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run actions/checkout@v5 | 0 | success |
| Complete job | 0 | success |
| **macos-latest** | 185 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 2 | success |
| Run dtolnay/rust-toolchain@stable | 7 | success |
| Run ./.github/actions/cargo-cache | 3 | success |
| cargo fmt --check | 0 | skipped |
| cargo clippy | 20 | success |
| cargo test | 68 | success |
| cargo build --release --bin pumice | 56 | success |
| cargo build --release --bin pumice-relay | 17 | success |
| Report release binary size | 0 | success |
| Set up MSVC environment | 0 | skipped |
| Verify release exe has no CRT DLL dependencies | 0 | skipped |
| Upload release binary | 2 | success |
| Run ./.github/actions/cargo-cache-save | 4 | success |
| Post Run actions/checkout@v5 | 0 | success |
| Complete job | 0 | success |
| **windows-latest** | 271 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 5 | success |
| Run dtolnay/rust-toolchain@stable | 6 | success |
| Run ./.github/actions/cargo-cache | 11 | success |
| cargo fmt --check | 0 | skipped |
| cargo clippy | 37 | success |
| cargo test | 105 | success |
| cargo build --release --bin pumice | 68 | success |
| cargo build --release --bin pumice-relay | 28 | success |
| Report release binary size | 0 | success |
| Set up MSVC environment | 3 | success |
| Verify release exe has no CRT DLL dependencies | 0 | success |
| Upload release binary | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run actions/checkout@v5 | 2 | success |
| Complete job | 0 | success |

</details>

<details>
<summary>A warm Release — 40987501170 attempt 2</summary>

| Job | Service seconds | Ready-to-start queue seconds | Result |
| --- | ---: | ---: | --- |
| Packaging helper tests | 26 | 142 | success |
| aarch64-apple-darwin | 162 | 499 | success |
| Transcription bundle (linux-x64, Vulkan) | 111 | 204 | success |
| Transcription bundle (darwin-arm64, Metal) | 121 | 704 | success |
| x86_64-unknown-linux-musl | 126 | 224 | success |
| Transcription bundle (win32-x64, Vulkan) | 177 | 172 | success |
| x86_64-pc-windows-msvc | 170 | 12 | success |
| x86_64-apple-darwin | 275 | 184 | success |
| Aggregate checksums | 7 | 33 | success |
| Stage and pack npm packages | 42 | 7 | success |
| pnpm/Bun install smoke (linux-x64) | 17 | 6 | success |
| npm install smoke (win32-x64) | 52 | 2 | success |
| npm install smoke (darwin-x64) | 55 | 50 | success |
| npm install smoke (darwin-arm64) | 99 | 58 | success |
| npm install smoke (linux-x64) | 17 | 8 | success |

| Job / step | Service seconds | Result |
| --- | ---: | --- |
| **Packaging helper tests** | 26 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 9 | success |
| Run packaging helper unit tests | 11 | success |
| Read the pinned Rust version | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 0 | success |
| **aarch64-apple-darwin** | 162 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 3 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 1 | success |
| Run dtolnay/rust-toolchain@master | 7 | success |
| Install musl tools | 0 | skipped |
| Run ./.github/actions/cargo-cache | 5 | success |
| cargo build --release --locked --bin pumice | 95 | success |
| cargo build --release --locked --bin pumice-relay | 40 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 0 | skipped |
| Verify Linux binary is statically linked | 0 | skipped |
| Generate the core third-party notices | 1 | success |
| Package archive | 1 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 1 | success |
| Run ./.github/actions/cargo-cache-save | 1 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 2 | success |
| **Transcription bundle (linux-x64, Vulkan)** | 111 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 3 | success |
| Run ./.github/actions/linux-vulkan-bundle | 98 | success |
| Check the stage starts | 0 | success |
| Pack the bundle | 4 | success |
| Upload the bundle | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run ./.github/actions/linux-vulkan-bundle | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **Transcription bundle (darwin-arm64, Metal)** | 121 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 2 | success |
| Run ./.github/actions/macos-metal-bundle | 111 | success |
| Check the stage starts | 0 | success |
| Pack the bundle | 1 | success |
| Upload the bundle | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run ./.github/actions/macos-metal-bundle | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 1 | success |
| **x86_64-unknown-linux-musl** | 126 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 2 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 1 | success |
| Run dtolnay/rust-toolchain@master | 13 | success |
| Install musl tools | 6 | success |
| Run ./.github/actions/cargo-cache | 3 | success |
| cargo build --release --locked --bin pumice | 66 | success |
| cargo build --release --locked --bin pumice-relay | 27 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 0 | skipped |
| Verify Linux binary is statically linked | 0 | success |
| Generate the core third-party notices | 0 | success |
| Package archive | 1 | success |
| Smoke test packaged archive | 1 | success |
| Upload release archive | 1 | success |
| Run ./.github/actions/cargo-cache-save | 1 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **Transcription bundle (win32-x64, Vulkan)** | 177 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 5 | success |
| Run ./.github/actions/windows-vulkan-bundle | 160 | success |
| Check the stage starts | 0 | success |
| Upload the bundle | 4 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run ./.github/actions/windows-vulkan-bundle | 1 | success |
| Post Run actions/checkout@v7 | 2 | success |
| Complete job | 0 | success |
| **x86_64-pc-windows-msvc** | 170 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 5 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 1 | success |
| Run dtolnay/rust-toolchain@master | 12 | success |
| Install musl tools | 0 | skipped |
| Run ./.github/actions/cargo-cache | 5 | success |
| cargo build --release --locked --bin pumice | 104 | success |
| cargo build --release --locked --bin pumice-relay | 33 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 1 | success |
| Verify Linux binary is statically linked | 0 | skipped |
| Generate the core third-party notices | 1 | success |
| Package archive | 0 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 2 | success |
| Complete job | 0 | success |
| **x86_64-apple-darwin** | 275 | success |
| Set up job | 4 | success |
| Run actions/checkout@v7 | 3 | success |
| Run actions/setup-python@v7 | 1 | success |
| Extract crate version | 1 | success |
| Run dtolnay/rust-toolchain@master | 20 | success |
| Install musl tools | 0 | skipped |
| Run ./.github/actions/cargo-cache | 27 | success |
| cargo build --release --locked --bin pumice | 149 | success |
| cargo build --release --locked --bin pumice-relay | 60 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 0 | skipped |
| Verify Linux binary is statically linked | 0 | skipped |
| Generate the core third-party notices | 1 | success |
| Package archive | 1 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 3 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 1 | success |
| **Aggregate checksums** | 7 | success |
| Set up job | 1 | success |
| Run actions/download-artifact@v8 | 2 | success |
| Generate SHA256SUMS | 0 | success |
| Upload release preview | 2 | success |
| Complete job | 0 | success |
| **Stage and pack npm packages** | 42 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 3 | success |
| Run dtolnay/rust-toolchain@master | 8 | success |
| Download release preview archives | 1 | success |
| Download the transcription bundles | 2 | success |
| Assemble the bundle stages | 0 | success |
| Generate the third-party notices | 5 | success |
| Stage and pack npm tarballs | 13 | success |
| Report package sizes | 0 | success |
| Upload npm preview | 3 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 0 | success |
| **pnpm/Bun install smoke (linux-x64)** | 17 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 1 | success |
| Run pnpm/action-setup@v6 | 5 | success |
| Run oven-sh/setup-bun@v2 | 1 | success |
| Extract crate version | 1 | success |
| Download npm preview | 1 | success |
| Start loopback test registry | 1 | success |
| Install and run with pnpm | 1 | success |
| Install and run with Bun | 0 | success |
| Stop test registry | 0 | success |
| Post Run oven-sh/setup-bun@v2 | 2 | success |
| Post Run pnpm/action-setup@v6 | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **npm install smoke (win32-x64)** | 52 | success |
| Set up job | 3 | success |
| Run actions/checkout@v7 | 7 | success |
| Run actions/setup-python@v7 | 1 | success |
| Run actions/setup-node@v7 | 7 | success |
| Extract crate version | 3 | success |
| Download npm preview | 3 | success |
| Start loopback test registry | 1 | success |
| Install pumice through the test registry | 8 | success |
| Run installed pumice --version | 0 | success |
| Executable bits after install | 0 | skipped |
| pumice transcribe (version and backends without a model) | 1 | success |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | skipped |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 1 | success |
| npx through the test registry | 10 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | skipped |
| Stop test registry | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 2 | success |
| Complete job | 0 | success |
| **npm install smoke (darwin-x64)** | 55 | success |
| Set up job | 3 | success |
| Run actions/checkout@v7 | 3 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 1 | success |
| Extract crate version | 1 | success |
| Download npm preview | 4 | success |
| Start loopback test registry | 36 | success |
| Install pumice through the test registry | 1 | success |
| Run installed pumice --version | 0 | success |
| Executable bits after install | 0 | success |
| pumice transcribe (version and backends without a model) | 0 | skipped |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | success |
| Cache Whisper tiny Q8_0 model | 0 | skipped |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 0 | skipped |
| npx through the test registry | 1 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | success |
| Stop test registry | 1 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 0 | success |
| **npm install smoke (darwin-arm64)** | 99 | success |
| Set up job | 3 | success |
| Run actions/checkout@v7 | 2 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 1 | success |
| Extract crate version | 1 | success |
| Download npm preview | 4 | success |
| Start loopback test registry | 35 | success |
| Install pumice through the test registry | 2 | success |
| Run installed pumice --version | 0 | success |
| Executable bits after install | 0 | success |
| pumice transcribe (version and backends without a model) | 38 | success |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | skipped |
| Cache Whisper tiny Q8_0 model | 1 | success |
| Download and verify model | 1 | success |
| pumice transcribe file with the tiny model (CPU) | 3 | success |
| npx through the test registry | 1 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | success |
| Stop test registry | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 2 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 1 | success |
| **npm install smoke (linux-x64)** | 17 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 1 | success |
| Extract crate version | 1 | success |
| Download npm preview | 2 | success |
| Start loopback test registry | 1 | success |
| Install pumice through the test registry | 1 | success |
| Run installed pumice --version | 0 | success |
| Executable bits after install | 0 | success |
| pumice transcribe (version and backends without a model) | 0 | success |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | skipped |
| Cache Whisper tiny Q8_0 model | 0 | success |
| Download and verify model | 1 | success |
| pumice transcribe file with the tiny model (CPU) | 1 | success |
| npx through the test registry | 1 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | skipped |
| Stop test registry | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 2 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 1 | success |

</details>

<details>
<summary>A warm Transcription — 40987501189 attempt 2</summary>

| Job | Service seconds | Ready-to-start queue seconds | Result |
| --- | ---: | ---: | --- |
| Transcription CPU engine smoke (Linux) | 69 | 2 | success |
| Transcription Metal preview bundle (macOS arm64) | 841 | 9 | success |
| Transcription runtime CPU variants (Linux) | 99 | 2 | success |
| Transcription Vulkan preview bundle (Linux x64) | 214 | 2 | success |
| Transcription Vulkan preview bundle (Windows x64) | 382 | 2 | success |

| Job / step | Service seconds | Result |
| --- | ---: | --- |
| **Transcription CPU engine smoke (Linux)** | 69 | success |
| Set up job | 0 | success |
| Run actions/checkout@v5 | 2 | success |
| Run dtolnay/rust-toolchain@stable | 0 | success |
| Run ./.github/actions/cargo-cache | 5 | success |
| cargo clippy -p pumice-transcription --features engine | 3 | success |
| cargo test -p pumice-transcription --features opus | 17 | success |
| cargo build --release -p pumice-transcription --features engine | 36 | success |
| Cache Whisper tiny Q8_0 model | 1 | success |
| Download and verify model | 0 | skipped |
| Verify cached model | 0 | success |
| Smoke test transcription with tiny model | 1 | success |
| Backend policy without a GPU build (static engine) | 1 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run actions/checkout@v5 | 0 | success |
| Complete job | 0 | success |
| **Transcription Metal preview bundle (macOS arm64)** | 841 | success |
| Set up job | 2 | success |
| Run actions/checkout@v5 | 2 | success |
| Run ./.github/actions/macos-metal-bundle | 144 | success |
| cargo clippy -p pumice-transcription --features metal | 22 | success |
| cargo test -p pumice-transcription --features metal | 114 | success |
| Record bundle sizes | 0 | success |
| Write the test helpers | 0 | success |
| Backend registration without a model (backend cpu, auto and gpu) | 65 | success |
| Bundle manifest checks (damaged, unlisted, missing, bad manifest) | 33 | success |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 0 | skipped |
| Verify cached model | 0 | success |
| Smoke test a copied bundle with tiny model (path with spaces; backend cpu, auto, gpu) | 97 | success |
| Build pumice (orchestrator) | 95 | success |
| backend auto never loads Metal (fresh folder, corrupted module; file, managed serve, pumice serve) | 4 | success |
| Metal diagnostics: backend gpu without flash attention (text recorded only) | 88 | success |
| Metal diagnostics: ggml test-backend-ops on MTL0 (recorded only) | 141 | success |
| Quarantined copy (Gatekeeper) and the documented fix | 25 | success |
| Pack the preview bundle | 0 | success |
| Upload the preview bundle | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run ./.github/actions/macos-metal-bundle | 0 | success |
| Post Run actions/checkout@v5 | 1 | success |
| Complete job | 1 | success |
| **Transcription runtime CPU variants (Linux)** | 99 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 1 | success |
| Run dtolnay/rust-toolchain@stable | 2 | success |
| Run ./.github/actions/cargo-cache | 5 | success |
| cargo clippy -p pumice-transcription --features cpu-variants | 12 | success |
| cargo test -p pumice-transcription --features cpu-variants | 22 | success |
| Build and stage cpu-variants | 51 | success |
| Assert staged layout | 0 | success |
| Cache Whisper tiny Q8_0 model | 0 | success |
| Download and verify model | 0 | skipped |
| Verify cached model | 0 | success |
| Smoke test staged binary with tiny model (read-only install) | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run actions/checkout@v5 | 0 | success |
| Complete job | 0 | success |
| **Transcription Vulkan preview bundle (Linux x64)** | 214 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 1 | success |
| Run ./.github/actions/linux-vulkan-bundle | 68 | success |
| cargo clippy -p pumice-transcription --features vulkan | 11 | success |
| cargo test -p pumice-transcription --features vulkan | 67 | success |
| Record bundle sizes | 0 | success |
| Install the runtime test packages (Vulkan loader, Mesa) | 13 | success |
| Write the test helpers | 0 | success |
| Backend registration without a Vulkan device (backend cpu, auto and gpu) | 1 | success |
| Bundle manifest checks (damaged, unlisted, missing) | 1 | success |
| Cache Whisper tiny Q8_0 model | 1 | success |
| Download and verify model | 0 | skipped |
| Verify cached model | 0 | success |
| Smoke test the staged bundle with tiny model (no Vulkan device; backend cpu, auto, gpu) | 2 | success |
| Ubuntu 22.04 without a Vulkan loader (container) | 4 | success |
| Software Vulkan device (Mesa lavapipe; auto stays on the CPU, gpu uses it) | 36 | success |
| Pack the preview bundle | 3 | success |
| Upload the preview bundle | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run ./.github/actions/linux-vulkan-bundle | 0 | success |
| Post Run actions/checkout@v5 | 0 | success |
| Complete job | 0 | success |
| **Transcription Vulkan preview bundle (Windows x64)** | 382 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 6 | success |
| Run ./.github/actions/windows-vulkan-bundle | 198 | success |
| cargo clippy -p pumice-transcription --features vulkan | 24 | success |
| cargo test -p pumice-transcription --features vulkan | 118 | success |
| Record bundle sizes | 0 | success |
| Backend registration without a model (backend cpu, auto and gpu) | 1 | success |
| Bundle manifest checks (damaged, unlisted, missing) | 1 | success |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 0 | success |
| Smoke test the staged bundle with tiny model (backend cpu, auto, gpu) | 3 | success |
| DLL search order (decoy DLLs in the current directory and on PATH) | 19 | success |
| Upload the preview bundle | 4 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run ./.github/actions/windows-vulkan-bundle | 0 | success |
| Post Run actions/checkout@v5 | 2 | success |
| Complete job | 0 | success |

</details>

<details>
<summary>A DNS-fix CI — 40989357682 attempt 2</summary>

| Job | Service seconds | Ready-to-start queue seconds | Result |
| --- | ---: | ---: | --- |
| windows-latest | 283 | 73 | success |
| macos-latest | 332 | 33 | success |
| ubuntu-latest | 196 | 28 | success |

| Job / step | Service seconds | Result |
| --- | ---: | --- |
| **windows-latest** | 283 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 5 | success |
| Run dtolnay/rust-toolchain@stable | 7 | success |
| Run ./.github/actions/cargo-cache | 11 | success |
| cargo fmt --check | 0 | skipped |
| cargo clippy | 43 | success |
| cargo test | 107 | success |
| cargo build --release --bin pumice | 71 | success |
| cargo build --release --bin pumice-relay | 29 | success |
| Report release binary size | 0 | success |
| Set up MSVC environment | 3 | success |
| Verify release exe has no CRT DLL dependencies | 1 | success |
| Upload release binary | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run actions/checkout@v5 | 1 | success |
| Complete job | 0 | success |
| **macos-latest** | 332 | success |
| Set up job | 3 | success |
| Run actions/checkout@v5 | 3 | success |
| Run dtolnay/rust-toolchain@stable | 3 | success |
| Run ./.github/actions/cargo-cache | 22 | success |
| cargo fmt --check | 0 | skipped |
| cargo clippy | 18 | success |
| cargo test | 108 | success |
| cargo build --release --bin pumice | 120 | success |
| cargo build --release --bin pumice-relay | 42 | success |
| Report release binary size | 0 | success |
| Set up MSVC environment | 0 | skipped |
| Verify release exe has no CRT DLL dependencies | 0 | skipped |
| Upload release binary | 3 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run actions/checkout@v5 | 2 | success |
| Complete job | 4 | success |
| **ubuntu-latest** | 196 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 1 | success |
| Run dtolnay/rust-toolchain@stable | 1 | success |
| Run ./.github/actions/cargo-cache | 6 | success |
| cargo fmt --check | 1 | success |
| cargo clippy | 12 | success |
| cargo test | 77 | success |
| cargo build --release --bin pumice | 66 | success |
| cargo build --release --bin pumice-relay | 26 | success |
| Report release binary size | 0 | success |
| Set up MSVC environment | 0 | skipped |
| Verify release exe has no CRT DLL dependencies | 0 | skipped |
| Upload release binary | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run actions/checkout@v5 | 0 | success |
| Complete job | 0 | success |

</details>

<details>
<summary>A DNS-fix Release — 40989357683 attempt 2</summary>

| Job | Service seconds | Ready-to-start queue seconds | Result |
| --- | ---: | ---: | --- |
| Packaging helper tests | 16 | 2 | success |
| Transcription bundle (win32-x64, Vulkan) | 143 | 2 | success |
| x86_64-unknown-linux-musl | 110 | 3 | success |
| x86_64-pc-windows-msvc | 216 | 5 | success |
| x86_64-apple-darwin | 259 | 194 | success |
| Transcription bundle (darwin-arm64, Metal) | 126 | 455 | success |
| Transcription bundle (linux-x64, Vulkan) | 106 | 3 | success |
| aarch64-apple-darwin | 131 | 240 | success |
| Aggregate checksums | 6 | 2 | success |
| Stage and pack npm packages | 43 | 7 | success |
| pnpm/Bun install smoke (linux-x64) | 26 | 2 | success |
| npm install smoke (darwin-arm64) | 49 | 35 | success |
| npm install smoke (win32-x64) | 41 | 30 | success |
| npm install smoke (linux-x64) | 21 | 73 | success |
| npm install smoke (darwin-x64) | 38 | 257 | success |

| Job / step | Service seconds | Result |
| --- | ---: | --- |
| **Packaging helper tests** | 16 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 1 | success |
| Run packaging helper unit tests | 10 | success |
| Read the pinned Rust version | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 0 | success |
| **Transcription bundle (win32-x64, Vulkan)** | 143 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 4 | success |
| Run ./.github/actions/windows-vulkan-bundle | 130 | success |
| Check the stage starts | 0 | success |
| Upload the bundle | 3 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run ./.github/actions/windows-vulkan-bundle | 0 | success |
| Post Run actions/checkout@v7 | 2 | success |
| Complete job | 0 | success |
| **x86_64-unknown-linux-musl** | 110 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 2 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 0 | success |
| Run dtolnay/rust-toolchain@master | 11 | success |
| Install musl tools | 11 | success |
| Run ./.github/actions/cargo-cache | 4 | success |
| cargo build --release --locked --bin pumice | 52 | success |
| cargo build --release --locked --bin pumice-relay | 21 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 0 | skipped |
| Verify Linux binary is statically linked | 0 | success |
| Generate the core third-party notices | 1 | success |
| Package archive | 1 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **x86_64-pc-windows-msvc** | 216 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 6 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 1 | success |
| Run dtolnay/rust-toolchain@master | 15 | success |
| Install musl tools | 0 | skipped |
| Run ./.github/actions/cargo-cache | 5 | success |
| cargo build --release --locked --bin pumice | 132 | success |
| cargo build --release --locked --bin pumice-relay | 45 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 1 | success |
| Verify Linux binary is statically linked | 0 | skipped |
| Generate the core third-party notices | 0 | success |
| Package archive | 1 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 2 | success |
| Run ./.github/actions/cargo-cache-save | 1 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 2 | success |
| Complete job | 0 | success |
| **x86_64-apple-darwin** | 259 | success |
| Set up job | 3 | success |
| Run actions/checkout@v7 | 3 | success |
| Run actions/setup-python@v7 | 1 | success |
| Extract crate version | 0 | success |
| Run dtolnay/rust-toolchain@master | 14 | success |
| Install musl tools | 0 | skipped |
| Run ./.github/actions/cargo-cache | 7 | success |
| cargo build --release --locked --bin pumice | 159 | success |
| cargo build --release --locked --bin pumice-relay | 63 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 0 | skipped |
| Verify Linux binary is statically linked | 0 | skipped |
| Generate the core third-party notices | 1 | success |
| Package archive | 1 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 1 | success |
| Run ./.github/actions/cargo-cache-save | 1 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 1 | success |
| **Transcription bundle (darwin-arm64, Metal)** | 126 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 1 | success |
| Run ./.github/actions/macos-metal-bundle | 116 | success |
| Check the stage starts | 0 | success |
| Pack the bundle | 1 | success |
| Upload the bundle | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run ./.github/actions/macos-metal-bundle | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 1 | success |
| **Transcription bundle (linux-x64, Vulkan)** | 106 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 1 | success |
| Run ./.github/actions/linux-vulkan-bundle | 93 | success |
| Check the stage starts | 0 | success |
| Pack the bundle | 4 | success |
| Upload the bundle | 3 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run ./.github/actions/linux-vulkan-bundle | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **aarch64-apple-darwin** | 131 | success |
| Set up job | 3 | success |
| Run actions/checkout@v7 | 2 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 0 | success |
| Run dtolnay/rust-toolchain@master | 9 | success |
| Install musl tools | 0 | skipped |
| Run ./.github/actions/cargo-cache | 8 | success |
| cargo build --release --locked --bin pumice | 74 | success |
| cargo build --release --locked --bin pumice-relay | 25 | success |
| Smoke test binary --version | 1 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 0 | skipped |
| Verify Linux binary is statically linked | 0 | skipped |
| Generate the core third-party notices | 0 | success |
| Package archive | 1 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run actions/setup-python@v7 | 1 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 1 | success |
| **Aggregate checksums** | 6 | success |
| Set up job | 1 | success |
| Run actions/download-artifact@v8 | 1 | success |
| Generate SHA256SUMS | 0 | success |
| Upload release preview | 2 | success |
| Complete job | 0 | success |
| **Stage and pack npm packages** | 43 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 4 | success |
| Run dtolnay/rust-toolchain@master | 8 | success |
| Download release preview archives | 1 | success |
| Download the transcription bundles | 1 | success |
| Assemble the bundle stages | 1 | success |
| Generate the third-party notices | 5 | success |
| Stage and pack npm tarballs | 15 | success |
| Report package sizes | 0 | success |
| Upload npm preview | 3 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 1 | success |
| **pnpm/Bun install smoke (linux-x64)** | 26 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 3 | success |
| Run pnpm/action-setup@v6 | 6 | success |
| Run oven-sh/setup-bun@v2 | 2 | success |
| Extract crate version | 0 | success |
| Download npm preview | 4 | success |
| Start loopback test registry | 1 | success |
| Install and run with pnpm | 1 | success |
| Install and run with Bun | 0 | success |
| Stop test registry | 0 | success |
| Post Run oven-sh/setup-bun@v2 | 0 | success |
| Post Run pnpm/action-setup@v6 | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 0 | success |
| **npm install smoke (darwin-arm64)** | 49 | success |
| Set up job | 3 | success |
| Run actions/checkout@v7 | 2 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 1 | success |
| Extract crate version | 1 | success |
| Download npm preview | 1 | success |
| Start loopback test registry | 2 | success |
| Install pumice through the test registry | 1 | success |
| Run installed pumice --version | 0 | success |
| Executable bits after install | 0 | success |
| pumice transcribe (version and backends without a model) | 31 | success |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | skipped |
| Cache Whisper tiny Q8_0 model | 1 | success |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 1 | success |
| npx through the test registry | 1 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | success |
| Stop test registry | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 1 | success |
| **npm install smoke (win32-x64)** | 41 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 6 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 5 | success |
| Extract crate version | 3 | success |
| Download npm preview | 2 | success |
| Start loopback test registry | 1 | success |
| Install pumice through the test registry | 5 | success |
| Run installed pumice --version | 0 | success |
| Executable bits after install | 0 | skipped |
| pumice transcribe (version and backends without a model) | 1 | success |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | skipped |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 2 | success |
| npx through the test registry | 6 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | skipped |
| Stop test registry | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 3 | success |
| Complete job | 0 | success |
| **npm install smoke (linux-x64)** | 21 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 1 | success |
| Run actions/setup-node@v7 | 0 | success |
| Extract crate version | 1 | success |
| Download npm preview | 4 | success |
| Start loopback test registry | 1 | success |
| Install pumice through the test registry | 1 | success |
| Run installed pumice --version | 0 | success |
| Executable bits after install | 0 | success |
| pumice transcribe (version and backends without a model) | 0 | success |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | skipped |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 1 | success |
| npx through the test registry | 1 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | skipped |
| Stop test registry | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 1 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **npm install smoke (darwin-x64)** | 38 | success |
| Set up job | 5 | success |
| Run actions/checkout@v7 | 3 | success |
| Run actions/setup-python@v7 | 1 | success |
| Run actions/setup-node@v7 | 3 | success |
| Extract crate version | 1 | success |
| Download npm preview | 7 | success |
| Start loopback test registry | 2 | success |
| Install pumice through the test registry | 4 | success |
| Run installed pumice --version | 0 | success |
| Executable bits after install | 0 | success |
| pumice transcribe (version and backends without a model) | 0 | skipped |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | success |
| Cache Whisper tiny Q8_0 model | 0 | skipped |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 0 | skipped |
| npx through the test registry | 4 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | success |
| Stop test registry | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 1 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 1 | success |

</details>

<details>
<summary>A DNS-fix Transcription — 40989357726 attempt 2</summary>

| Job | Service seconds | Ready-to-start queue seconds | Result |
| --- | ---: | ---: | --- |
| Transcription CPU engine smoke (Linux) | 91 | 29 | success |
| Transcription Metal preview bundle (macOS arm64) | 911 | 205 | success |
| Transcription runtime CPU variants (Linux) | 94 | 67 | success |
| Transcription Vulkan preview bundle (Linux x64) | 342 | 72 | success |
| Transcription Vulkan preview bundle (Windows x64) | 297 | 20 | success |

| Job / step | Service seconds | Result |
| --- | ---: | --- |
| **Transcription CPU engine smoke (Linux)** | 91 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 1 | success |
| Run dtolnay/rust-toolchain@stable | 1 | success |
| Run ./.github/actions/cargo-cache | 10 | success |
| cargo clippy -p pumice-transcription --features engine | 9 | success |
| cargo test -p pumice-transcription --features opus | 20 | success |
| cargo build --release -p pumice-transcription --features engine | 41 | success |
| Cache Whisper tiny Q8_0 model | 1 | success |
| Download and verify model | 0 | skipped |
| Verify cached model | 0 | success |
| Smoke test transcription with tiny model | 2 | success |
| Backend policy without a GPU build (static engine) | 0 | success |
| Run ./.github/actions/cargo-cache-save | 1 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run actions/checkout@v5 | 0 | success |
| Complete job | 0 | success |
| **Transcription Metal preview bundle (macOS arm64)** | 911 | success |
| Set up job | 0 | success |
| Run actions/checkout@v5 | 2 | success |
| Run ./.github/actions/macos-metal-bundle | 112 | success |
| cargo clippy -p pumice-transcription --features metal | 22 | success |
| cargo test -p pumice-transcription --features metal | 103 | success |
| Record bundle sizes | 0 | success |
| Write the test helpers | 0 | success |
| Backend registration without a model (backend cpu, auto and gpu) | 67 | success |
| Bundle manifest checks (damaged, unlisted, missing, bad manifest) | 36 | success |
| Cache Whisper tiny Q8_0 model | 1 | success |
| Download and verify model | 0 | skipped |
| Verify cached model | 1 | success |
| Smoke test a copied bundle with tiny model (path with spaces; backend cpu, auto, gpu) | 163 | success |
| Build pumice (orchestrator) | 113 | success |
| backend auto never loads Metal (fresh folder, corrupted module; file, managed serve, pumice serve) | 5 | success |
| Metal diagnostics: backend gpu without flash attention (text recorded only) | 112 | success |
| Metal diagnostics: ggml test-backend-ops on MTL0 (recorded only) | 130 | success |
| Quarantined copy (Gatekeeper) and the documented fix | 36 | success |
| Pack the preview bundle | 1 | success |
| Upload the preview bundle | 1 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run ./.github/actions/macos-metal-bundle | 0 | success |
| Post Run actions/checkout@v5 | 1 | success |
| Complete job | 2 | success |
| **Transcription runtime CPU variants (Linux)** | 94 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 1 | success |
| Run dtolnay/rust-toolchain@stable | 1 | success |
| Run ./.github/actions/cargo-cache | 5 | success |
| cargo clippy -p pumice-transcription --features cpu-variants | 5 | success |
| cargo test -p pumice-transcription --features cpu-variants | 21 | success |
| Build and stage cpu-variants | 53 | success |
| Assert staged layout | 0 | success |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 0 | skipped |
| Verify cached model | 0 | success |
| Smoke test staged binary with tiny model (read-only install) | 1 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run actions/checkout@v5 | 1 | success |
| Complete job | 0 | success |
| **Transcription Vulkan preview bundle (Linux x64)** | 342 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 1 | success |
| Run ./.github/actions/linux-vulkan-bundle | 103 | success |
| cargo clippy -p pumice-transcription --features vulkan | 19 | success |
| cargo test -p pumice-transcription --features vulkan | 92 | success |
| Record bundle sizes | 0 | success |
| Install the runtime test packages (Vulkan loader, Mesa) | 14 | success |
| Write the test helpers | 0 | success |
| Backend registration without a Vulkan device (backend cpu, auto and gpu) | 1 | success |
| Bundle manifest checks (damaged, unlisted, missing) | 2 | success |
| Cache Whisper tiny Q8_0 model | 8 | success |
| Download and verify model | 0 | skipped |
| Verify cached model | 0 | success |
| Smoke test the staged bundle with tiny model (no Vulkan device; backend cpu, auto, gpu) | 3 | success |
| Ubuntu 22.04 without a Vulkan loader (container) | 5 | success |
| Software Vulkan device (Mesa lavapipe; auto stays on the CPU, gpu uses it) | 81 | success |
| Pack the preview bundle | 5 | success |
| Upload the preview bundle | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run ./.github/actions/linux-vulkan-bundle | 1 | success |
| Post Run actions/checkout@v5 | 0 | success |
| Complete job | 0 | success |
| **Transcription Vulkan preview bundle (Windows x64)** | 297 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 6 | success |
| Run ./.github/actions/windows-vulkan-bundle | 145 | success |
| cargo clippy -p pumice-transcription --features vulkan | 18 | success |
| cargo test -p pumice-transcription --features vulkan | 98 | success |
| Record bundle sizes | 0 | success |
| Backend registration without a model (backend cpu, auto and gpu) | 1 | success |
| Bundle manifest checks (damaged, unlisted, missing) | 1 | success |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 0 | success |
| Smoke test the staged bundle with tiny model (backend cpu, auto, gpu) | 3 | success |
| DLL search order (decoy DLLs in the current directory and on PATH) | 13 | success |
| Upload the preview bundle | 4 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run ./.github/actions/windows-vulkan-bundle | 0 | success |
| Post Run actions/checkout@v5 | 1 | success |
| Complete job | 1 | success |

</details>

<details>
<summary>B first CI — 40993089644 attempt 1</summary>

| Job | Service seconds | Ready-to-start queue seconds | Result |
| --- | ---: | ---: | --- |
| macos-latest | 194 | 3 | success |
| windows-latest | 386 | 2 | success |
| ubuntu-latest | 174 | 3 | success |

| Job / step | Service seconds | Result |
| --- | ---: | --- |
| **macos-latest** | 194 | success |
| Set up job | 2 | success |
| Run actions/checkout@v5 | 1 | success |
| Run dtolnay/rust-toolchain@stable | 8 | success |
| Run ./.github/actions/cargo-cache | 10 | success |
| cargo fmt --check | 0 | skipped |
| cargo clippy | 10 | success |
| cargo test | 75 | success |
| cargo build --release --bin pumice | 59 | success |
| cargo build --release --bin pumice-relay | 24 | success |
| Report release binary size | 0 | success |
| Set up MSVC environment | 0 | skipped |
| Verify release exe has no CRT DLL dependencies | 0 | skipped |
| Upload release binary | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run actions/checkout@v5 | 0 | success |
| Complete job | 2 | success |
| **windows-latest** | 386 | success |
| Set up job | 2 | success |
| Run actions/checkout@v5 | 7 | success |
| Run dtolnay/rust-toolchain@stable | 7 | success |
| Run ./.github/actions/cargo-cache | 14 | success |
| cargo fmt --check | 0 | skipped |
| cargo clippy | 47 | success |
| cargo test | 144 | success |
| cargo build --release --bin pumice | 109 | success |
| cargo build --release --bin pumice-relay | 43 | success |
| Report release binary size | 1 | success |
| Set up MSVC environment | 4 | success |
| Verify release exe has no CRT DLL dependencies | 1 | success |
| Upload release binary | 1 | success |
| Run ./.github/actions/cargo-cache-save | 1 | success |
| Post Run actions/checkout@v5 | 2 | success |
| Complete job | 0 | success |
| **ubuntu-latest** | 174 | success |
| Set up job | 2 | success |
| Run actions/checkout@v5 | 1 | success |
| Run dtolnay/rust-toolchain@stable | 2 | success |
| Run ./.github/actions/cargo-cache | 7 | success |
| cargo fmt --check | 3 | success |
| cargo clippy | 13 | success |
| cargo test | 69 | success |
| cargo build --release --bin pumice | 52 | success |
| cargo build --release --bin pumice-relay | 20 | success |
| Report release binary size | 0 | success |
| Set up MSVC environment | 0 | skipped |
| Verify release exe has no CRT DLL dependencies | 0 | skipped |
| Upload release binary | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run actions/checkout@v5 | 0 | success |
| Complete job | 0 | success |

</details>

<details>
<summary>B first Release — 40993089663 attempt 1</summary>

| Job | Service seconds | Ready-to-start queue seconds | Result |
| --- | ---: | ---: | --- |
| Packaging helper tests | 14 | 2 | success |
| Transcription bundle (win32-x64, Vulkan) | 159 | 2 | success |
| Transcription bundle (darwin-arm64, Metal) | 268 | 9 | success |
| x86_64-unknown-linux-musl | 105 | 2 | success |
| Transcription bundle (linux-x64, Vulkan) | 565 | 2 | success |
| x86_64-apple-darwin | 298 | 110 | success |
| x86_64-pc-windows-msvc | 205 | 2 | success |
| aarch64-apple-darwin | 153 | 32 | success |
| Aggregate checksums | 5 | 2 | success |
| Stage and pack npm packages | 42 | 3 | success |
| pnpm/Bun install smoke (linux-x64) | 18 | 1 | success |
| npm install smoke (darwin-x64) | 32 | 5 | success |
| npm install smoke (darwin-arm64) | 51 | 2 | success |
| npm install smoke (linux-x64) | 15 | 1 | success |
| npm install smoke (win32-x64) | 45 | 2 | success |

| Job / step | Service seconds | Result |
| --- | ---: | --- |
| **Packaging helper tests** | 14 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 1 | success |
| Run packaging helper unit tests | 6 | success |
| Read the pinned Rust version | 1 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **Transcription bundle (win32-x64, Vulkan)** | 159 | success |
| Set up job | 0 | success |
| Run actions/checkout@v7 | 5 | success |
| Run ./.github/actions/windows-vulkan-bundle | 145 | success |
| Check the stage starts | 0 | success |
| Upload the bundle | 4 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run ./.github/actions/windows-vulkan-bundle | 0 | success |
| Post Run actions/checkout@v7 | 2 | success |
| Complete job | 0 | success |
| **Transcription bundle (darwin-arm64, Metal)** | 268 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 2 | success |
| Run ./.github/actions/macos-metal-bundle | 253 | success |
| Check the stage starts | 0 | success |
| Pack the bundle | 1 | success |
| Upload the bundle | 1 | success |
| Run ./.github/actions/cargo-cache-save | 6 | success |
| Post Run ./.github/actions/macos-metal-bundle | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 1 | success |
| **x86_64-unknown-linux-musl** | 105 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 0 | success |
| Run dtolnay/rust-toolchain@master | 10 | success |
| Install musl tools | 7 | success |
| Run ./.github/actions/cargo-cache | 3 | success |
| cargo build --release --locked --bin pumice | 53 | success |
| cargo build --release --locked --bin pumice-relay | 21 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 0 | skipped |
| Verify Linux binary is statically linked | 0 | success |
| Generate the core third-party notices | 0 | success |
| Package archive | 1 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 2 | success |
| Run ./.github/actions/cargo-cache-save | 1 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **Transcription bundle (linux-x64, Vulkan)** | 565 | success |
| Set up job | 5 | success |
| Run actions/checkout@v7 | 2 | success |
| Run ./.github/actions/linux-vulkan-bundle | 546 | success |
| Check the stage starts | 0 | success |
| Pack the bundle | 5 | success |
| Upload the bundle | 1 | success |
| Run ./.github/actions/cargo-cache-save | 3 | success |
| Post Run ./.github/actions/linux-vulkan-bundle | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 0 | success |
| **x86_64-apple-darwin** | 298 | success |
| Set up job | 4 | success |
| Run actions/checkout@v7 | 4 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 1 | success |
| Run dtolnay/rust-toolchain@master | 18 | success |
| Install musl tools | 0 | skipped |
| Run ./.github/actions/cargo-cache | 22 | success |
| cargo build --release --locked --bin pumice | 179 | success |
| cargo build --release --locked --bin pumice-relay | 63 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 0 | skipped |
| Verify Linux binary is statically linked | 0 | skipped |
| Generate the core third-party notices | 1 | success |
| Package archive | 1 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 1 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run actions/setup-python@v7 | 1 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 0 | success |
| **x86_64-pc-windows-msvc** | 205 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 5 | success |
| Run actions/setup-python@v7 | 1 | success |
| Extract crate version | 1 | success |
| Run dtolnay/rust-toolchain@master | 11 | success |
| Install musl tools | 0 | skipped |
| Run ./.github/actions/cargo-cache | 4 | success |
| cargo build --release --locked --bin pumice | 129 | success |
| cargo build --release --locked --bin pumice-relay | 44 | success |
| Smoke test binary --version | 1 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 0 | success |
| Verify Linux binary is statically linked | 0 | skipped |
| Generate the core third-party notices | 1 | success |
| Package archive | 1 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 1 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run actions/setup-python@v7 | 1 | success |
| Post Run actions/checkout@v7 | 2 | success |
| Complete job | 0 | success |
| **aarch64-apple-darwin** | 153 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 2 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 0 | success |
| Run dtolnay/rust-toolchain@master | 7 | success |
| Install musl tools | 0 | skipped |
| Run ./.github/actions/cargo-cache | 7 | success |
| cargo build --release --locked --bin pumice | 86 | success |
| cargo build --release --locked --bin pumice-relay | 38 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 1 | success |
| Verify exe has no CRT DLL dependencies | 0 | skipped |
| Verify Linux binary is statically linked | 0 | skipped |
| Generate the core third-party notices | 0 | success |
| Package archive | 1 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 3 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 2 | success |
| **Aggregate checksums** | 5 | success |
| Set up job | 1 | success |
| Run actions/download-artifact@v8 | 1 | success |
| Generate SHA256SUMS | 0 | success |
| Upload release preview | 2 | success |
| Complete job | 0 | success |
| **Stage and pack npm packages** | 42 | success |
| Set up job | 3 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 1 | success |
| Run dtolnay/rust-toolchain@master | 9 | success |
| Download release preview archives | 2 | success |
| Download the transcription bundles | 3 | success |
| Assemble the bundle stages | 1 | success |
| Generate the third-party notices | 4 | success |
| Stage and pack npm tarballs | 12 | success |
| Report package sizes | 0 | success |
| Upload npm preview | 4 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **pnpm/Bun install smoke (linux-x64)** | 18 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 1 | success |
| Run actions/setup-node@v7 | 0 | success |
| Run pnpm/action-setup@v6 | 6 | success |
| Run oven-sh/setup-bun@v2 | 1 | success |
| Extract crate version | 0 | success |
| Download npm preview | 1 | success |
| Start loopback test registry | 1 | success |
| Install and run with pnpm | 1 | success |
| Install and run with Bun | 1 | success |
| Stop test registry | 0 | success |
| Post Run oven-sh/setup-bun@v2 | 1 | success |
| Post Run pnpm/action-setup@v6 | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 1 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **npm install smoke (darwin-x64)** | 32 | success |
| Set up job | 4 | success |
| Run actions/checkout@v7 | 5 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 2 | success |
| Extract crate version | 1 | success |
| Download npm preview | 7 | success |
| Start loopback test registry | 1 | success |
| Install pumice through the test registry | 2 | success |
| Run installed pumice --version | 0 | success |
| Executable bits after install | 0 | success |
| pumice transcribe (version and backends without a model) | 0 | skipped |
| pumice transcribe without a bundle (Intel Mac, core only) | 1 | success |
| Cache Whisper tiny Q8_0 model | 0 | skipped |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 0 | skipped |
| npx through the test registry | 1 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | success |
| Stop test registry | 0 | success |
| Post Run actions/setup-node@v7 | 1 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 1 | success |
| **npm install smoke (darwin-arm64)** | 51 | success |
| Set up job | 3 | success |
| Run actions/checkout@v7 | 2 | success |
| Run actions/setup-python@v7 | 1 | success |
| Run actions/setup-node@v7 | 1 | success |
| Extract crate version | 0 | success |
| Download npm preview | 1 | success |
| Start loopback test registry | 1 | success |
| Install pumice through the test registry | 1 | success |
| Run installed pumice --version | 1 | success |
| Executable bits after install | 0 | success |
| pumice transcribe (version and backends without a model) | 31 | success |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | skipped |
| Cache Whisper tiny Q8_0 model | 0 | success |
| Download and verify model | 3 | success |
| pumice transcribe file with the tiny model (CPU) | 1 | success |
| npx through the test registry | 0 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | success |
| Stop test registry | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 1 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 1 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 1 | success |
| **npm install smoke (linux-x64)** | 15 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 0 | success |
| Extract crate version | 1 | success |
| Download npm preview | 2 | success |
| Start loopback test registry | 1 | success |
| Install pumice through the test registry | 0 | success |
| Run installed pumice --version | 1 | success |
| Executable bits after install | 0 | success |
| pumice transcribe (version and backends without a model) | 0 | success |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | skipped |
| Cache Whisper tiny Q8_0 model | 0 | success |
| Download and verify model | 1 | success |
| pumice transcribe file with the tiny model (CPU) | 1 | success |
| npx through the test registry | 0 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | skipped |
| Stop test registry | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 2 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **npm install smoke (win32-x64)** | 45 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 6 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 5 | success |
| Extract crate version | 2 | success |
| Download npm preview | 6 | success |
| Start loopback test registry | 1 | success |
| Install pumice through the test registry | 5 | success |
| Run installed pumice --version | 0 | success |
| Executable bits after install | 0 | skipped |
| pumice transcribe (version and backends without a model) | 2 | success |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | skipped |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 1 | success |
| npx through the test registry | 7 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | skipped |
| Stop test registry | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 3 | success |
| Complete job | 0 | success |

</details>

<details>
<summary>B first Transcription — 40993089668 attempt 1</summary>

| Job | Service seconds | Ready-to-start queue seconds | Result |
| --- | ---: | ---: | --- |
| Transcription Vulkan preview bundle (Linux x64) | 670 | 2 | success |
| Transcription runtime CPU variants (Linux) | 99 | 5 | success |
| Transcription Vulkan preview bundle (Windows x64) | 384 | 2 | success |
| Transcription CPU engine smoke (Linux) | 87 | 2 | success |
| Transcription Metal preview bundle (macOS arm64) | 1051 | 41 | success |

| Job / step | Service seconds | Result |
| --- | ---: | --- |
| **Transcription Vulkan preview bundle (Linux x64)** | 670 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 1 | success |
| Run ./.github/actions/linux-vulkan-bundle | 463 | success |
| cargo clippy -p pumice-transcription --features vulkan | 15 | success |
| cargo test -p pumice-transcription --features vulkan | 90 | success |
| Record bundle sizes | 0 | success |
| Install the runtime test packages (Vulkan loader, Mesa) | 13 | success |
| Write the test helpers | 0 | success |
| Backend registration without a Vulkan device (backend cpu, auto and gpu) | 1 | success |
| Bundle manifest checks (damaged, unlisted, missing) | 1 | success |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 0 | skipped |
| Verify cached model | 0 | success |
| Smoke test the staged bundle with tiny model (no Vulkan device; backend cpu, auto, gpu) | 2 | success |
| Ubuntu 22.04 without a Vulkan loader (container) | 5 | success |
| Software Vulkan device (Mesa lavapipe; auto stays on the CPU, gpu uses it) | 63 | success |
| Pack the preview bundle | 4 | success |
| Upload the preview bundle | 2 | success |
| Run ./.github/actions/cargo-cache-save | 2 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run ./.github/actions/linux-vulkan-bundle | 1 | success |
| Post Run actions/checkout@v5 | 0 | success |
| Complete job | 0 | success |
| **Transcription runtime CPU variants (Linux)** | 99 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 3 | success |
| Run dtolnay/rust-toolchain@stable | 1 | success |
| Run ./.github/actions/cargo-cache | 7 | success |
| cargo clippy -p pumice-transcription --features cpu-variants | 14 | success |
| cargo test -p pumice-transcription --features cpu-variants | 20 | success |
| Build and stage cpu-variants | 45 | success |
| Assert staged layout | 0 | success |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 0 | skipped |
| Verify cached model | 0 | success |
| Smoke test staged binary with tiny model (read-only install) | 1 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run actions/checkout@v5 | 1 | success |
| Complete job | 0 | success |
| **Transcription Vulkan preview bundle (Windows x64)** | 384 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 7 | success |
| Run ./.github/actions/windows-vulkan-bundle | 188 | success |
| cargo clippy -p pumice-transcription --features vulkan | 24 | success |
| cargo test -p pumice-transcription --features vulkan | 129 | success |
| Record bundle sizes | 0 | success |
| Backend registration without a model (backend cpu, auto and gpu) | 1 | success |
| Bundle manifest checks (damaged, unlisted, missing) | 2 | success |
| Cache Whisper tiny Q8_0 model | 1 | success |
| Download and verify model | 1 | success |
| Smoke test the staged bundle with tiny model (backend cpu, auto, gpu) | 3 | success |
| DLL search order (decoy DLLs in the current directory and on PATH) | 18 | success |
| Upload the preview bundle | 4 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run ./.github/actions/windows-vulkan-bundle | 1 | success |
| Post Run actions/checkout@v5 | 1 | success |
| Complete job | 0 | success |
| **Transcription CPU engine smoke (Linux)** | 87 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 1 | success |
| Run dtolnay/rust-toolchain@stable | 1 | success |
| Run ./.github/actions/cargo-cache | 4 | success |
| cargo clippy -p pumice-transcription --features engine | 6 | success |
| cargo test -p pumice-transcription --features opus | 20 | success |
| cargo build --release -p pumice-transcription --features engine | 48 | success |
| Cache Whisper tiny Q8_0 model | 1 | success |
| Download and verify model | 0 | skipped |
| Verify cached model | 0 | success |
| Smoke test transcription with tiny model | 2 | success |
| Backend policy without a GPU build (static engine) | 1 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run actions/checkout@v5 | 0 | success |
| Complete job | 0 | success |
| **Transcription Metal preview bundle (macOS arm64)** | 1051 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 2 | success |
| Run ./.github/actions/macos-metal-bundle | 257 | success |
| cargo clippy -p pumice-transcription --features metal | 21 | success |
| cargo test -p pumice-transcription --features metal | 106 | success |
| Record bundle sizes | 0 | success |
| Write the test helpers | 0 | success |
| Backend registration without a model (backend cpu, auto and gpu) | 71 | success |
| Bundle manifest checks (damaged, unlisted, missing, bad manifest) | 35 | success |
| Cache Whisper tiny Q8_0 model | 1 | success |
| Download and verify model | 0 | skipped |
| Verify cached model | 0 | success |
| Smoke test a copied bundle with tiny model (path with spaces; backend cpu, auto, gpu) | 176 | success |
| Build pumice (orchestrator) | 102 | success |
| backend auto never loads Metal (fresh folder, corrupted module; file, managed serve, pumice serve) | 5 | success |
| Metal diagnostics: backend gpu without flash attention (text recorded only) | 119 | success |
| Metal diagnostics: ggml test-backend-ops on MTL0 (recorded only) | 114 | success |
| Quarantined copy (Gatekeeper) and the documented fix | 32 | success |
| Pack the preview bundle | 0 | success |
| Upload the preview bundle | 1 | success |
| Run ./.github/actions/cargo-cache-save | 3 | success |
| Post Cache Whisper tiny Q8_0 model | 1 | success |
| Post Run ./.github/actions/macos-metal-bundle | 0 | success |
| Post Run actions/checkout@v5 | 0 | success |
| Complete job | 2 | success |

</details>

<details>
<summary>B warm Release — 40993089663 attempt 2</summary>

| Job | Service seconds | Ready-to-start queue seconds | Result |
| --- | ---: | ---: | --- |
| Packaging helper tests | 14 | 2 | success |
| Transcription bundle (darwin-arm64, Metal) | 164 | 3 | success |
| x86_64-unknown-linux-musl | 120 | 3 | success |
| Transcription bundle (win32-x64, Vulkan) | 184 | 2 | success |
| x86_64-pc-windows-msvc | 178 | 2 | success |
| x86_64-apple-darwin | 264 | 4 | success |
| Transcription bundle (linux-x64, Vulkan) | 97 | 3 | success |
| aarch64-apple-darwin | 173 | 4 | success |
| Aggregate checksums | 7 | 2 | success |
| Stage and pack npm packages | 39 | 2 | success |
| npm install smoke (win32-x64) | 33 | 2 | success |
| npm install smoke (darwin-x64) | 38 | 4 | success |
| npm install smoke (darwin-arm64) | 49 | 7 | success |
| pnpm/Bun install smoke (linux-x64) | 25 | 2 | success |
| npm install smoke (linux-x64) | 17 | 2 | success |

| Job / step | Service seconds | Result |
| --- | ---: | --- |
| **Packaging helper tests** | 14 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 1 | success |
| Run packaging helper unit tests | 8 | success |
| Read the pinned Rust version | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **Transcription bundle (darwin-arm64, Metal)** | 164 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 2 | success |
| Run ./.github/actions/macos-metal-bundle | 153 | success |
| Check the stage starts | 0 | success |
| Pack the bundle | 1 | success |
| Upload the bundle | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run ./.github/actions/macos-metal-bundle | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 1 | success |
| **x86_64-unknown-linux-musl** | 120 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 0 | success |
| Run dtolnay/rust-toolchain@master | 12 | success |
| Install musl tools | 7 | success |
| Run ./.github/actions/cargo-cache | 2 | success |
| cargo build --release --locked --bin pumice | 64 | success |
| cargo build --release --locked --bin pumice-relay | 26 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 0 | skipped |
| Verify Linux binary is statically linked | 0 | success |
| Generate the core third-party notices | 0 | success |
| Package archive | 1 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 0 | success |
| **Transcription bundle (win32-x64, Vulkan)** | 184 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 6 | success |
| Run ./.github/actions/windows-vulkan-bundle | 168 | success |
| Check the stage starts | 1 | success |
| Upload the bundle | 3 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run ./.github/actions/windows-vulkan-bundle | 0 | success |
| Post Run actions/checkout@v7 | 3 | success |
| Complete job | 0 | success |
| **x86_64-pc-windows-msvc** | 178 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 5 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 2 | success |
| Run dtolnay/rust-toolchain@master | 11 | success |
| Install musl tools | 0 | skipped |
| Run ./.github/actions/cargo-cache | 5 | success |
| cargo build --release --locked --bin pumice | 110 | success |
| cargo build --release --locked --bin pumice-relay | 34 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 1 | success |
| Verify exe has no CRT DLL dependencies | 0 | success |
| Verify Linux binary is statically linked | 0 | skipped |
| Generate the core third-party notices | 1 | success |
| Package archive | 0 | success |
| Smoke test packaged archive | 1 | success |
| Upload release archive | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 2 | success |
| Complete job | 0 | success |
| **x86_64-apple-darwin** | 264 | success |
| Set up job | 3 | success |
| Run actions/checkout@v7 | 3 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 0 | success |
| Run dtolnay/rust-toolchain@master | 20 | success |
| Install musl tools | 0 | skipped |
| Run ./.github/actions/cargo-cache | 24 | success |
| cargo build --release --locked --bin pumice | 154 | success |
| cargo build --release --locked --bin pumice-relay | 50 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 0 | skipped |
| Verify Linux binary is statically linked | 0 | skipped |
| Generate the core third-party notices | 1 | success |
| Package archive | 1 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 2 | success |
| Run ./.github/actions/cargo-cache-save | 1 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 0 | success |
| **Transcription bundle (linux-x64, Vulkan)** | 97 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 1 | success |
| Run ./.github/actions/linux-vulkan-bundle | 83 | success |
| Check the stage starts | 0 | success |
| Pack the bundle | 5 | success |
| Upload the bundle | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run ./.github/actions/linux-vulkan-bundle | 1 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **aarch64-apple-darwin** | 173 | success |
| Set up job | 3 | success |
| Run actions/checkout@v7 | 2 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 0 | success |
| Run dtolnay/rust-toolchain@master | 9 | success |
| Install musl tools | 0 | skipped |
| Run ./.github/actions/cargo-cache | 8 | success |
| cargo build --release --locked --bin pumice | 103 | success |
| cargo build --release --locked --bin pumice-relay | 39 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 0 | skipped |
| Verify Linux binary is statically linked | 0 | skipped |
| Generate the core third-party notices | 0 | success |
| Package archive | 1 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 2 | success |
| Run ./.github/actions/cargo-cache-save | 1 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 1 | success |
| **Aggregate checksums** | 7 | success |
| Set up job | 1 | success |
| Run actions/download-artifact@v8 | 1 | success |
| Generate SHA256SUMS | 0 | success |
| Upload release preview | 2 | success |
| Complete job | 0 | success |
| **Stage and pack npm packages** | 39 | success |
| Set up job | 3 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 1 | success |
| Run dtolnay/rust-toolchain@master | 7 | success |
| Download release preview archives | 2 | success |
| Download the transcription bundles | 2 | success |
| Assemble the bundle stages | 0 | success |
| Generate the third-party notices | 4 | success |
| Stage and pack npm tarballs | 11 | success |
| Report package sizes | 0 | success |
| Upload npm preview | 3 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 1 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **npm install smoke (win32-x64)** | 33 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 5 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 5 | success |
| Extract crate version | 2 | success |
| Download npm preview | 2 | success |
| Start loopback test registry | 1 | success |
| Install pumice through the test registry | 4 | success |
| Run installed pumice --version | 1 | success |
| Executable bits after install | 0 | skipped |
| pumice transcribe (version and backends without a model) | 0 | success |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | skipped |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 1 | success |
| npx through the test registry | 5 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | skipped |
| Stop test registry | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 1 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 0 | success |
| **npm install smoke (darwin-x64)** | 38 | success |
| Set up job | 4 | success |
| Run actions/checkout@v7 | 3 | success |
| Run actions/setup-python@v7 | 1 | success |
| Run actions/setup-node@v7 | 2 | success |
| Extract crate version | 1 | success |
| Download npm preview | 7 | success |
| Start loopback test registry | 3 | success |
| Install pumice through the test registry | 4 | success |
| Run installed pumice --version | 0 | success |
| Executable bits after install | 0 | success |
| pumice transcribe (version and backends without a model) | 0 | skipped |
| pumice transcribe without a bundle (Intel Mac, core only) | 1 | success |
| Cache Whisper tiny Q8_0 model | 0 | skipped |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 0 | skipped |
| npx through the test registry | 3 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | success |
| Stop test registry | 0 | success |
| Post Run actions/setup-node@v7 | 1 | success |
| Post Run actions/setup-python@v7 | 1 | success |
| Post Run actions/checkout@v7 | 2 | success |
| Complete job | 1 | success |
| **npm install smoke (darwin-arm64)** | 49 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 2 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 1 | success |
| Extract crate version | 1 | success |
| Download npm preview | 3 | success |
| Start loopback test registry | 1 | success |
| Install pumice through the test registry | 1 | success |
| Run installed pumice --version | 0 | success |
| Executable bits after install | 0 | success |
| pumice transcribe (version and backends without a model) | 28 | success |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | skipped |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 2 | success |
| npx through the test registry | 1 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | success |
| Stop test registry | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 1 | success |
| **pnpm/Bun install smoke (linux-x64)** | 25 | success |
| Set up job | 3 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 1 | success |
| Run actions/setup-node@v7 | 2 | success |
| Run pnpm/action-setup@v6 | 5 | success |
| Run oven-sh/setup-bun@v2 | 2 | success |
| Extract crate version | 0 | success |
| Download npm preview | 3 | success |
| Start loopback test registry | 1 | success |
| Install and run with pnpm | 1 | success |
| Install and run with Bun | 0 | success |
| Stop test registry | 0 | success |
| Post Run oven-sh/setup-bun@v2 | 0 | success |
| Post Run pnpm/action-setup@v6 | 0 | success |
| Post Run actions/setup-node@v7 | 1 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **npm install smoke (linux-x64)** | 17 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 1 | success |
| Extract crate version | 0 | success |
| Download npm preview | 3 | success |
| Start loopback test registry | 1 | success |
| Install pumice through the test registry | 1 | success |
| Run installed pumice --version | 0 | success |
| Executable bits after install | 0 | success |
| pumice transcribe (version and backends without a model) | 0 | success |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | skipped |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 1 | success |
| npx through the test registry | 1 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | skipped |
| Stop test registry | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |

</details>

<details>
<summary>C first CI — 40993159760 attempt 1</summary>

| Job | Service seconds | Ready-to-start queue seconds | Result |
| --- | ---: | ---: | --- |
| ubuntu-latest | 198 | 2 | success |
| macos-latest | 242 | 313 | success |
| windows-latest | 557 | 2 | success |

| Job / step | Service seconds | Result |
| --- | ---: | --- |
| **ubuntu-latest** | 198 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 2 | success |
| Run dtolnay/rust-toolchain@stable | 1 | success |
| Run ./.github/actions/cargo-cache | 8 | success |
| cargo fmt --check | 3 | success |
| cargo clippy | 15 | success |
| cargo test | 76 | success |
| cargo build --release --bin pumice | 61 | success |
| cargo build --release --bin pumice-relay | 25 | success |
| Report release binary size | 0 | success |
| Set up MSVC environment | 0 | skipped |
| Verify release exe has no CRT DLL dependencies | 0 | skipped |
| Upload release binary | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run actions/checkout@v5 | 0 | success |
| Complete job | 0 | success |
| **macos-latest** | 242 | success |
| Set up job | 2 | success |
| Run actions/checkout@v5 | 2 | success |
| Run dtolnay/rust-toolchain@stable | 2 | success |
| Run ./.github/actions/cargo-cache | 12 | success |
| cargo fmt --check | 0 | skipped |
| cargo clippy | 16 | success |
| cargo test | 98 | success |
| cargo build --release --bin pumice | 70 | success |
| cargo build --release --bin pumice-relay | 28 | success |
| Report release binary size | 0 | success |
| Set up MSVC environment | 0 | skipped |
| Verify release exe has no CRT DLL dependencies | 0 | skipped |
| Upload release binary | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run actions/checkout@v5 | 0 | success |
| Complete job | 7 | success |
| **windows-latest** | 557 | success |
| Set up job | 2 | success |
| Run actions/checkout@v5 | 6 | success |
| Run dtolnay/rust-toolchain@stable | 6 | success |
| Run ./.github/actions/cargo-cache | 4 | success |
| cargo fmt --check | 0 | skipped |
| cargo clippy | 90 | success |
| cargo test | 227 | success |
| cargo build --release --bin pumice | 157 | success |
| cargo build --release --bin pumice-relay | 41 | success |
| Report release binary size | 0 | success |
| Set up MSVC environment | 6 | success |
| Verify release exe has no CRT DLL dependencies | 0 | success |
| Upload release binary | 2 | success |
| Run ./.github/actions/cargo-cache-save | 11 | success |
| Post Run actions/checkout@v5 | 2 | success |
| Complete job | 0 | success |

</details>

<details>
<summary>C first Release — 40993159759 attempt 1</summary>

| Job | Service seconds | Ready-to-start queue seconds | Result |
| --- | ---: | ---: | --- |
| Locate shared transcription bundles | 678 | 3 | success |
| Packaging helper tests | 17 | 2 | success |
| x86_64-apple-darwin | 303 | 378 | success |
| x86_64-pc-windows-msvc | 175 | 2 | success |
| aarch64-apple-darwin | 200 | 173 | success |
| x86_64-unknown-linux-musl | 123 | 8 | success |
| Transcription bundle (win32-x64, Vulkan) | 81 | 2 | success |
| Transcription bundle (linux-x64, Vulkan) | 63 | 2 | success |
| Transcription bundle (darwin-arm64, Metal) | 29 | 8 | success |
| Aggregate checksums | 8 | 2 | success |
| Stage and pack npm packages | 45 | 2 | success |
| pnpm/Bun install smoke (linux-x64) | 18 | 1 | success |
| npm install smoke (darwin-x64) | 21 | 3 | success |
| npm install smoke (win32-x64) | 45 | 2 | success |
| npm install smoke (linux-x64) | 14 | 2 | success |
| npm install smoke (darwin-arm64) | 63 | 7 | success |

| Job / step | Service seconds | Result |
| --- | ---: | --- |
| **Locate shared transcription bundles** | 678 | success |
| Set up job | 0 | success |
| Run actions/checkout@v7 | 1 | success |
| Locate same-PR bundle producer | 674 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **Packaging helper tests** | 17 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 3 | success |
| Run packaging helper unit tests | 7 | success |
| Read the pinned Rust version | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **x86_64-apple-darwin** | 303 | success |
| Set up job | 4 | success |
| Run actions/checkout@v7 | 3 | success |
| Run actions/setup-python@v7 | 1 | success |
| Extract crate version | 0 | success |
| Run dtolnay/rust-toolchain@master | 28 | success |
| Install musl tools | 0 | skipped |
| Run ./.github/actions/cargo-cache | 9 | success |
| cargo build --release --locked --bin pumice | 197 | success |
| cargo build --release --locked --bin pumice-relay | 45 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 0 | skipped |
| Verify Linux binary is statically linked | 0 | skipped |
| Generate the core third-party notices | 0 | success |
| Package archive | 1 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 3 | success |
| Run ./.github/actions/cargo-cache-save | 7 | success |
| Post Run actions/setup-python@v7 | 1 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 1 | success |
| **x86_64-pc-windows-msvc** | 175 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 6 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 1 | success |
| Run dtolnay/rust-toolchain@master | 11 | success |
| Install musl tools | 0 | skipped |
| Run ./.github/actions/cargo-cache | 6 | success |
| cargo build --release --locked --bin pumice | 110 | success |
| cargo build --release --locked --bin pumice-relay | 31 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 0 | success |
| Verify Linux binary is statically linked | 0 | skipped |
| Generate the core third-party notices | 1 | success |
| Package archive | 0 | success |
| Smoke test packaged archive | 1 | success |
| Upload release archive | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 2 | success |
| Complete job | 0 | success |
| **aarch64-apple-darwin** | 200 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 3 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 0 | success |
| Run dtolnay/rust-toolchain@master | 9 | success |
| Install musl tools | 0 | skipped |
| Run ./.github/actions/cargo-cache | 5 | success |
| cargo build --release --locked --bin pumice | 132 | success |
| cargo build --release --locked --bin pumice-relay | 35 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 0 | skipped |
| Verify Linux binary is statically linked | 0 | skipped |
| Generate the core third-party notices | 1 | success |
| Package archive | 0 | success |
| Smoke test packaged archive | 1 | success |
| Upload release archive | 1 | success |
| Run ./.github/actions/cargo-cache-save | 6 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 1 | success |
| **x86_64-unknown-linux-musl** | 123 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 2 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 1 | success |
| Run dtolnay/rust-toolchain@master | 12 | success |
| Install musl tools | 8 | success |
| Run ./.github/actions/cargo-cache | 2 | success |
| cargo build --release --locked --bin pumice | 64 | success |
| cargo build --release --locked --bin pumice-relay | 26 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 0 | skipped |
| Verify Linux binary is statically linked | 0 | success |
| Generate the core third-party notices | 0 | success |
| Package archive | 1 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 0 | success |
| **Transcription bundle (win32-x64, Vulkan)** | 81 | success |
| Set up job | 0 | success |
| Run actions/checkout@v7 | 6 | success |
| Run ./.github/actions/windows-vulkan-bundle | 59 | success |
| Check the stage starts | 1 | success |
| Upload the bundle | 3 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run ./.github/actions/windows-vulkan-bundle | 9 | success |
| Post Run actions/checkout@v7 | 2 | success |
| Complete job | 0 | success |
| **Transcription bundle (linux-x64, Vulkan)** | 63 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 1 | success |
| Run ./.github/actions/linux-vulkan-bundle | 46 | success |
| Check the stage starts | 0 | success |
| Pack the bundle | 4 | success |
| Upload the bundle | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run ./.github/actions/linux-vulkan-bundle | 7 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **Transcription bundle (darwin-arm64, Metal)** | 29 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 3 | success |
| Run ./.github/actions/macos-metal-bundle | 17 | success |
| Check the stage starts | 0 | success |
| Pack the bundle | 1 | success |
| Upload the bundle | 2 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run ./.github/actions/macos-metal-bundle | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 0 | success |
| **Aggregate checksums** | 8 | success |
| Set up job | 1 | success |
| Run actions/download-artifact@v8 | 2 | success |
| Generate SHA256SUMS | 0 | success |
| Upload release preview | 2 | success |
| Complete job | 0 | success |
| **Stage and pack npm packages** | 45 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 1 | success |
| Run dtolnay/rust-toolchain@master | 9 | success |
| Download release preview archives | 3 | success |
| Download the transcription bundles | 3 | success |
| Assemble the bundle stages | 0 | success |
| Generate the third-party notices | 4 | success |
| Stage and pack npm tarballs | 14 | success |
| Report package sizes | 0 | success |
| Upload npm preview | 4 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **pnpm/Bun install smoke (linux-x64)** | 18 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 1 | success |
| Run actions/setup-node@v7 | 0 | success |
| Run pnpm/action-setup@v6 | 6 | success |
| Run oven-sh/setup-bun@v2 | 1 | success |
| Extract crate version | 0 | success |
| Download npm preview | 1 | success |
| Start loopback test registry | 1 | success |
| Install and run with pnpm | 1 | success |
| Install and run with Bun | 0 | success |
| Stop test registry | 0 | success |
| Post Run oven-sh/setup-bun@v2 | 2 | success |
| Post Run pnpm/action-setup@v6 | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 0 | success |
| **npm install smoke (darwin-x64)** | 21 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 2 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 1 | success |
| Extract crate version | 0 | success |
| Download npm preview | 4 | success |
| Start loopback test registry | 1 | success |
| Install pumice through the test registry | 2 | success |
| Run installed pumice --version | 0 | success |
| Executable bits after install | 0 | success |
| pumice transcribe (version and backends without a model) | 0 | skipped |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | success |
| Cache Whisper tiny Q8_0 model | 0 | skipped |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 0 | skipped |
| npx through the test registry | 1 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | success |
| Stop test registry | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 1 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **npm install smoke (win32-x64)** | 45 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 5 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 4 | success |
| Extract crate version | 2 | success |
| Download npm preview | 3 | success |
| Start loopback test registry | 2 | success |
| Install pumice through the test registry | 7 | success |
| Run installed pumice --version | 0 | success |
| Executable bits after install | 0 | skipped |
| pumice transcribe (version and backends without a model) | 1 | success |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | skipped |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 1 | success |
| npx through the test registry | 12 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | skipped |
| Stop test registry | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 3 | success |
| Complete job | 0 | success |
| **npm install smoke (linux-x64)** | 14 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 1 | success |
| Extract crate version | 0 | success |
| Download npm preview | 2 | success |
| Start loopback test registry | 1 | success |
| Install pumice through the test registry | 1 | success |
| Run installed pumice --version | 0 | success |
| Executable bits after install | 0 | success |
| pumice transcribe (version and backends without a model) | 0 | success |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | skipped |
| Cache Whisper tiny Q8_0 model | 0 | success |
| Download and verify model | 1 | success |
| pumice transcribe file with the tiny model (CPU) | 1 | success |
| npx through the test registry | 1 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | skipped |
| Stop test registry | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 1 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **npm install smoke (darwin-arm64)** | 63 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 2 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 0 | success |
| Extract crate version | 1 | success |
| Download npm preview | 6 | success |
| Start loopback test registry | 1 | success |
| Install pumice through the test registry | 1 | success |
| Run installed pumice --version | 0 | success |
| Executable bits after install | 0 | success |
| pumice transcribe (version and backends without a model) | 37 | success |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | skipped |
| Cache Whisper tiny Q8_0 model | 1 | success |
| Download and verify model | 2 | success |
| pumice transcribe file with the tiny model (CPU) | 1 | success |
| npx through the test registry | 1 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | success |
| Stop test registry | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 2 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 1 | success |

</details>

<details>
<summary>C first Transcription — 40993159761 attempt 1</summary>

| Job | Service seconds | Ready-to-start queue seconds | Result |
| --- | ---: | ---: | --- |
| Transcription CPU engine smoke (Linux) | 94 | 79 | success |
| Transcription Vulkan preview bundle (Windows x64) | 337 | 7 | success |
| Transcription Metal preview bundle (macOS arm64) | 985 | 144 | success |
| Transcription Vulkan preview bundle (Linux x64) | 772 | 90 | success |
| Transcription runtime CPU variants (Linux) | 674 | 117 | success |

| Job / step | Service seconds | Result |
| --- | ---: | --- |
| **Transcription CPU engine smoke (Linux)** | 94 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 1 | success |
| Run dtolnay/rust-toolchain@stable | 1 | success |
| Run ./.github/actions/cargo-cache | 5 | success |
| cargo clippy -p pumice-transcription --features engine | 9 | success |
| cargo test -p pumice-transcription --features opus | 21 | success |
| cargo build --release -p pumice-transcription --features engine | 49 | success |
| Cache Whisper tiny Q8_0 model | 1 | success |
| Download and verify model | 0 | skipped |
| Verify cached model | 0 | success |
| Smoke test transcription with tiny model | 2 | success |
| Backend policy without a GPU build (static engine) | 1 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run actions/checkout@v5 | 0 | success |
| Complete job | 0 | success |
| **Transcription Vulkan preview bundle (Windows x64)** | 337 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 4 | success |
| Run ./.github/actions/windows-vulkan-bundle | 160 | success |
| cargo clippy -p pumice-transcription --features vulkan | 21 | success |
| cargo test -p pumice-transcription --features vulkan | 118 | success |
| Record bundle sizes | 0 | success |
| Backend registration without a model (backend cpu, auto and gpu) | 1 | success |
| Bundle manifest checks (damaged, unlisted, missing) | 1 | success |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 1 | success |
| Smoke test the staged bundle with tiny model (backend cpu, auto, gpu) | 2 | success |
| DLL search order (decoy DLLs in the current directory and on PATH) | 16 | success |
| Upload the preview bundle | 4 | success |
| Run ./.github/actions/cargo-cache-save | 1 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run ./.github/actions/windows-vulkan-bundle | 0 | success |
| Post Run actions/checkout@v5 | 2 | success |
| Complete job | 0 | success |
| **Transcription Metal preview bundle (macOS arm64)** | 985 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 2 | success |
| Run ./.github/actions/macos-metal-bundle | 257 | success |
| cargo clippy -p pumice-transcription --features metal | 27 | success |
| cargo test -p pumice-transcription --features metal | 107 | success |
| Record bundle sizes | 0 | success |
| Write the test helpers | 0 | success |
| Backend registration without a model (backend cpu, auto and gpu) | 62 | success |
| Bundle manifest checks (damaged, unlisted, missing, bad manifest) | 27 | success |
| Cache Whisper tiny Q8_0 model | 1 | success |
| Download and verify model | 0 | skipped |
| Verify cached model | 0 | success |
| Smoke test a copied bundle with tiny model (path with spaces; backend cpu, auto, gpu) | 180 | success |
| Build pumice (orchestrator) | 90 | success |
| backend auto never loads Metal (fresh folder, corrupted module; file, managed serve, pumice serve) | 3 | success |
| Metal diagnostics: backend gpu without flash attention (text recorded only) | 60 | success |
| Metal diagnostics: ggml test-backend-ops on MTL0 (recorded only) | 125 | success |
| Quarantined copy (Gatekeeper) and the documented fix | 29 | success |
| Pack the preview bundle | 1 | success |
| Upload the preview bundle | 2 | success |
| Run ./.github/actions/cargo-cache-save | 6 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run ./.github/actions/macos-metal-bundle | 0 | success |
| Post Run actions/checkout@v5 | 1 | success |
| Complete job | 2 | success |
| **Transcription Vulkan preview bundle (Linux x64)** | 772 | success |
| Set up job | 2 | success |
| Run actions/checkout@v5 | 1 | success |
| Run ./.github/actions/linux-vulkan-bundle | 539 | success |
| cargo clippy -p pumice-transcription --features vulkan | 19 | success |
| cargo test -p pumice-transcription --features vulkan | 90 | success |
| Record bundle sizes | 0 | success |
| Install the runtime test packages (Vulkan loader, Mesa) | 13 | success |
| Write the test helpers | 0 | success |
| Backend registration without a Vulkan device (backend cpu, auto and gpu) | 1 | success |
| Bundle manifest checks (damaged, unlisted, missing) | 2 | success |
| Cache Whisper tiny Q8_0 model | 1 | success |
| Download and verify model | 0 | skipped |
| Verify cached model | 1 | success |
| Smoke test the staged bundle with tiny model (no Vulkan device; backend cpu, auto, gpu) | 2 | success |
| Ubuntu 22.04 without a Vulkan loader (container) | 5 | success |
| Software Vulkan device (Mesa lavapipe; auto stays on the CPU, gpu uses it) | 78 | success |
| Pack the preview bundle | 5 | success |
| Upload the preview bundle | 2 | success |
| Run ./.github/actions/cargo-cache-save | 4 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run ./.github/actions/linux-vulkan-bundle | 5 | success |
| Post Run actions/checkout@v5 | 0 | success |
| Complete job | 0 | success |
| **Transcription runtime CPU variants (Linux)** | 674 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 1 | success |
| Run dtolnay/rust-toolchain@stable | 1 | success |
| Run ./.github/actions/cargo-cache | 2 | success |
| cargo clippy -p pumice-transcription --features cpu-variants | 206 | success |
| cargo test -p pumice-transcription --features cpu-variants | 203 | success |
| Build and stage cpu-variants | 250 | success |
| Assert staged layout | 0 | success |
| Cache Whisper tiny Q8_0 model | 1 | success |
| Download and verify model | 0 | skipped |
| Verify cached model | 0 | success |
| Smoke test staged binary with tiny model (read-only install) | 1 | success |
| Run ./.github/actions/cargo-cache-save | 5 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run actions/checkout@v5 | 0 | success |
| Complete job | 0 | success |

</details>

<details>
<summary>C warm Release — 40993159759 attempt 2</summary>

| Job | Service seconds | Ready-to-start queue seconds | Result |
| --- | ---: | ---: | --- |
| Locate shared transcription bundles | 5 | 2 | success |
| Packaging helper tests | 16 | 2 | success |
| Transcription bundle (darwin-arm64, Metal) | 24 | 8 | success |
| x86_64-apple-darwin | 390 | 5 | success |
| Transcription bundle (linux-x64, Vulkan) | 55 | 3 | success |
| Transcription bundle (win32-x64, Vulkan) | 54 | 3 | success |
| x86_64-unknown-linux-musl | 158 | 3 | success |
| x86_64-pc-windows-msvc | 280 | 3 | success |
| aarch64-apple-darwin | 175 | 9 | success |
| Aggregate checksums | 9 | 3 | success |
| Stage and pack npm packages | 32 | 2 | success |
| pnpm/Bun install smoke (linux-x64) | 18 | 2 | success |
| npm install smoke (darwin-arm64) | 64 | 3 | success |
| npm install smoke (linux-x64) | 20 | 2 | success |
| npm install smoke (win32-x64) | 51 | 3 | success |
| npm install smoke (darwin-x64) | 34 | 3 | success |

| Job / step | Service seconds | Result |
| --- | ---: | --- |
| **Locate shared transcription bundles** | 5 | success |
| Set up job | 0 | success |
| Run actions/checkout@v7 | 1 | success |
| Locate same-PR bundle producer | 1 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **Packaging helper tests** | 16 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 2 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 2 | success |
| Run packaging helper unit tests | 7 | success |
| Read the pinned Rust version | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **Transcription bundle (darwin-arm64, Metal)** | 24 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 2 | success |
| Run ./.github/actions/macos-metal-bundle | 13 | success |
| Check the stage starts | 1 | success |
| Pack the bundle | 0 | success |
| Upload the bundle | 2 | success |
| Run ./.github/actions/cargo-cache-save | 1 | success |
| Post Run ./.github/actions/macos-metal-bundle | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 1 | success |
| **x86_64-apple-darwin** | 390 | success |
| Set up job | 5 | success |
| Run actions/checkout@v7 | 4 | success |
| Run actions/setup-python@v7 | 1 | success |
| Extract crate version | 0 | success |
| Run dtolnay/rust-toolchain@master | 21 | success |
| Install musl tools | 0 | skipped |
| Run ./.github/actions/cargo-cache | 10 | success |
| cargo build --release --locked --bin pumice | 256 | success |
| cargo build --release --locked --bin pumice-relay | 82 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 0 | skipped |
| Verify Linux binary is statically linked | 0 | skipped |
| Generate the core third-party notices | 1 | success |
| Package archive | 2 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 1 | success |
| Run ./.github/actions/cargo-cache-save | 1 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 1 | success |
| **Transcription bundle (linux-x64, Vulkan)** | 55 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 1 | success |
| Run ./.github/actions/linux-vulkan-bundle | 37 | success |
| Check the stage starts | 0 | success |
| Pack the bundle | 4 | success |
| Upload the bundle | 8 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run ./.github/actions/linux-vulkan-bundle | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 0 | success |
| **Transcription bundle (win32-x64, Vulkan)** | 54 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 6 | success |
| Run ./.github/actions/windows-vulkan-bundle | 39 | success |
| Check the stage starts | 1 | success |
| Upload the bundle | 3 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run ./.github/actions/windows-vulkan-bundle | 1 | success |
| Post Run actions/checkout@v7 | 2 | success |
| Complete job | 0 | success |
| **x86_64-unknown-linux-musl** | 158 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 2 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 1 | success |
| Run dtolnay/rust-toolchain@master | 11 | success |
| Install musl tools | 8 | success |
| Run ./.github/actions/cargo-cache | 1 | success |
| cargo build --release --locked --bin pumice | 96 | success |
| cargo build --release --locked --bin pumice-relay | 26 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 0 | skipped |
| Verify Linux binary is statically linked | 0 | success |
| Generate the core third-party notices | 0 | success |
| Package archive | 1 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 3 | success |
| Run ./.github/actions/cargo-cache-save | 4 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 0 | success |
| **x86_64-pc-windows-msvc** | 280 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 6 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 2 | success |
| Run dtolnay/rust-toolchain@master | 11 | success |
| Install musl tools | 0 | skipped |
| Run ./.github/actions/cargo-cache | 4 | success |
| cargo build --release --locked --bin pumice | 195 | success |
| cargo build --release --locked --bin pumice-relay | 47 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 1 | success |
| Verify Linux binary is statically linked | 0 | skipped |
| Generate the core third-party notices | 1 | success |
| Package archive | 0 | success |
| Smoke test packaged archive | 1 | success |
| Upload release archive | 1 | success |
| Run ./.github/actions/cargo-cache-save | 5 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 3 | success |
| Complete job | 0 | success |
| **aarch64-apple-darwin** | 175 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 2 | success |
| Run actions/setup-python@v7 | 0 | success |
| Extract crate version | 1 | success |
| Run dtolnay/rust-toolchain@master | 8 | success |
| Install musl tools | 0 | skipped |
| Run ./.github/actions/cargo-cache | 10 | success |
| cargo build --release --locked --bin pumice | 101 | success |
| cargo build --release --locked --bin pumice-relay | 39 | success |
| Smoke test binary --version | 0 | success |
| Smoke test pumice-relay --version | 0 | success |
| Verify exe has no CRT DLL dependencies | 0 | skipped |
| Verify Linux binary is statically linked | 0 | skipped |
| Generate the core third-party notices | 2 | success |
| Package archive | 0 | success |
| Smoke test packaged archive | 0 | success |
| Upload release archive | 3 | success |
| Run ./.github/actions/cargo-cache-save | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 2 | success |
| **Aggregate checksums** | 9 | success |
| Set up job | 1 | success |
| Run actions/download-artifact@v8 | 2 | success |
| Generate SHA256SUMS | 0 | success |
| Upload release preview | 3 | success |
| Complete job | 0 | success |
| **Stage and pack npm packages** | 32 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 2 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 0 | success |
| Run dtolnay/rust-toolchain@master | 7 | success |
| Download release preview archives | 1 | success |
| Download the transcription bundles | 1 | success |
| Assemble the bundle stages | 1 | success |
| Generate the third-party notices | 4 | success |
| Stage and pack npm tarballs | 9 | success |
| Report package sizes | 0 | success |
| Upload npm preview | 3 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **pnpm/Bun install smoke (linux-x64)** | 18 | success |
| Set up job | 1 | success |
| Run actions/checkout@v7 | 2 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 1 | success |
| Run pnpm/action-setup@v6 | 5 | success |
| Run oven-sh/setup-bun@v2 | 1 | success |
| Extract crate version | 1 | success |
| Download npm preview | 1 | success |
| Start loopback test registry | 1 | success |
| Install and run with pnpm | 0 | success |
| Install and run with Bun | 1 | success |
| Stop test registry | 0 | success |
| Post Run oven-sh/setup-bun@v2 | 0 | success |
| Post Run pnpm/action-setup@v6 | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 1 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **npm install smoke (darwin-arm64)** | 64 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 3 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 1 | success |
| Extract crate version | 1 | success |
| Download npm preview | 6 | success |
| Start loopback test registry | 1 | success |
| Install pumice through the test registry | 1 | success |
| Run installed pumice --version | 0 | success |
| Executable bits after install | 0 | success |
| pumice transcribe (version and backends without a model) | 36 | success |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | skipped |
| Cache Whisper tiny Q8_0 model | 3 | success |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 2 | success |
| npx through the test registry | 1 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | success |
| Stop test registry | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 1 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 1 | success |
| **npm install smoke (linux-x64)** | 20 | success |
| Set up job | 2 | success |
| Run actions/checkout@v7 | 1 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 0 | success |
| Extract crate version | 1 | success |
| Download npm preview | 6 | success |
| Start loopback test registry | 1 | success |
| Install pumice through the test registry | 1 | success |
| Run installed pumice --version | 0 | success |
| Executable bits after install | 0 | success |
| pumice transcribe (version and backends without a model) | 0 | success |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | skipped |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 0 | success |
| npx through the test registry | 1 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | skipped |
| Stop test registry | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 0 | success |
| Complete job | 0 | success |
| **npm install smoke (win32-x64)** | 51 | success |
| Set up job | 3 | success |
| Run actions/checkout@v7 | 7 | success |
| Run actions/setup-python@v7 | 1 | success |
| Run actions/setup-node@v7 | 7 | success |
| Extract crate version | 4 | success |
| Download npm preview | 2 | success |
| Start loopback test registry | 1 | success |
| Install pumice through the test registry | 6 | success |
| Run installed pumice --version | 1 | success |
| Executable bits after install | 0 | skipped |
| pumice transcribe (version and backends without a model) | 1 | success |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | skipped |
| Cache Whisper tiny Q8_0 model | 2 | success |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 1 | success |
| npx through the test registry | 11 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | skipped |
| Stop test registry | 0 | success |
| Post Cache Whisper tiny Q8_0 model | 0 | success |
| Post Run actions/setup-node@v7 | 0 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 2 | success |
| Complete job | 0 | success |
| **npm install smoke (darwin-x64)** | 34 | success |
| Set up job | 6 | success |
| Run actions/checkout@v7 | 5 | success |
| Run actions/setup-python@v7 | 0 | success |
| Run actions/setup-node@v7 | 2 | success |
| Extract crate version | 1 | success |
| Download npm preview | 6 | success |
| Start loopback test registry | 3 | success |
| Install pumice through the test registry | 3 | success |
| Run installed pumice --version | 0 | success |
| Executable bits after install | 1 | success |
| pumice transcribe (version and backends without a model) | 0 | skipped |
| pumice transcribe without a bundle (Intel Mac, core only) | 0 | success |
| Cache Whisper tiny Q8_0 model | 0 | skipped |
| Download and verify model | 0 | skipped |
| pumice transcribe file with the tiny model (CPU) | 0 | skipped |
| npx through the test registry | 3 | success |
| Quarantine attribute on the npm-installed files (macOS) | 0 | success |
| Stop test registry | 0 | success |
| Post Run actions/setup-node@v7 | 1 | success |
| Post Run actions/setup-python@v7 | 0 | success |
| Post Run actions/checkout@v7 | 1 | success |
| Complete job | 0 | success |

</details>
