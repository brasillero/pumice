# Backlog — p14.1-windows-vulkan

## Active

## Deferred

### #2 · Deferred · Test the Windows GPU bundle on a machine with no Vulkan loader
- type: Task
- area: distribution
- origin: reports/2026-10-16-gpu-design.md
- ticket: —
- created: 2026-10-16
- reason: the CI runner gets `System32\vulkan-1.dll` from the SDK install; a clean machine is needed

**Context.** P14.1 proved the no-device case on CI (the Vulkan module loads, finds 0 devices, transcription runs on CPU), but not a machine without `vulkan-1.dll`, where the Vulkan module must fail to load and be skipped.

**To do.**
- [ ] Owner check on a Windows machine or VM without a Vulkan driver, or an automated equivalent.

## Closed

### #1 · Done · Restrict the Windows DLL search order of the GPU bundle
- type: Tech debt
- area: distribution
- origin: reports/2026-10-16-gpu-design.md
- ticket: —
- created: 2026-10-16
- closed: 2026-10-16
- outcome: PR #117 (`p14.1b-dll-search-order`): delay-loaded core, `SetDefaultDllDirectories(APPLICATION_DIR | SYSTEM32)` first in `main`, full-path core preload; decoy tests in cwd and PATH with negative controls

**Context.** Design §9: an absolute module path does not restrict where Windows looks for that module's own dependencies, and imports resolved before `main` cannot be protected from Rust code. Before any public Windows bundle, dependent-DLL lookup must be limited to the bundle and System32. Unverified: the mechanism (for example `/DEPENDENTLOADFLAG`, `SetDefaultDllDirectories` before loading modules, or a delay-loaded core).

**To do.**
- [x] Choose and implement the mechanism.
- [x] Test with a malicious DLL in the current directory and on PATH, and with a missing core DLL.

### #3 · Done · Replace P14.1's temporary GPU switches with the backend policy
- type: Tech debt
- area: transcription
- origin: D3
- ticket: —
- created: 2026-10-16
- closed: 2026-10-16
- outcome: PR #116 (`p14.2-backend-policy`): `--backend` and the model-free `backends` command replace the env var and `debug-backends`; device strings are trimmed

**Context.** P14.1 lets CI choose whether `gpu/vulkan/` is scanned with the environment variable `PUMICE_TRANSCRIBE_SKIP_GPU_DIR=1` (`crates/transcription/src/engine.rs`) and reports registration through a hidden `debug-backends` command (`crates/transcription/src/cli.rs`). Both are marked temporary. CPU description strings also carry trailing spaces from the native API.

**To do.**
- [x] Remove the environment variable; drive the GPU scan from `backend:` / `--backend`.
- [x] Replace or remove `debug-backends` (design §6 suggests a model-free `backends list --json`).
- [x] Trim device strings in the reported snapshot.
