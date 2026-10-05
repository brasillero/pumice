# Release archives

The `Release builds` GitHub Actions workflow builds versioned archives for four targets. It runs on pull requests and main, and can also be run manually from the repository's **Actions** tab. It does not publish a GitHub Release or create a version tag.

| Platform | Rust target | Archive |
| --- | --- | --- |
| Windows x64 | `x86_64-pc-windows-msvc` | ZIP |
| Linux x64 | `x86_64-unknown-linux-musl` | tar.gz |
| macOS Apple Silicon | `aarch64-apple-darwin` | tar.gz |
| macOS Intel | `x86_64-apple-darwin` | tar.gz |

## Download and verify

Open a successful workflow run and download the combined `pumice-<version>-release` artifact. GitHub wraps Actions artifacts in a ZIP; extract that first to find the platform archives and `SHA256SUMS`. Downloading Actions artifacts requires a GitHub account.

Each platform archive contains one version/target directory with the Pumice executable, `LICENSE`, and `pumice.example.yaml`. Rust, Node, Python and other development tools are not needed to run the executable. The official AI CLI you select must still be installed and logged in on the same operating system and user account as Pumice.

On Linux or macOS, verify the matching archive against `SHA256SUMS` using `sha256sum` or `shasum -a 256`. On Windows, use `Get-FileHash -Algorithm SHA256` and compare the result with the matching manifest entry. Checksums detect an incomplete or altered download; they are not a signing or notarization guarantee.

Extract the archive for your operating system and architecture. From the extracted directory, run:

```sh
./pumice --version
./pumice check-config
./pumice serve
```

On Windows PowerShell, use `./pumice.exe` instead. Leave the terminal running while you use Handy. This archive does not register automatic startup or install a service.

The example YAML is a reference, not an automatically loaded configuration. Pumice uses built-in defaults unless you supply `--config <path>` or create its per-user file. Windows uses `%APPDATA%\pumice\pumice.yaml`; Linux and macOS use `$XDG_CONFIG_HOME/pumice/pumice.yaml`, or `~/.config/pumice/pumice.yaml` when `XDG_CONFIG_HOME` is unset. Run `pumice check-config --config <path>` before using an explicit YAML.

In Handy, use the Custom post-processing provider, base URL `http://127.0.0.1:7567/v1`, and model `claude`, `codex`, or `passthrough`. Passthrough returns the original transcript without an AI call.

Native Windows Pumice uses Windows CLI installations and logins. To keep using your WSL CLIs, run the Linux executable inside WSL and point Handy on Windows at the same loopback URL. Do not copy login credentials between systems.

## Build checks and limitations

Every target runs the fake-CLI test suite and a release build. The workflow verifies Windows static CRT linking and Linux static linkage. It extracts each archive and checks the packaged executable's version and configuration validation with an explicitly empty YAML; these smoke checks do not run inference.

The current package version comes from Cargo metadata and appears in the archive names. The workflow only packages the product executable, never the test CLI.

These preview artifacts are unsigned and macOS artifacts are not notarized. Windows or macOS may show a security prompt. Signing, installers, automatic startup, generated release notes and updates are later Phase 3 work. A successful archive workflow is evidence for S7.1, not completion of the distribution gate.

## Prepare release notes

The `Release notes` workflow generates preview artifacts from Conventional Commits with pinned git-cliff 2.14.2. Its full-history checkout and fixture tests verify grouping, compatibility-change visibility and preservation of previously released sections. It runs offline after downloading the build-time tool.

Manual workflow runs accept a version and date; blank inputs use Cargo’s version and the HEAD commit date in UTC. After publishing a release, advance Cargo to the next intended version before generating another preview; preparing an existing release is deliberately rejected.

The committed `CHANGELOG.md` contains an Unreleased snapshot. Cargo's current version is not a published release. The prepared artifact contains a proposed version section; preparing it does not create a Git tag, change Cargo or publish a GitHub Release.

For local preparation, use Python 3.12 and git-cliff 2.14.2 on the development machine:

```sh
python3 .github/scripts/prepare-release.py --version 0.1.0 --date 2026-10-04
```

Replace the example version/date with the intended release values. The version must exactly match Cargo.toml, be valid SemVer, and not already exist as a release/tag. Use a complete Git checkout. The generated files are `dist/release/CHANGELOG.md` and `dist/release/release-notes.md`; tracked files and tags remain unchanged.

The proposed version section and release-notes.md contain the same bytes. When publishing is separately approved, reuse release-notes.md for GitHub release notes and Tauri's updater `notes` field. Copy the generated full changelog into the release commit rather than manually editing historical release sections. Future version changes, signing keys and publication remain owner-controlled.
