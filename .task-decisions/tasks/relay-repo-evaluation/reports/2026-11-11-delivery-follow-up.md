# Relay delivery follow-up (2026-11-11)

Implements the owner's cleanup and three-platform npm/npx goal after the initial evaluation. The initial report describes the baseline at `41ae758`; this report describes the resulting branch. No service, credentials, installed AI CLI, npm publication or release draft was changed. Only fake CLIs were used for tests.

## Implemented

- Removed the archived generic HTTP adapter, its fake server, adapter tests, four direct dependencies, HTTP-only errors and no-CLI discovery branches. The crates still needed transitively by axum remain. Retired configuration names still produce a helpful removal error. The owner explicitly superseded the archive decision.
- Removed the unused process cleanup scope and caller deadline from the deleted `pumice run` design, with their tests. Normal timeout, process-tree cleanup and temporary-directory behavior remain covered.
- Declared provider modules outside a macro so rustfmt sees them; formatted those files. Corrected stale installation text and removed redundant comments.
- Dropped Intel Mac from release, packaging, notices and smoke tests. The native targets are Windows x64, Linux x64 musl and macOS arm64. npm has one launcher and three native packages. AGENTS.md now names these architectures, as explicitly authorized.
- Fixed the Windows Claude npm shim: Claude's descriptor expects `@anthropic-ai/claude-code/bin/claude.exe`; the resolver launches it directly after checking the single quoted package target and file presence. JavaScript entrypoints still use Node. The regression failed with `NotInstalled` before the fix because the resolver required Node even for an executable; it now passes without Node. Altered targets, repeated targets and missing files are rejected. The fixture preserves the legacy JavaScript shape separately.
- Removed CI's duplicate release builds, size uploads and CRT check. Linux, Windows and macOS still run clippy and tests. Release preview retains three native builds, static-linkage checks, archive verification and npm/npx installs, plus Linux pnpm/Bun smoke tests. It still runs for `src/**` changes: retaining that coverage verifies shipped artifacts on every code PR and avoids weakening musl coverage while simplifying CI.
- Closed the 16 old deferred items for removed product modules. Cancelled the obsolete modular-product 0.3.0 publishing instructions; the draft itself is untouched.

## Relevance of remaining findings

| Finding | Still relevant? | Result |
| --- | --- | --- |
| Windows Claude npm shim | Yes, current provider resolution | Fixed with portable resolver regression; real Windows provider installation still unverified |
| Host/Origin checks | Yes, current relay router accepts cross-origin/plain-text requests and arbitrary Host headers | Keep as a security follow-up; guard design requires tunnel/client compatibility choices |
| 0.3.0 version and draft | Yes, draft archives describe the removed modular product while current Cargo version is still 0.3.0 | Cancelled old publishing instructions; did not delete or publish the draft or choose a new version |
| Duplicate release builds | Yes, CI and release preview built the same executable | Removed duplicate CI release steps; retained platform tests and complete release preview |
| Old transcription/GPU/module checks | No, the features are gone | Closed obsolete work; do not restore it |

## Distribution recommendation

The requested npm/npx route remains supported. Keep release archives as well: npm uses the same native builds and currently consumes the archives as packaging inputs. Removing the archive channel would not remove Windows, Linux or macOS compilation. No rewrite, channel removal or automated publishing is needed to meet the owner's stated goal.

Minimum maintained matrix: three native release builds and npm/npx install checks, Rust tests on all three operating systems, and packaging-helper tests on Linux. CI exercises native platform behavior; a Linux cross-check cannot establish Windows or macOS execution correctness.

## Validation

- `cargo test --locked`: 490 passed.
- `cargo test --locked --target x86_64-unknown-linux-musl`: 490 passed.
- `cargo fmt --check` and `cargo clippy --locked --all-targets -- -D warnings`: pass.
- `python3 -m unittest discover -s scripts -p 'test_*.py'`: 76 tests, one skipped, pass.
- Task-note shape check and `git diff --check`: pass.
- Cross-platform native builds, archive checks and npm/npx installs await the PR's CI; they are not claimed from local Linux testing.

## Questions for the owner

- Choose the relay release version and how to handle the existing unpublished modular-product `v0.3.0` draft/changelog. Do not publish the old draft as the relay release.
- Choose allowed hosts and browser origins for a future relay guard, including the public tunnel and any browser-based clients. This is still current relay work, not an obsolete module feature. No access policy or required configuration fields were changed here.

These questions do not prevent review of this cleanup PR. Publishing, deployment and access-policy changes remain outside this implementation.

## Evidence

See `src/process/resolve.rs` (native-shim regression), `src/providers/claude.rs` (declared target and installation hint), `.github/workflows/ci.yml`, `.github/workflows/release.yml`, and `scripts/test_npm_launcher.py` (Intel Mac rejection). The [official Claude setup guide](https://code.claude.com/docs/en/setup), checked 2026-11-11, recommends native installation. The initial report contains the baseline measurements and external sources.
