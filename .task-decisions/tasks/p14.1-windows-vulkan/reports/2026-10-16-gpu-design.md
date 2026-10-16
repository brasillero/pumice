# P14 GPU transcription design research

2026-10-16. Repository inspected at `469847c44930d1470c96eac678bca18979b9701f`.
Research only: no repository edits, installations, model execution, or engine builds.
Recommendations below are proposals, not newly recorded owner decisions.
**UNVERIFIED** means source-supported design or estimate without the relevant runtime/build validation.

## 1. Recommendation and existing constraints

Ship an optional Windows x64 Vulkan bundle first, containing the existing runtime CPU variants and one loadable Vulkan module. Keep a separate portable CPU-only archive. Then deliver macOS arm64 Metal and Linux GNU/glibc Vulkan in separate packaging/validation PRs. Keep macOS Intel on CPU; defer CUDA.

This follows `AGENTS.md`, `arch-modules/D2,D8`, and architecture-plan §4. P6/D2 currently defers downloadable transcription packages and Windows/macOS engine delivery: this research does not reverse it. A supervisor/owner must explicitly schedule the proposed packaging work. P7/D2 requires one CPU module per directory, without runtime copying or handwritten ggml loader FFI. Reuse that design.

Corrections to the task's assumptions at this commit:
- Implementation is `crates/transcription/src/engine.rs::cpp`, not `src/engine/cpp/*`.
- `engine` explicitly loads `Backend::Cpu`; enabling an upstream Vulkan Cargo feature alone cannot enable GPU use.
- `cpu-variants` enables `engine` + `transcribe-cpp/dynamic-backends`, but runtime loading/staging is Linux-only; the ISA table supports x86-64 only.
- `backend` exists in `file --json` and human `serve` startup output, **not** managed `ready`, `/ready`, or stack status. Parent lifecycle and readiness parsers reject unknown fields (`src/orchestrator/serve/child.rs:189,727`). Both ends must change together.
- `release.yml` currently packages only the core executable; it builds relay too but does not package transcription. Transcription CI currently exercises the native engine on Linux only. The P7 research note's “not merged” status is stale; task notes say Done/PR #99 and code is present.

## 2. Pinned engine capabilities and module-loading behavior

