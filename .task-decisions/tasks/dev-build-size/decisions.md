# Decisions — dev-build-size

### D1 · Active · Debug builds keep line tables for workspace code and no debug info for dependencies
- date: 2026-10-30
- decided by: brasillero
- origin: conversation
- area: process
- revisit if: a debugging session needs full variable-level debug info in dev builds

**Decision.** `[profile.dev]` sets `debug = "line-tables-only"` and `[profile.dev.package."*"]` sets `debug = false`. Tests use the dev profile, so they inherit both. Release builds are unchanged.

**Why.** Local `target/` had grown to 54 GB, and full debug info was most of each build. Panic backtraces still show files and lines for Pumice's code, and builds are smaller and link faster (`reports/2026-10-30-measurements.md`).

**Rejected.** A different linker (`mold`, or setting `lld`): rustc 1.99 already links `x86_64-unknown-linux-gnu` with its bundled LLD, so there was nothing to gain without a system install. `lust-lang`: it cannot compile `async`, `unsafe` or proc-macro code, which Pumice needs. `debug = 0` for the workspace too: it would lose file and line numbers in backtraces for a few hundred megabytes.

**Consequences.** Stepping through dependency code in a debugger shows no variables; set `CARGO_PROFILE_DEV_DEBUG=true` for such a session. `target/` still grows over time, because Cargo keeps old builds; `cargo clean` resets it.
