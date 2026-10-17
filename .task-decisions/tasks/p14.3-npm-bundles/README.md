---
title: P14.3: npm platform packages carry the transcription folder
state: Done
ticket: —
branch: p14.3-npm-bundles
mr: PR #120
start: 2026-10-16
end: 2026-10-17
---

Pumice ships through npm as one CLI installed as a folder (`arch-modules/D16`). The Windows x64, Linux x64 and Apple Silicon platform packages carry `pumice` next to the complete `pumice-transcribe` bundle in `bin/` (about 25, 27 and 7 MB compressed; GPU module included, D3); Intel Macs stay core-only (D2); the Linux package declares `libc: ["glibc"]` and Alpine is dropped (D1). The packer validates each bundle against its `bundle.json`, a per-platform allowlist, file modes and (Linux) the ELF dependency closure, enforces a 50/150 MB budget and reads the packed tarballs back. Per-package `THIRD_PARTY_NOTICES.md` cover the shipped Rust crates, native components and the Rust standard library, with MPL source links (#1). Release preview builds the core and the three bundles at the same commit through shared composite actions, with one pinned Rust version matching the notices (D4), and installs the packed packages on Windows, Linux and both Macs from a loopback registry (`--version`, `backends`, a tiny CPU transcription, `npx`, pnpm and Bun). Release archives stay core-only (D5); publishing stays owner-operated. Implemented by Claude Opus 5.5 subagents; two Codex review rounds (round 1: MPL source, std notices, ELF closure; round 2: approve); design by Codex; final review by the orchestrator.