Installed crates: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/transcribe-cpp{,-sys}-0.3.1/`. Both `.cargo_vcs_info.json` files identify upstream commit `3f32fbcc7bb3246851a0234263438bc3c0fa1cac`. Below, `R:` is the safe crate and `S:` is the sys crate, relative to those directories.

Verified from `R:Cargo.toml`, `S:bindings/rust/sys/build.rs:120–245`, and `S:CMakeLists.txt:125–157,366–400`:
- Cargo GPU features: `vulkan`, `metal`, `cuda`, `rocm`; also `shared`, `dynamic-backends`, `openmp`. Default upstream feature is `metal`; Pumice disables upstream defaults.
- `vulkan` → `TRANSCRIBE_VULKAN=ON` → `GGML_VULKAN=ON`; CUDA → `TRANSCRIBE_CUDA`, ROCm → `TRANSCRIBE_HIP`. Metal is Apple-only and explicitly disabled when its feature is absent.
- `dynamic-backends` implies shared libtranscribe/ggml, `TRANSCRIBE_GGML_BACKEND_DL=ON`, `GGML_BACKEND_DL=ON`; on x86 it also enables all CPU variants and a conservative floor. `GGML_NATIVE` is forced OFF. GPU and CPU backends become CMake `MODULE` libraries.
- Native source contains additional ggml backends, but that does not mean the Rust crate exposes or validates them as supported transcription choices. No need to expand scope beyond the listed Cargo GPU features.
- `TRANSCRIBE_CMAKE_ARGS` and then `CMAKE_ARGS` are applied after feature definitions. The existing portable flags must be replaced with the P7 safe subset for accelerated builds, with a separate target/cache directory. Sanitize conflicting ambient build flags; do not accidentally inherit `TRANSCRIBE_DIR` (which bypasses source building and makes features inert).

**Yes, a Vulkan module can coexist with the CPU variants.** `S:ggml/src/ggml-vulkan/CMakeLists.txt` uses `ggml_add_backend_library(ggml-vulkan)`; the common helper creates a loadable module. Vulkan is a dependency of that module, not a required startup dependency of the executable/core libraries in this posture. Missing Vulkan loader makes that module fail to load and be skipped. The CPU module remains usable.

Recommended startup sequence, reusing P7's chooser:
1. Disable native transcription logging before initialization, as today. Resolve only trusted install-relative directories. Determine one immutable allowed mask: CPU-only for `cpu`, CPU|VULKAN for Windows/Linux accelerated `auto/gpu`, CPU|METAL for macOS accelerated `auto/gpu`. Do **not** use `BackendMask::ALL`: OTHER would also permit `GGML_BACKEND_PATH` external plugins.
2. Choose an ISA module with the existing Rust OS-aware detector, intersected with modules shipped for this target. Initialize the selected `cpu/<variant>/` with the FINAL mask, even before loading GPU code. Require a usable CPU device, including when GPU was requested; native scheduling needs CPU for unsupported operations.
3. For `auto/gpu`, scan a separate `gpu/vulkan/` or `gpu/metal/` directory using `init_backends_with(Some(dir), same_mask)`. Skip absent optional GPU directories in auto; treat corrupt/mismatched declared bundle contents as installation errors. For `cpu`, never scan/load the GPU module.
4. Finish all registration before enumeration, model loading, or concurrent use. Load with Auto, Cpu, or the platform's explicit GPU Backend; check the actual model device/kind before announcing readiness.

Source supports sequential distinct-directory loads: `S:src/transcribe.cpp:1187–1296` records canonical loaded directories and permits repeated identical masks. The mask is fixed at first registration; initializing CPU with CPU-only and subsequently widening it **fails**. Repeated loading of the same directory is idempotent and cannot retry a failed scan. **UNVERIFIED:** the proposed two-directory sequence in a Pumice GPU executable; give it a no-model integration test before inference tests. No new unsafe ggml FFI is needed.

P7 safety must survive: never let ggml score all ISA modules together; no AMX selection on stable Rust. MSVC builds produce nine x86 variants, not Linux's fourteen: x64, sse42, sandybridge, haswell, skylakex, cannonlake, cascadelake, icelake, alderlake (`S:ggml/src/CMakeLists.txt:487–522`). Audit MSVC's actual `/arch` implications against the detector, rather than assuming GCC flags map identically. Missing-file fallback already walks compatible variants; present-but-unloadable CPU modules currently fail, so do not advertise a broader repair policy.

## 3. Windows Vulkan build: concrete CI ingredients

Use `windows-2022` + stable `x86_64-pc-windows-msvc`, Visual Studio 2022 C/C++ tools/Windows SDK, and CMake (root requires ≥3.16; Vulkan's subproject ≥3.19; recommend a pinned CMake 3.x version already validated by CI). No GPU is needed to compile. Prefer the Visual Studio 17 2022 x64 generator initially; if using Ninja, explicitly import the x64 MSVC environment through `VsDevCmd.bat`/`vswhere` first, so ambient MinGW cannot win. Verify `cl`, CMake and SDK tool versions in the job.

Use **LunarG SDK 1.4.350.0 as the initial candidate**: the exact pinned engine revision uses it for its Windows Vulkan Cargo/deep-path build. I verified HTTP HEAD 200 for `https://sdk.lunarg.com/sdk/download/1.4.350.0/windows/vulkan_sdk.exe?Human=true` (324,012,984-byte installer); downloaded no payload. **UNVERIFIED:** successful installation/build in Pumice's GitHub Actions runner; SDK availability can change.

Action candidate (real action commit resolved during research):
```yaml
- uses: humbletim/install-vulkan-sdk@30ba978f977e81b72d091fc8888feb1fb26f9aff # v1.2
  with:
    version: '1.4.350.0'
    cache: true
```
It downloads/extracts the prebuilt SDK with 7z on Windows, sets `VULKAN_SDK` and adds its bin directory to PATH. It does not install a GPU driver. Pin action SHA and SDK version independently; record/check the official installer SHA-256 in the implementation (this action does not verify it itself). Keep its cache distinct by SDK/target. A short SDK path avoids its helper's unquoted-path limitations.

Do **not** substitute `humbletim/setup-vulkan-sdk`'s default Headers+Loader setup: the pinned ggml build also needs **glslc (shaderc)** and **SPIRV-Headers' CMake package**, including modern Vulkan-Hpp headers. glslangValidator is not a substitute for glslc. Full SDK extraction is simpler than building shaderc just for CI.

