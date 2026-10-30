# Native compiler cache outcome and SDK restore proof (2026-10-30)

## #4: discarded after measurement

The initial PR #136 trial installed ccache and set C/C++ compiler launchers through TRANSCRIBE_CMAKE_ARGS on Linux and macOS. Windows retained Visual Studio: CMake compiler launchers require Makefile/Ninja generators, so no generator, /O2 or static CRT change was attempted.

Cold Release run 40989214048: Linux job 123847338621 made 531 compiler calls, zero hits and 531 misses. Its 1,062 primary-storage misses are cache lookups, not 1,062 compilations. Metal job 123847338668 made 160 cacheable calls, zero hits and 160 misses. Compressed native entries were about 22 MiB Linux / 8 MiB Mac.

Warm Rust/native-install runs at 56751a5: Linux Release job 123857206433 and Transcription job 123857059029 (runs 40992098961 and 40992098958) both restored exact Cargo caches and reported zero compiler calls, zero hits and zero misses. Cargo already retained the native install. No populated compiler cache was saved. Linux compiler-cache setup added approximately 14 seconds in the cold trial before native compilation; it demonstrated no incremental compilation saving in the warm production jobs. The full trial workflows were superseded and are not claimed as complete successes; these are completed-job measurements.

The final implementation removes the compiler-cache installation, launchers, extra cache entries and statistics steps. The repaired Cargo cache continues to retain native installations. #4 is Discarded: no observed benefit for its added setup/cache maintenance. Reconsider only with a workload that actually recompiles unchanged C/C++ despite the repaired Cargo cache, or with evidence that target-cache eviction leaves a useful compiler cache behind. All bundle scripts return to their original portable baseline. No coverage removed.

## #7: installed SDK cache retained

Benchmark run 40987788551: Linux archive restore plus verify/unpack 26 s versus installed restore 6 s; Windows installer restore plus verify/install 35 s versus installed restore 11 s (plus 2 s file probe). Installed cache compressed sizes: Linux 505,196,271 bytes; Windows about 437,240,281 bytes. Cold misses still verify the pinned download SHA-256 before installation; restored SDKs retain required-file/tool preflight checks.

Windows restored-SDK build proof: run 40991092167, job 123853796385, at 62f6d79. The installed SDK key hit at 02:22:36 UTC; the log rendered `if ('true' -ne 'true')`, so Start-Process was not called. Cargo completed the optimized release build at 02:24:24; the DLL/static CRT audit passed at 02:24:33. The job succeeded. Windows generator, /O2 and static CRT settings were unchanged.

Linux restored-SDK build proof: run 40992098961, job 123857206433, restored the installed SDK in about 9.1 s and completed the build/staging/audit successfully. The paired Transcription job restored it in about 7.4 s and passed its additional checks. Final full workflows remain required at the final head; no incomplete workflow is represented as green here.

PR #135 was closed unmerged, its branch deleted, and benchmark entries 9734650628/9734667330 removed. Both #4 and #7 keep the original test/audit coverage.
