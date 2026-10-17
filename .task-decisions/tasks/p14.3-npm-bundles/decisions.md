# Decisions — p14.3-npm-bundles

### D1 · Active · The Linux x64 package targets glibc, and Alpine (musl) is not supported
- date: 2026-10-16
- decided by: claude (orchestrator, owner-delegated 2026-10-16)
- origin: reports/2026-10-16-npm-bundle-design.md
- area: distribution
- revisit if: someone needs Pumice on Alpine or another musl distribution

**Decision.** `@brasillero/pumice-linux-x64` declares `libc: ["glibc"]` and carries the GNU transcription bundle (glibc 2.35 or newer). No musl package is built; the bundled `pumice` core may stay static musl.

**Why.** The transcription bundle needs glibc; nobody but the owner uses Pumice yet, so a separate musl package and launcher libc detection would be work for no user.

**Rejected.** A `pumice-linux-x64-musl` package with a CPU-only musl transcription build; keeping the Linux package without a `libc` field (Alpine would install a bundle that cannot run).

### D2 · Active · The Intel Mac package keeps the core only
- date: 2026-10-16
- decided by: claude (orchestrator, owner-delegated 2026-10-16)
- origin: reports/2026-10-16-npm-bundle-design.md
- area: distribution
- revisit if: an Intel Mac user needs transcription (#2)

**Decision.** `@brasillero/pumice-darwin-x64` ships `pumice` alone, as in 0.2.0, until a macOS x86-64 CPU bundle exists (#2). The docs say so.

**Why.** No CPU bundle build exists for Intel Macs, and the GPU design keeps them CPU-only; building one is separate work.

**Rejected.** Blocking P14.3 on an Intel Mac build.

### D3 · Active · Each platform package carries its GPU module; there are no separate GPU packages
- date: 2026-10-16
- decided by: claude (orchestrator, owner-delegated 2026-10-16)
- origin: reports/2026-10-16-npm-bundle-design.md
- area: distribution
- revisit if: a package outgrows the npm registry's limits or the size budget

**Decision.** The Windows, Linux and Apple Silicon packages ship their full bundle, GPU module included (about 22–24 MB compressed on Windows and Linux, about 5 MB on Apple Silicon). `backend: cpu` stays the default, so the GPU module is only loaded when asked for. `pumice-relay` is not shipped in npm (`pumice relay` embeds it).

**Why.** One versioned package per platform avoids discovery and version-matching problems between a base and a GPU package; the measured sizes are acceptable.

**Rejected.** Separate `-gpu` packages; a download at install or first run.

### D4 · Active · Release builds use a pinned Rust toolchain that matches the pinned standard-library notices
- date: 2026-10-16
- decided by: claude (orchestrator, owner-delegated 2026-10-16)
- origin: conversation
- area: distribution
- revisit if: —

**Decision.** Every job in the release preview that builds a shipped binary (core and transcription bundles) installs one fixed Rust version, the same one the standard-library notices in `scripts/notices/` are pinned to (1.99.1 today). Moving to a newer Rust is a deliberate change that re-pins the notices in the same commit. Other workflows keep following stable.

**Why.** The notices generator refuses a toolchain it has no pinned texts for, so with `stable` every Rust release would break the release preview until someone re-pinned; a fixed release toolchain also makes release builds reproducible.

**Rejected.** Following stable and re-pinning after each failure; dropping the version guard (the notices could silently go stale).

### D5 · Active · Release archives stay core-only; the transcription folder ships through npm
- date: 2026-10-16
- decided by: claude (orchestrator, owner-delegated 2026-10-16)
- origin: conversation
- area: distribution
- revisit if: the owner wants a downloadable archive with transcription

**Decision.** The `.zip`/`.tar.gz` release archives keep only `pumice` and its notices. Transcription is installed through the npm platform packages (owner's preference, `arch-modules/D16`); the transcription preview artifacts of each CI run remain available for testing.

**Why.** One distribution channel for the bundle keeps release work small while nobody else uses Pumice; npm already carries it.

**Rejected.** Adding the bundle to the archives in this task.