Preflight: `glslc --version`; verify SDK `Include/vulkan`, `Lib/vulkan-1.lib`, and that `find_package(SPIRV-Headers CONFIG REQUIRED)` succeeds with `VULKAN_SDK`/`CMAKE_PREFIX_PATH`. ggml calls `find_package(Vulkan COMPONENTS glslc REQUIRED)` and generates C++ arrays of SPIR-V at build time. Release users need neither shaderc/glslc nor SPIR-V headers/shader files.

Proposed Cargo feature `vulkan = ["cpu-variants", "transcribe-cpp/vulkan"]` (target guard Windows/Linux GNU); do not change upstream dependency defaults. Future `metal` can share an internal dynamic-backend capability without needlessly invoking the x86 chooser on arm64. A build after those features exist would be:
```powershell
$env:CARGO_TARGET_DIR = "$env:RUNNER_TEMP\pumice-vulkan"
$env:TRANSCRIBE_CMAKE_ARGS = '-DGGML_NATIVE=OFF -DTRANSCRIBE_USE_SYSTEM_BLAS=OFF -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded'
cargo build --release --locked -p pumice-transcription --features vulkan --target x86_64-pc-windows-msvc --message-format=json
```
Capture JSON output and extract this build's exact sys `out_dir`, as P7 already does. Windows runtime DLLs/modules are installed under `out/bin`, import libraries/manifests under `out/lib`; do not copy Linux's `out/lib/*.so` glob. Upstream already fixes MSVC `/O2 /Ob2 /DNDEBUG`, nested shader-generator MAX_PATH layout, and uses a short NTFS junction for Cargo's native build. Reuse those fixes; a short target path still helps rustc/link.exe.

Keep Rust `+crt-static` from `.cargo/config.toml`; force native Release CRT `MultiThreaded` (/MT), with CMP0091 NEW before project initialization. This is compatible in principle with shared application DLLs; static CRT does not require making every application library static. **UNVERIFIED:** the complete Pumice/MSVC `/MT` graph and cross-DLL allocation/free safety. Inspect build flags and `dumpbin /dependents` recursively for the executable and **every shipped DLL**, including CPU/GPU modules and Opus integration; reject vcruntime/msvcp/UCRT redistributable dependencies in application artifacts. Audit allocation ownership across module boundaries and exercise shutdown/repeated sessions. A CRT exception requires owner authorization; do not silently switch to /MD.

The shader-generator executable and glslc are build-host tools: their own CRT imports do not become end-user dependencies because they are not shipped. Ensure CMake's nested ExternalProject succeeds; it does not inherit all outer build settings automatically.

