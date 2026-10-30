# Native compiler and SDK caches (2026-10-30)

PR #136 implements CMake ccache launchers on Linux and macOS, including the Metal job. Cache keys include compiler, CMake and ccache versions, Rust, backend, runner image, SDK pins, CMake settings and script hashes. Each backend stores at most 400 MiB before compression. Statistics are printed at job completion. Windows retains the Visual Studio generator, which does not implement CMake compiler launchers. No generator, optimization or static CRT change was made.

## Vulkan SDK experiment (#7)

Benchmark PR #135, run [40987788551](https://github.com/brasillero/pumice/actions/runs/40987788551), compared warm archive/installer restoration plus verification/install against an installed SDK cache:

| Platform | Current steps | Installed restore | Compressed current / installed |
| --- | ---: | ---: | ---: |
| Ubuntu 22.04 | 2 + 24 = 26 s | 6 s | 313,376,180 / 505,196,271 bytes |
| Windows 2022 | 6 + 29 = 35 s | 11 s (+2 s file check) | 304,812,970 / 437,240,281 bytes |

The 20–24 second warm saving justifies adopting installed SDK caching. Cold caches still download and verify the pinned SHA-256 before installing; warm caches retain the required-file and compiler preflight checks. Real bundle CI is required to verify restoration supports building, especially on Windows. Benchmark cache entries 9734650628 and 9734667330 were deleted, PR #135 closed unmerged, and its remote branch deleted. This removes 942,436,552 bytes of benchmark caches.

## Validation

Local cargo fmt, workspace clippy with warnings denied, workspace tests, and 99 Python helper tests passed (one optional package-manager test skipped). actionlint 1.7.12 passed; shellcheck is unavailable. Native bundle builds are validated on GitHub runners; cold/warm measurements remain pending until those runs complete. No coverage removed.
