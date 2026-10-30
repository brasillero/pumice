# CI speed comparison (2026-10-30, validation in progress)

This report is **incomplete**. Pending measurements are not claimed as wins. The task remains In progress. No merge is authorized by this report.

## Workflow comparison

Wall time is first job start to last job completion (includes later runner contention); runner minutes sum job durations, without OS billing multipliers. Initial queue time is separate. The slow-job tables below show service time, not queue. A single sample cannot establish causality.

| Workflow | Historical baseline wall / runner min | PR A cold wall / runner min | PR A warm | PR B cold / warm | PR C |
| --- | --- | --- | --- | --- | --- |
| CI | 8m57 / 21.0 | 9m11 / 17.2, failed Mac; exclude as a successful comparison | attempt 3 of 40987501204 pending | cold 8m26 / 18.2; warm not required for unchanged Rust jobs | pending |
| Release preview | 17m56 / 45.3 | 13m56 / 46.3 | attempt 2 of 40987501170 pending | 40989214048 / 40990338617 pending | 40989944137 pending |
| Transcription engine | 19m13 / 68.6 | 14m37 / 57.9 | attempt 2 of 40987501189 pending | 40989214066 / 40990341042 pending | 40989944195 pending |

Historical baseline runs: CI 40980838739, Release 40980838699, Transcription 40980838776. Their initial queues were 2 seconds. PR A cold queues were about 2 seconds. PR B CI initial queue was 3 seconds. Other queue/critical-path measurements must be filled from completed attempts, without mixing jobs from retries. PR A's original macOS retry was canceled by the authorized fixture-fix push under PR-scoped concurrency; it does not establish whether the original timing failure repeats. The warm CI rerun is the next complete check.

The older baseline used an older Linux runner image; same-day `ubuntu-latest` is ubuntu26. PR A cold CPU variants dropped from 19m12 to 11m30 before #6. Do not attribute that entire decrease to caching or native reuse.

## Changes and proof