Runtime: users need a compatible vendor GPU driver/ICD **and** its Vulkan loader (`vulkan-1.dll`), not merely a copied DLL. The pinned backend checks Vulkan ≥1.2 and device capabilities; SDK version 1.4.350.0 is not a promise that every Vulkan 1.2 GPU works. No SDK, VS, Rust, CMake, CUDA toolkit, validation layers or developer runtime is an end-user prerequisite. Do not bundle random loader/driver DLLs. [LunarG runtime/SDK distinction](https://vulkan.lunarg.com/doc/view/1.4.309.0/windows/getting_started.html).

Sources: [exact upstream Windows recipe](https://github.com/handy-computer/transcribe.cpp/blob/3f32fbcc7bb3246851a0234263438bc3c0fa1cac/.github/workflows/rust-ci.yml#L270), [installer action](https://github.com/humbletim/install-vulkan-sdk/tree/30ba978f977e81b72d091fc8888feb1fb26f9aff), [setup action components](https://github.com/humbletim/setup-vulkan-sdk), [CMake CRT selection](https://cmake.org/cmake/help/latest/variable/CMAKE_MSVC_RUNTIME_LIBRARY.html).

## 4. Missing GPU and startup fallback

Source-verified mechanisms, **UNVERIFIED as packaged Pumice behavior**:
- No `vulkan-1.dll`/`libvulkan.so.1`: OS dynamic loading of the optional Vulkan module fails; ggml skips it. The executable/core must have no direct import of Vulkan, or fallback cannot run.
- Loader exists but has no ICD/device: Vulkan init failure is caught by `ggml_backend_vk_reg()` and returns null, or registers zero GPU devices. CPU is still registered because it loaded first.
- GPU device initialization fails normally: `src/transcribe-load-common.cpp::dev_init_checked/try_init_kind/init_backends` contains C++ exceptions; Auto tries discrete GPUs before integrated GPUs, then CPU. Explicit Vulkan/Metal fails if no matching initialized device exists.
- Auto is **not** a general recovery guarantee for GPU model-allocation/OOM after device initialization. Whisper allocates weights after backend selection (`src/arch/whisper/model.cpp:472–482`). Do not mask corrupt models/unknown load errors with a CPU retry. Initially return `model_load_failed`, with fixed guidance to select CPU; broader typed startup-OOM recovery can be a later proposal.

On successful CPU fallback report actual CPU device and variant, requested mode, and a safe reason such as `gpu_unavailable`; do not invent “driver missing” when the public API cannot distinguish missing loader, excluded backend, failed init, or zero devices. No GPU ready record before full model load and successful listener bind. If CPU modules/core libraries are absent, report an installation error; no automatic downloads or executing another archive.

“No driver/device must not crash” is an acceptance gate for normal absence, not proof against arbitrary driver defects. C++ exception guards cannot catch access violations, SIGSEGV, aborts, or hangs; pinned native sources contain fatal paths. Managed transcription already gives native process isolation. CPU mode's mask must keep GPU code entirely unloaded, providing a dependable escape hatch even with a broken installed driver.

## 5. macOS arm64 Metal, Linux Vulkan, and CUDA

**macOS arm64:** build on `macos-15` arm64 with selected Xcode/Apple clang/macOS SDK, CMake and normal Unix build tools. Set and verify a deliberate deployment target (candidate macOS 11.0 for Apple Silicon; **UNVERIFIED** with this engine and the selected current SDK). Add `transcribe-cpp/metal` explicitly. It sets `TRANSCRIBE_METAL=ON` and `GGML_METAL_EMBED_LIBRARY=ON`. Foundation, Metal, MetalKit and any Accelerate dependency are OS frameworks.

Important pinned-source detail: despite the option's name/comment, EMBED_LIBRARY embeds concatenated **Metal source text**, then uses `newLibraryWithSource` to compile it via the OS Metal runtime. It does not embed precompiled `.metallib` bytes (`ggml-metal/CMakeLists.txt`, `ggml-metal-device.m:448–490`). This needs no developer compiler installed by users and no `.metal` sidecars. Verify on a clean macOS install without Xcode; startup compilation latency is **UNVERIFIED**. Turning embedding off instead invokes build-time `xcrun metal/metallib` and needs Xcode Metal tools, plus packaging default.metallib and potentially ggml-tensor.metallib; defer that extra path.

Prefer the same loadable-module pack design, one portable arm64 CPU module plus Metal. Dynamic-backends does not turn on ARM all-variants, so do not port the x86 ISA chooser or invent Apple CPU tiers now. Native CMake MODULE naming is `.so` on Apple in this source; core libraries are `.dylib`. Verify actual install output and match ggml's `.so` scanner instead of renaming a backend to `.dylib`. Native installs use `@loader_path`; add executable-relative loader paths for the Pumice consumer and verify relocated bundles with `otool -L`. macOS Intel remains CPU-only regardless of feature unification in other jobs. Hosted macOS jobs are build/link/fallback evidence unless a real Metal device is confirmed; real-GPU correctness/performance stays a hardware check.

**Linux x64 Vulkan:** separate `x86_64-unknown-linux-gnu` bundle, never folded into musl. Build with GCC/clang, CMake, optional Ninja, `libvulkan-dev`, `glslc`, and `spirv-headers` providing its CMake config; upstream's native CI installs those exact dependency categories. Pin the builder/toolchain/SDK or distribution snapshot, not `ubuntu-latest` as an implied ABI contract. Candidate glibc floor 2.35 (Ubuntu 22.04 builder), **UNVERIFIED** until ELF symbol/dependency audit. If distribution glslc/headers are too old for the pinned shader sources, use the pinned LunarG SDK in the build job rather than silently dropping shader support.

Runtime requires matching glibc/C++ runtime closure and an OS Vulkan loader/ICD/driver; common Debian/Ubuntu packages are `libvulkan1` and `mesa-vulkan-drivers` for Intel/AMD, or the vendor driver for NVIDIA. The core/application must start without Vulkan installed because only the optional module needs it. Preserve `$ORIGIN` for the executable/core; nested modules may need `$ORIGIN/../..` or already-loaded core SONAME resolution. Audit every ELF after relocation without LD_LIBRARY_PATH; do not claim the executable rpath fixes every module's dependency chain. Users need no SDK/compiler; choose between bundled matching C++ runtime libraries and a documented standard runtime floor after a dependency audit.

**CUDA later:** `cuda` exists now and invokes CUDAToolkit/nvcc; `rocm` invokes HIP/ROCm. CUDA requires a compatible NVIDIA driver and potentially redistributed cuBLAS/cudart libraries (Windows cuBLAS is not made fully static by GGML_STATIC). Toolkit size, licenses, architecture list, CRT/dependency closure and benchmarking are separate work. Neither CUDA nor ROCm should enter universal packs.

## 6. Selection and reporting proposal

Recommend config `backend: cpu|auto|gpu` in `transcribe.yml`, with `--backend` on `file` and `serve` overriding it. Initially default to **cpu** to make enabling GPU explicit; the accelerated archive can document `backend: auto`. This default is an owner choice, not an existing decision. GPU mode means the pack's native GPU backend (Windows/Linux Vulkan; macOS arm64 Metal), requires GPU initialization, and never silently selects CPU. GPU scheduling may still use CPU for unsupported operations.

Do not offer only automatic selection: CPU is essential for diagnosis, reproducibility and broken drivers; GPU-required mode makes tests and performance measurements honest. No per-request backend changes, backend settings duplicated into orchestrator.yml, native module paths, or request-controlled environment variables. Config validation accepts syntax without probing drivers or loading models.

Defer a persistent numeric device index. Safe bindings already expose `devices()`, `Device.kind/device_type/device_id/index`, and `ModelOptions.device`; registry index is explicitly process-local. First implement `backends list --json` without a model, returning shipped/registered state honestly. If multi-GPU selection is needed now, prefer `--device-id` matching an exposed stable hardware ID with exact-device/no-fallback semantics; Metal may not supply one. A temporary `--device INDEX` must use that process's enumerated list, reject cpu+GPU-device conflicts, and document that indices are unstable. Never treat a Vulkan physical-device ordinal as the ggml registry index.

Keep `backend` as a human string for existing CLI output, but add structured metadata from `model.device().kind`/identity, not the requested flag or a CPU-module mapping. Proposed `compute` object:
```json
{"requested":"auto","backend":"cpu","device":{"name":"CPU","description":"Intel ...","id":null},"cpu_variant":"alderlake","fallback_reason":"gpu_unavailable"}
```
For Vulkan/Metal, report their actual kind/device; `cpu_variant` may describe the loaded CPU helper, but must not be appended to a GPU label as though it were the primary. Bound/sanitize hardware strings (e.g. 128 UTF-8 bytes per label, 256 for description), encode as JSON, and preserve managed records' <4096-byte cap including worst-case escaping. Test that cap with maximum-length model/instance IDs.

Use the same immutable selected-compute snapshot in `file --json`, standalone serve's startup output, managed stdout `ready`, managed `GET /ready`, and orchestrator `GET /pumice/v1/status`. Update parent `RawRecord`, `ReadyBody`, `ReadyInfo`, supervision/status storage and fakes together; recommend bumping internal protocol version to 2 on both sides because field sets are strict. Keep exact package-version checks. Public `/v1/audio/transcriptions` keeps its OpenAI response shape, without backend/device additions.

**Mid-run GPU failure:** return the current typed `engine_failed`/HTTP `transcription_failed`, never retry that audio on CPU or replay downstream work. For a known unusable native engine, withdraw readiness and retire the child; standalone exits, managed parent applies P13/D5's existing bounded restart policy. Add a safe internal backend-failed shutdown reason if necessary; do not pretend current `/health` detects a poisoned device. On the next controlled startup auto may select CPU; required-gpu remains an error. Native crash/hang is handled by existing process/liveness/deadline supervision. Cancellation/deadline is not automatically classified as GPU device loss. Typed device-loss classification is **UNVERIFIED**; the current Rust adapter collapses native failures, so avoid attempting live reinitialization in the same process.

## 7. CI without GPUs and manual gates

Required automated checks (future implementation; none run in this research):
- Build Windows Vulkan pack with MSVC/static-CRT audit, Linux Vulkan GNU pack, and macOS arm64 Metal pack in their respective PRs. Check imports recursively; Vulkan appears only on its optional module. Run extracted-package tests from a different cwd, a path with spaces/non-ASCII characters, and a read-only install. Ensure no build directory/SDK/PATH leak is required.
- Unit-test policy with injected device availability, CPU ISA/OS state, GPU-init failure, explicit-gpu refusal, actual metadata, mask fixed before registration, and no mid-run replay. Reuse fake Engine/managed-child fixtures for HTTP/errors/readiness/restarts.
- A no-model native test process loads chosen CPU then optional GPU with the same final mask and enumerates devices. Run each scenario in a fresh process because registry/mask/directory attempts are global. Test cpu mode with a deliberately invalid GPU module and external `GGML_BACKEND_PATH` to prove neither executes.
- Linux deterministic no-device case: set `VK_DRIVER_FILES` and legacy `VK_ICD_FILENAMES` to a nonexistent test manifest path, unset `GGML_VK_VISIBLE_DEVICES`, and assert CPU registration/selection; confirm loader version honors the override. `TRANSCRIBE_BACKENDS=cpu` alone proves exclusion, not genuine no-device fallback. A separate fresh container with runtime loader absent verifies missing-loader degradation.
- Windows CI may have a loader from the SDK/runner; removing SDK bin from PATH alone does not prove no-loader behavior. Use the no-ICD override for the no-device lane, and a clean driver-free VM or import-dependency fixture for missing-loader behavior. Never remove system DLLs. Owner clean-machine absence test remains required until an automated equivalent is reliable.
- One optional, pinned tiny-model smoke checks CPU fallback end-to-end, language/non-empty transcript, selected CPU metadata and managed readiness. Reuse current SHA-pinned tiny model/fixtures; normal correctness tests continue to use fakes. Run `cargo test --workspace --locked` on Linux and relevant feature tests/clippy when implementing; this research changes no code.

**Optional Linux software Vulkan:** Mesa lavapipe is practical as a backend-execution smoke, not a speed benchmark or proof of hardware support. CI may install `mesa-vulkan-drivers`, loader/tools and pin the Mesa version/image. Select only its ICD with `VK_DRIVER_FILES`/`VK_ICD_FILENAMES`; pinned ggml otherwise deliberately excludes CPU Vulkan devices. Set **`GGML_VK_VISIBLE_DEVICES=0`** for this one process to force the lavapipe device, then require reported backend `vulkan`; an Auto run that silently used CPU does not count. Time-limit a short tiny-model smoke. Shader/features and latency on the chosen Mesa version are **UNVERIFIED**. Without the forced selection, the same ICD is useful for proving “only software Vulkan → CPU fallback.” [Mesa identifies lavapipe as its Vulkan software frontend](https://docs.mesa3d.org/sourcetree.html).

**Windows owner checks, real GPU:** clean extraction without SDK/VS runtime setup; first CPU and required Vulkan runs; same pinned audio/model/language/prompt; compare transcript correctness, cold load and warm p50/p95, memory/VRAM, longer/shorter audio, silence and quantized Turbo; cancellation, shutdown/restart, unsupported/no driver/device, and explicit GPU failure. No guaranteed bit-identical GPU/CPU transcript without measurement. Owner performs all Windows GUI/driver actions. macOS Apple Silicon needs equivalent real-Metal correctness/clean-OS checks; Linux real GPU later validates its separate pack. Do not install anything on the owner's Windows side.

## 8. server-1 observations (read-only, no inference)

- `lscpu`: Intel Core i5-14500T, 20 logical CPUs.
- `/usr/lib/x86_64-linux-gnu`: `libvulkan.so.1 -> ...1.4.341`; Intel ANV/hasvk, lavapipe (`libvulkan_lvp.so`), Radeon, nouveau and other ICD libraries exist.
- `/usr/share/vulkan/icd.d/intel_icd.json`: `libvulkan_intel.so`, advertised API 1.4.335. `lvp_icd.json`: `libvulkan_lvp.so`, API 1.4.335.
- `/dev/dri/card1` and `renderD128` exist; current user can read/write renderD128 (ACL), whose sysfs driver is i915. User is not in video/render groups, but effective node access passes.
- Neither `vulkaninfo` nor `glslc` is installed/on PATH; `vulkaninfo --summary` was therefore not run. CMake is present. No package installed and no sudo used.

This is promising hardware/loader/ICD evidence, **not** confirmation of successful Vulkan instance creation, physical-device enumeration, required features, Turbo fit, correctness, or speed. All of those remain **UNVERIFIED**. Do not infer “UHD works” from shared-library filenames. No need to build the full engine to establish the observations requested here.

## 9. Archive layout, size, manifests, release changes

Proposed Windows archive `pumice-transcribe-VERSION-x86_64-pc-windows-msvc-vulkan.zip`:
```text
pumice-transcribe.exe
transcribe.dll, ggml.dll, ggml-base.dll
cpu/x64/ggml-cpu-x64.dll
cpu/<other-msvc-tier>/ggml-cpu-<tier>.dll
gpu/vulkan/ggml-vulkan.dll
bundle.json, LICENSE, THIRD_PARTY_NOTICES.md, INSTALL.md, transcribe.example.yml
```
Keep all runtime core DLLs beside the executable. Do not ship `.lib`, `.pdb`, CMake files, SDKs, shader generator, validation layers, or model weights. Harden Windows dependent-DLL search to trusted application paths plus OS System32; absolute module paths alone do not restrict their dependency search. Cover initial imported core DLLs as well as post-start `LoadLibraryW` (a call in Rust main cannot protect imports already resolved before main). **UNVERIFIED:** final hardening/linker mechanism; malicious cwd/PATH fixture and missing-core tests are release gates.

Linux `...-x86_64-unknown-linux-gnu-vulkan.tar.gz`: same tree with core `libtranscribe.so.0.3`, `libggml.so.0`, `libggml-base.so.0`, modules `libggml-cpu-<tier>.so` and `libggml-vulkan.so`. Preserve required SONAME symlinks or explicit copies; no driver/loader vendoring by default. macOS `...-aarch64-apple-darwin-metal.tar.gz`: `.dylib` cores, portable CPU `.so` module and `gpu/metal/libggml-metal.so`, embedded shader source. Actual native filenames/install IDs require platform artifact inspection.

Separate `...-TARGET-cpu` archives retain the `engine` portable executable and no GPU/variant modules. Preserve a static musl CPU asset on Linux; all GPU/variant Linux packs are GNU. P6 means these transcription archives do not exist today: “keep CPU-only archive” is a requirement on the new matrix, not a claim it is already released. Users copy the **whole** accelerated folder; install/upgrade an entire version atomically, keep models/config outside it. To use `pumice transcribe`/managed serve, core and transcription executable must be siblings under current dispatcher rules; document merging into one versioned suite directory without flattening `cpu/`/`gpu/`.

Reuse P7's exact Cargo out_dir selection, staged tree checks and isolated build directories in one shared pack helper, with small OS-specific library naming/loader adapters. Its current Linux stager duplicates versioned core symlinks as full files; preserve behavior first, optimize deliberately later. Extend existing packaging helpers only enough to package a validated native tree; never glob arbitrary target caches or accept incompatible loose packs.

`bundle.json`: product version, target, engine crate versions + upstream commit/ABI digest, native build features, CRT/libc/deployment floor, CPU variants/GPU modules, relative file paths/hashes/licenses and build SDK/toolchain identity. Validate bundle identity/required files before native module registration where possible; initial OS imports require packaging/loader guarantees too. Update native components together; no downloading a GPU pack at startup or on a request.

Size evidence: existing local P7 stage is **34 MiB** on disk; local portable release binary is **8.9 MiB** (`du`, pre-existing artifacts, build provenance not revalidated). Windows variants differ and `/MT` duplicates runtime code per DLL. Planning allowances **UNVERIFIED, not measured release sizes**: Windows/Linux accelerated bundle ~30–120 MiB uncompressed, Vulkan module increment ~10–80 MiB; Metal increment ~1–15 MiB; compression/debug stripping can change these substantially. Record real per-file/uncompressed/compressed sizes from first CI artifacts; set budgets from those measurements, not these estimates. No SDK (~hundreds of MB) or model (~GB) should enter archives.

`release.yml`: add a separate transcription-preview matrix/jobs; keep core musl/static checks and npm pack jobs independent. Build CPU+Windows Vulkan first; add other GPU platforms in their own PRs. Upload distinct artifact names, checksum all new archive files, validate bundle manifests and extraction/relocation. Current aggregate `pumice-*` pattern can also match `pumice-transcribe-*`: narrow artifact/npm inputs explicitly so transcription assets do not get consumed by the core npm staging logic. Release workflow is a preview, not a publishing pipeline; public delivery still follows owner release policy.

Docs: amend `docs/transcription.md` and `docs/install.md` to distinguish core single-file installation from whole transcription bundles; GPU driver/loader prerequisites and supported target floors; CPU escape hatch; explicit backend config and actual readiness reporting; no SDK/developer runtime; models separately downloaded explicitly; no automatic drivers/downloads. Include native/transcribe/ggml/Opus notices. Do not change the read-only product/spec docs.

## 10. Reviewable PR sequence and estimates

Changed-line estimates include tests/docs, **UNVERIFIED** until implementation scope is finalized:
1. **P14.1 Windows Vulkan bundle foundation** (~450–750 lines): add Windows Vulkan feature/build lane; reuse/generalize P7 chooser/stager for MSVC names + same-mask two-directory init; artifact manifest/dependency checks; no-model no-device/exclusion tests. Gate /MT and DLL search before expanding. Produce a reviewable CI artifact, not a public release.
2. **P14.2 Backend policy and truthful reporting** (~350–650 lines): config/CLI cpu|auto|gpu, safe initialization/load semantics, shared compute snapshot, coordinated managed protocol 2 + parent status/fakes, failure/no-replay tests. No runtime backend switching or auto benchmark. Finish Windows real-GPU owner checklist with its candidate artifact.
3. **P14.3 Windows transcription release preview and CPU archive** (~250–450 lines): native-tree archive packaging, preserved CPU-only variant, checksums/licenses/install docs, extraction/clean-machine validation and measured size report. Schedule public packages only with owner authorization per P6/D2. If first two PRs stay small, their tooling can share the same feature branch stack; keep packaging review separate.
4. **P14.4 macOS arm64 Metal** (~200–400 lines): same pack machinery, embedded sources, dylib/module loader paths, deployment audit, CPU counterpart and its own packaging/validation evidence. Intel CPU unchanged.
5. **P14.5 Linux GNU Vulkan** (~200–400 lines): same existing Linux CPU chooser, Vulkan module, declared ABI floor, ELF closure/relocation, no-loader/ICD tests, optional lavapipe smoke, independent CPU musl archive. Real Intel GPU test only when explicitly scheduled.

Do not make later platforms prerequisites for reviewing Windows. Each platform remains experimental until hardware evidence and dependency closure pass; no CUDA PR in this sequence.

## 11. Open questions for the owner/supervisor, with recommendations

1. **Reopen deferred transcription packaging now?** Recommend authorize Windows build/preview work first; keep public releases gated on owner real-GPU and clean-machine checks. P6/D2 remains in force until explicitly rescheduled.
2. **Default backend?** Recommend cpu initially, `auto` explicitly selected in transcribe.yml/CLI; `gpu` is required-GPU/no fallback. If installing the accelerated archive should itself authorize auto GPU selection, approve auto as that archive's documented default instead; choose once, do not vary secretly by detected driver.
3. **Device selection in first Windows increment?** Recommend automatic discrete-before-integrated selection plus truthful device reporting and model-free enumeration. Defer persistent indices until an actual multi-GPU need; stable ID selection is preferable when supported.
4. **Auto retry CPU on GPU model OOM?** Recommend no generic retry initially: current auto covers device initialization, not every model allocation failure. Fail before readiness with safe CPU guidance; add typed startup-only resource fallback later if demanded. Never retry an inference.
5. **How retire a GPU engine after a returned native error?** Recommend conservative process retirement for native engine failure (excluding cancellation/input errors), existing bounded P13 restart, and readiness withdrawal. Confirm the metadata-only shutdown reason/protocol alongside reporting. No same-process driver reset or hidden permanent CPU downgrade.
6. **Windows CRT exception if /MT fails?** Recommend none without evidence: attempt/inspect the full static-CRT native DLL graph first, fix ownership or upstream build integration. Bring a concrete failing artifact/audit to the owner before asking for a transcription-only exception.
7. **Platform floors and performance acceptance?** Recommend provisional Windows 10 x64/current vendor Vulkan driver, macOS arm64 ≥11, Linux glibc ≥2.35; all **UNVERIFIED** until build/runtime audits. Require correct transcripts and improvement over that same machine's best CPU variant on its actual dictation workload; no cross-machine speedup claim or hard VRAM promise.

No new owner decision was recorded; repository/task notes were left untouched.
