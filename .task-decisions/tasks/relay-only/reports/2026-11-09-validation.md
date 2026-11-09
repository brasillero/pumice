# Relay-only implementation validation

The product change is in `938b88b`. The relay library, CLI, provider integrations, HTTP API, request monitor and fake-CLI tests moved to the root package. The shared initializer now creates only `pumice.yml`. Other module code, examples, native build actions, bundle coordination and audio fixtures were removed. Current documentation and the explicitly authorized AGENTS.md update describe the relay alone.

## Evidence

- `cargo test --locked`: 531 tests passed on Linux, including the added rejection cases for removed commands and audio/flow routes.
- `cargo clippy --locked --all-targets -- -D warnings`, `cargo fmt --check`, `cargo build --release --locked --bin pumice`: passed.
- `python3 -m unittest discover -s scripts -p 'test_*.py'`: 75 tests passed, with one platform-specific skip. Covers launcher forwarding, isolated package-manager installs, notices and archive/tarball validation.
- A real `npm pack` run on all five fixture packages passed exact membership, mode and SHA-256 audits.
- The release executable reports `pumice 0.3.0` and validates `pumice.example.yml`.
- Documentation links and `git diff --check`: passed.
- actionlint 1.7.7 passed with its outdated `macos-15-intel` runner-label warning suppressed. The existing runner label is retained; no other workflow errors were reported.

## Practical limits

Cross-platform builds, static-link audits and installed real-binary npm smoke tests run in the PR's CI. Local Rust is 1.99.0; release notices remain pinned to 1.99.1 and are generated with that exact toolchain in the release preview. No real AI CLI calls, live service changes or deletions of user data were performed. Historical task notes and the pre-existing untracked `spikes/` directory remain intact.

## Questions for the owner

None. The owner explicitly authorized the relay-only product and AGENTS.md alignment. Migration is documented: rename the old settings file to `pumice.yml` and start the service with `pumice` or `pumice --plain`.
