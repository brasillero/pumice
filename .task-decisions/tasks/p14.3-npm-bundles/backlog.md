# Backlog — p14.3-npm-bundles

## Active

### #1 · To do · Third-party notices for everything the transcription bundle ships
- type: Task
- area: distribution
- origin: reports/2026-10-16-npm-bundle-design.md
- ticket: —
- created: 2026-10-16

**Context.** The packages only carry the formatter's notices. The transcription bundle adds transcribe.cpp/ggml (MIT), Opus (BSD-3-Clause), Symphonia (MPL-2.0) and the rest of the Rust dependency graph of `pumice-transcribe`; their license texts must ship before anything is published.

**To do.**
- [ ] Inventory the shipped dependency graph per platform and generate the combined notices file.

## Deferred

### #2 · Deferred · CPU-only transcription for Intel Macs
- type: Improvement
- area: distribution
- origin: D2
- ticket: —
- created: 2026-10-16
- reason: no CPU bundle build exists for macOS x86-64 yet

**Context.** `pumice-darwin-x64` keeps today's core-only payload: `pumice` works, `pumice transcribe` reports no engine.

**To do.**
- [ ] Add a macOS x86-64 CPU bundle build and ship it in `pumice-darwin-x64`.

### #3 · Deferred · Check why the Rust caches of the bundle jobs are almost empty
- type: Tech debt
- area: distribution
- origin: D4
- ticket: —
- created: 2026-10-17
- reason: builds work; only build time is at stake

**Context.** Recent `v0-rust-linux-vulkan-…` cache entries are about 2.6 KiB, against 304 MiB on 2026-10-16, so the bundle builds may be compiling from scratch. Separately, under D4 the release jobs no longer share caches with `transcription.yml`: the cache key hashes every installed toolchain, and the runner's preinstalled Rust differs from the pinned one.

**To do.**
- [ ] Find out what the bundle jobs cache today and restore useful caching (and, if worth it, remove the extra toolchains before the cache step).

## Closed
