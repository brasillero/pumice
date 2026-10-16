---
title: P14.4: macOS Apple Silicon Metal transcription bundle
state: Done
ticket: —
branch: p14.4-macos-metal
mr: PR #119
start: 2026-10-16
end: 2026-10-16
---

macOS Apple Silicon GPU transcription. The `metal` feature (a renamed `transcribe-cpp-sys` dependency scoped to Apple Silicon; not combinable with `vulkan`) builds the native Metal backend with its shader source embedded, so users need no Xcode. `build-macos-metal.sh` stages one portable arm64 CPU module (`cpu/arm64/libggml-cpu.so`) and `gpu/metal/libggml-metal.so` beside `.dylib` cores, with a required `bundle.json`, `@rpath`/`@loader_path` relocation, ad-hoc signatures and a Mach-O audit (arm64 only, minos 11.0, only `/usr/lib` and system frameworks). About 11 MB unpacked, 4.4 MB as a tarball. CI builds it on `macos-15` (Apple M1 Virtual) and tests registration, 16 damage cases, a tiny CPU smoke from a path with spaces, quarantine, and `pumice serve` with a Metal child. D1: on Apple Silicon `auto` stays on the CPU until a real Mac shows Metal is faster; D2: `auto` therefore never loads the Metal module (`gpu_auto_disabled`, saving 40 s of shader compilation on first start), while `gpu` uses Metal and `backends --json` still lists it. The runner's paravirtual GPU (family Apple5, no simdgroup support) runs `gpu` on Metal but produces garbage text; ggml's `test-backend-ops` there shows matmul, attention, softmax and norm unsupported and every supported Whisper op passing, so the runner is the suspected cause, recorded as informational CI diagnostics; the real-Mac check is #1. macOS Intel stays CPU-only. Implemented by Claude Opus 5.5 subagents; three Codex review rounds (round 1: D2 and the Metal diagnostics; round 2: non-fatal diagnostics, GELU_ERF, docs; round 3: approve); final review by the orchestrator.
