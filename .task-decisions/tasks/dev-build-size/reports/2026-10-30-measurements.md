# Debug build size and time, before and after D1

Measured 2026-10-30 on the Linux development machine (20 cores, 30 GB RAM), rustc 1.99.0, `cargo 1.99.0`. Before = `main` at `9491b82`; after = the same commit with the `Cargo.toml` change. Each run started from an empty `target/` (`cargo clean`), with the crates already downloaded. Two runs of each, alternating.

The command was `cargo test --workspace --no-run`: every library, binary and test program in debug mode. "Rebuild after an edit" appends a comment line to one file and runs the same command again.

| | Before, run 1 | Before, run 2 | After, run 1 | After, run 2 |
|---|---:|---:|---:|---:|
| Cold build | 22.9 s | 26.1 s | 18.7 s | 23.9 s |
| `target/` after the cold build | 5.3 GB | 5.3 GB | 2.2 GB | 2.2 GB |
| of which `debug/deps` | 4.0 GB | 4.0 GB | 1.4 GB | 1.4 GB |
| of which `debug/incremental` | 1.3 GB | 1.3 GB | 807 MB | 807 MB |
| Rebuild after an edit in `crates/relay/src/lib.rs` | 2.76 s | 3.33 s | 2.01 s | 2.36 s |
| Rebuild after an edit in `src/main.rs` | 2.75 s | 3.35 s | 2.05 s | 2.34 s |
| No-op rebuild | 0.35 s | 0.35 s | 0.28 s | 0.28 s |
| `target/` after the two edits | 5.7 GB | 5.7 GB | 2.6 GB | 2.6 GB |
| `target/debug/pumice` | 135 MB | — | 46 MB | — |

The sizes are the same in both runs; the times vary with machine load by a few seconds, so the cold-build gain is small and uncertain, while the rebuild gain (about 0.7–1.0 s, ~27%) held in both runs.

`readelf -p .comment target/debug/pumice` shows `Linker: LLD 23.1.1` before the change, so the default linker is already LLD.

Before the change, the developer's `target/` had grown to 54 GB (`cargo clean` removed 58.1 GiB in 132,218 files): Cargo keeps every old build, so one fresh build is not the whole story.

With the change, `cargo test --workspace --no-fail-fast` passed all 1,202 tests (48 `test result` lines: 42 test binaries plus doc-tests). A first full run without `--no-fail-fast` failed `download::tests::redirect_chain_cannot_extend_deadline` and `download::tests::swapped_part_during_download_is_detected_and_rejected`; both passed three of three runs in isolation with both settings and in the next full run. They assert wall-clock limits and are sensitive to machine load, not to debug info.
