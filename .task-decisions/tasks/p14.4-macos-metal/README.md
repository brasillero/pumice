---
title: P14.4: macOS Apple Silicon Metal transcription bundle
state: In progress
ticket: —
branch: p14.4-macos-metal
mr: —
start: 2026-10-16
end: —
---

macOS arm64 GPU transcription. The `metal` feature builds the native Metal backend with its shader source embedded (no Xcode needed by users), staged as one portable arm64 CPU module plus a loadable `gpu/metal/` module with the same `bundle.json` manifest as Windows and Linux, relocatable through `@loader_path`/`@executable_path`, and audited so only OS frameworks and bundled libraries are needed. CI builds and tests it on a GitHub-hosted Apple Silicon runner, which has no usable GPU, so real-Metal correctness and speed stay a hardware check. macOS Intel stays CPU-only. D1: `auto` keeps the CPU on Apple Silicon until a real Mac shows Metal is faster.
