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

## Closed
