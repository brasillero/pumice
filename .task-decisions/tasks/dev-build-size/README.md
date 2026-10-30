---
title: Smaller and faster local debug builds
state: Done
ticket: —
branch: dev-build-size
mr: PR #138
start: 2026-10-30
end: 2026-10-30
---

Debug and test builds keep line tables for the workspace's own code and drop debug info from dependencies (D1). A cold `cargo test --workspace --no-run` fills 2.2 GB of `target/` instead of 5.3 GB (−59%), and a rebuild after a one-line edit takes 2.0–2.4 s instead of 2.8–3.4 s. The default linker was already LLD. Release builds are unchanged. The CI Cargo cache key now covers the `[profile]` sections of `Cargo.toml`, so a profile change starts new caches instead of reusing builds made with the old settings. Measurements are in `reports/2026-10-30-measurements.md`.