- PR A [#133](https://github.com/brasillero/pumice/pull/133): explicit Cargo caches, core release caches, PR-scoped concurrency, streamed fixture manifests, targeted dumpbin lookup. Follow-up commits 336ad58 and f9e461d remove HTTPServer's reverse DNS lookup and verify no DNS is called when binding numeric loopback. Cold dumpbin time is 0 seconds versus 47–72 seconds historically. The first fixture patch still took about 36 seconds on Mac; the DNS follow-up awaits macOS npm-smoke timing.
- PR B [#136](https://github.com/brasillero/pumice/pull/136): Linux and macOS CMake ccache launchers through TRANSCRIBE_CMAKE_ARGS; preserve those settings in the bundle scripts; versioned native compiler/backend/SDK/script keys; statistics; installed Vulkan SDK caches. Windows keeps Visual Studio because CMake's compiler-launcher property supports Makefile/Ninja generators. No Ninja switch, /O2 change or CRT change.
- PR C [#137](https://github.com/brasillero/pumice/pull/137): a Linux coordinator locates the same PR/head Transcription run and waits for early audited bundle uploads. Release consumers reuse only an exact source-tree, Rust, Cargo, compiler, SDK, CMake, runner-image and build-environment identity match. Failed/canceled/absent producers fall back to independent builds. All producer tests continue to gate the PR. Manual previews build independently. Cold and warm coordinator runner minutes must be included when assessing the net saving.

Coverage removed: **none intended**. Three-OS tests, debug Rust tests, native Intel Mac archives and npm installs, static linkage checks, bundle.json validation, ELF/DLL/Mach-O audits, notices, npm/pnpm/Bun installs and release LTO/one codegen unit remain. Linux Vulkan still runs on Ubuntu 22.04 (glibc 2.35). Release retains pinned Rust; Transcription retains stable, including independent compilation when identity differs. Full CI validation of PR C is pending; this is a description of retained steps, not a passing-run claim.

## #6 measured independently

Source: `ci-speed-cpu-variants/reports/2026-10-30-cpu-variants.md` as read from origin/ci-speed-cpu-variants (the report will land with PR #134). Code measured at d70c1b68a1965bd35aefcd8e8dd0b0fd436a4258.

| CPU-variants job | Historical cold | PR A same-day cold | #6 cold | #6 warm |
| --- | ---: | ---: | ---: | ---: |
| Job duration | 19m02–19m12 | 11m30 | 8m41 | 1m34 |
| Queue | 2–3 s | 2 s | 47 s | 4 s |
| Release staging | 416–419 s | 251 s | 419 s | 56 s |
| Debug clippy | 355–364 s | 209 s | 37 s | 3 s |
| Debug tests | 355–356 s | 214 s | 46 s | 19 s |

The isolated warm CPU measurement is run 40988030401 attempt 2; other jobs in that experiment were canceled by its owner, so this is not a full-workflow success claim. Cache: 338,929,087 bytes compressed (323.2 MiB), warm exact hit; no compiler cache. Debug-profile coverage remains. #6 is owned by the other coder; no CPU-variants script or job changes are made in B/C.

## #7 decision and cache budget

Implement the installed SDK cache: benchmark run 40987788551 measured Linux 26 to 6 seconds; Windows 35 to 11 seconds (plus 2 seconds for its file-existence probe). Current/installed compressed bytes: Linux 313,376,180 / 505,196,271; Windows 304,812,970 / 437,240,281. Cold misses still download and SHA-256 verify the pinned SDK. Warm restores retain preflight checks; the actual warm Windows build remains required to prove it works without rerunning the installer.

PR #135 was closed unmerged; its two bench caches (942,436,552 bytes total) and remote branch were deleted. See `reports/2026-10-30-native-cache.md` for the experiment.

PR A caches: CI Linux about 335 MiB and Windows 444 MiB; core release 92–111 MiB; bundles Windows 140 MiB, Linux 130–156 MiB, Metal 110 MiB; Transcription engine 319 MiB and CPU variants 338 MiB (handover observations). PR B first completed Metal compilation reported 160/160 cacheable calls, zero hits; Linux reported 1,062 misses and zero hits, with cache storage about 0.03 GB of its 0.40 GB limit. Warm compiler hit rates remain pending.

Repository cache usage reached **8,790,736,759 bytes across 92 entries** at 02:12 UTC. The repository's 10 GB budget is close: PR-scoped caches, installed SDK copies and rolling native cache keys can evict earlier entries. Native caches are bounded to 400 MiB per backend before compression, not 400 MiB total repository storage. Cache misses after eviction must not be interpreted as a failed launcher. API snapshots and hit/miss logs must accompany the final warm comparison.

## #8 outcome and dependency map

Use component reuse through #5; keep every preview and both Mac architectures on every applicable PR. No main-only preview or reduced Intel testing is introduced. Cross-compilation is not introduced: Intel is rarely the critical path, and no measured evidence here justifies a generator/toolchain change while keeping native archive/install proof.

Dependency map: `release.yml` still covers all Rust crates, root source/manifests, .cargo configuration, packaging/notices/npm helper inputs, shared actions and fixtures. It now also covers `transcription.yml` and the bundle-sharing helper/tests, because producer behavior affects release consumption. `transcription.yml` retains its transcription/manifests/.cargo/fixture/action paths and also covers the sharing helper/tests. Coordinator wait and cache identity are explicit dependencies, not a reason to skip packaging proof. #8 remains pending until the component-reuse run is measured.

## Local checks and unresolved validation

cargo fmt --check, cargo clippy --workspace --all-targets -- -D warnings, cargo test --workspace, Python helper tests and actionlint 1.7.12 passed. shellcheck was unavailable; changed Bash was reviewed and parsed with bash -n.

Required follow-up: finish all A warm runs; confirm the macOS timing test and DNS-fixture improvement; finish B cold and warm (including restored Windows SDK compilation and native hit statistics); finish C and calculate net runner minutes including its coordinator; record final cache sizes and per-job queues/critical paths; only then close the corresponding backlog items. No task-state Done transition is made.

## Per-job and per-step service times

Seconds from GitHub timestamps. These are the completed historical baseline and PR A cold attempts; retry queues are not blended into cold results. GitHub reports composite action steps as one outer step; detailed compiler/cache statistics come from logs.

<details>
<summary>Baseline CI</summary>

| Job / step | Seconds | Result |
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
<summary>Baseline Release</summary>

| Job / step | Seconds | Result |
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
<summary>Baseline Transcription</summary>

| Job / step | Seconds | Result |
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
<summary>PR A cold Release</summary>

| Job / step | Seconds | Result |
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
<summary>PR A cold Transcription</summary>

| Job / step | Seconds | Result |
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
<summary>PR B cold CI</summary>

| Job / step | Seconds | Result |
| --- | ---: | --- |
| **ubuntu-latest** | 229 | success |
| Set up job | 2 | success |
| Run actions/checkout@v5 | 1 | success |
| Run dtolnay/rust-toolchain@stable | 1 | success |
| Run ./.github/actions/cargo-cache | 1 | success |
| cargo fmt --check | 1 | success |
| cargo clippy | 28 | success |
| cargo test | 90 | success |
| cargo build --release --bin pumice | 75 | success |
| cargo build --release --bin pumice-relay | 20 | success |
| Report release binary size | 0 | success |
| Set up MSVC environment | 0 | skipped |
| Verify release exe has no CRT DLL dependencies | 0 | skipped |
| Upload release binary | 2 | success |
| Run ./.github/actions/cargo-cache-save | 5 | success |
| Post Run actions/checkout@v5 | 1 | success |
| Complete job | 0 | success |
| **windows-latest** | 506 | success |
| Set up job | 1 | success |
| Run actions/checkout@v5 | 8 | success |
| Run dtolnay/rust-toolchain@stable | 13 | success |
| Run ./.github/actions/cargo-cache | 6 | success |
| cargo fmt --check | 0 | skipped |
| cargo clippy | 95 | success |
| cargo test | 173 | success |
| cargo build --release --bin pumice | 151 | success |
| cargo build --release --bin pumice-relay | 36 | success |
| Report release binary size | 0 | success |
| Set up MSVC environment | 5 | success |
| Verify release exe has no CRT DLL dependencies | 1 | success |
| Upload release binary | 2 | success |
| Run ./.github/actions/cargo-cache-save | 11 | success |
| Post Run actions/checkout@v5 | 2 | success |
| Complete job | 0 | success |
| **macos-latest** | 357 | success |
| Set up job | 2 | success |
| Run actions/checkout@v5 | 2 | success |
| Run dtolnay/rust-toolchain@stable | 2 | success |
| Run ./.github/actions/cargo-cache | 4 | success |
| cargo fmt --check | 0 | skipped |
| cargo clippy | 45 | success |
| cargo test | 109 | success |
| cargo build --release --bin pumice | 128 | success |
| cargo build --release --bin pumice-relay | 49 | success |
| Report release binary size | 0 | success |
| Set up MSVC environment | 0 | skipped |
| Verify release exe has no CRT DLL dependencies | 0 | skipped |
| Upload release binary | 1 | success |
| Run ./.github/actions/cargo-cache-save | 9 | success |
| Post Run actions/checkout@v5 | 1 | success |
| Complete job | 3 | success |

</details>
