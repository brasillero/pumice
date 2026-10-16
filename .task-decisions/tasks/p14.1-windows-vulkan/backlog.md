# Backlog — p14.1-windows-vulkan

## Active

## Deferred

### #1 · Deferred · Restrict the Windows DLL search order of the GPU bundle
- type: Tech debt
- area: distribution
- origin: reports/2026-10-16-gpu-design.md
- ticket: —
- created: 2026-10-16
- reason: release gate for P14.3; needs linker settings or new FFI, outside the P14.1 foundation

**Context.** Design §9: an absolute module path does not restrict where Windows looks for that module's own dependencies, and imports resolved before `main` cannot be protected from Rust code. Before any public Windows bundle, dependent-DLL lookup must be limited to the bundle and System32. Unverified: the mechanism (for example `/DEPENDENTLOADFLAG`, `SetDefaultDllDirectories` before loading modules, or a delay-loaded core).

**To do.**
- [ ] Choose and implement the mechanism.
- [ ] Test with a malicious DLL in the current directory and on PATH, and with a missing core DLL.

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
