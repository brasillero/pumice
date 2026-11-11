# Repository evaluation after relay-only (2026-11-11)

Proposals, not owner decisions. Evaluated `main` at `41ae758` (PR #140) on Linux. Nothing was changed outside this task folder: no code, workflow or documentation edits, no real AI call, no live service or user configuration touched, nothing installed.

## Summary

- **The relay itself is in good shape.** 531 tests pass (also on the shipped musl target), clippy and `cargo fmt --check` pass, and Pumice's own cost is negligible: 2 to 6 ms per request, 7 MB of memory, a 3.9 MB executable. There is no runtime performance work worth doing.
- **Two real problems.** Claude Code installed with npm cannot be started on Windows (finding 1), and the relay accepts requests from any web page (finding 2).
- **One release problem.** "0.3.0" currently names two different products (finding 3).
- **Recommended minimum matrix.** Ship three native targets (Windows x64, Linux x64 musl, macOS Apple Silicon), dropping Intel Mac. Test on the same three operating systems on every pull request, without the release build. Run the release preview only for packaging changes and before publishing. Keep both channels (GitHub archives and npm).
- **npx-only is not recommended.** It removes no build: the npm launcher still needs one native executable per platform. It would save about 480 lines of packaging code and one manual publishing step, and make Node.js 22 mandatory for every user while the official CLIs are moving away from Node.

## How this was checked

| What | Result |
| --- | --- |
| `cargo test --locked` (rustc 1.99.0, 20 cores) | 531 passed; 12 s to build, 9 s to run |
| `cargo test --locked --target x86_64-unknown-linux-musl` | 531 passed |
| `cargo clippy --locked --all-targets -- -D warnings`, `cargo fmt --check` | pass |
| `cargo build --release --locked --bin pumice` | 35 s; 3,887,376 bytes; 154 crates |
| `python3 -m unittest discover -s scripts -p 'test_*.py'` | pass (1 skipped), 3.8 s |
| Running relay with the test-only fake CLI as `claude`, on port 47567 | used for findings 2 and the runtime numbers |
| Installed CLIs, `--help` and `--version` only | Claude Code 2.1.322, Codex 0.167.1, Kiro 2.29.0 |
| GitHub runs | CI 42677334012 and Release preview 42677334069 (PR #140, `645a426`); CI 42682488559 (`main`) |

Runtime numbers come from the fake CLI, so they contain no AI call and no gateway hop.

## Confirmed problems

### 1. Claude Code installed with npm cannot be started on Windows (high, correctness)

**Impact.** On Windows, the documented main target, a user who installs Claude Code with npm gets `UnsupportedShim` for every `claude` request. Pumice itself prints that npm command as the install hint (`src/providers/claude.rs:56`).

**Evidence.** Pumice translates the npm `.cmd` shim only when it points at `@anthropic-ai/claude-code/cli.js` (`src/providers/claude.rs:46`, compared at `src/process/resolve.rs:170`). The published package 2.1.322 has no `cli.js`: its `bin` is `bin/claude.exe`, a placeholder that a postinstall step replaces with the native executable. I generated the shim with npm 12.2.0's own `cmd-shim` 9.0.2 for that layout; it calls `"%dp0%\node_modules\@anthropic-ai\claude-code\bin\claude.exe" %*`. Fed to `translate_windows_shim` in a scratch copy of the repository, it returned `UnsupportedShim`. The repository's fixture `tests/fixtures/windows-shims/claude.cmd` still holds the old `cli.js` shim, so the tests pass. Codex is unaffected: its `bin` is still `bin/codex.js`.

**Not verified.** No real Windows install was run. `s8.3-phase2-gate#1` (check a real Windows npm install) is still deferred.

**Workaround today, configuration only.** Set `binary:` to the full path of `claude.exe`, or install Claude Code with Anthropic's native installer, which puts `claude.exe` on PATH (Pumice resolves `.exe` files directly, `src/process/resolve.rs:240-253`; read, not run on Windows).

**Proposed action.** Accept an npm shim whose single quoted target is the expected `.exe` inside the provider's package and start that executable directly; replace the fixture with the current shim; change the install hint to the native installer, which Anthropic now recommends.

**Tradeoff.** The shim allow-list grows by one shape. It stays a fixed path inside the named package, never a shell.

### 2. The relay runs requests from any web page (medium, security)

**Impact.** While Pumice runs, a web page open in the user's browser can make it call the AI CLI. A plain cross-site request spends the user's subscription quota without reading the reply. With DNS rebinding the page also reads the reply, so it can use the subscription as its own. Tools are off, so nothing on the machine is read or changed.

**Evidence.** The router has no Host, Origin or Content-Type check (`src/api/mod.rs:179-187`); the body is parsed whatever its declared type (`src/api/handlers.rs:256-267`). Against the running relay:

| Request | Result |
| --- | --- |
| `Origin: https://evil.example`, `Sec-Fetch-Site: cross-site`, `Content-Type: text/plain` (a browser sends this without a CORS preflight) | 200 with the reply; the fake CLI ran |
| `Host: rebind.evil.example:47567` on `/v1/chat/completions` and `/v1/models` | 200 |

The removed `pumice serve` had exactly this guard (`p13.1-flow-seam/D8`); the git history shows none for the relay on 7567.

**Not verified.** No browser was used. Chrome 142 and later ask the user before a public page may reach loopback (Local Network Access); other browsers were not checked.

**Proposed action.** Reject a request whose `Host` is not `127.0.0.1:<port>` or an explicitly configured extra host, and reject any request that carries an `Origin` not explicitly configured. Handy's captured request has no `Origin` header (`docs/api.md:38-42`).

**Tradeoffs.** One or two new required settings in `pumice.yml`, so every existing file needs them. The owner's public tunnel must list its hostname, or it stops working. A browser-based client would need its origin listed; whether OpenWhispr sends `Origin` is unknown.

### 3. "0.3.0" names two different products (medium, release)

**Evidence.**

- `Cargo.toml:3` says `0.3.0`, and `CHANGELOG.md:21` has a dated `[0.3.0] — 2026-10-29` section describing the modular product, with the relay-only breaking changes above it under `[Unreleased]`.
- 0.3.0 was never published: there is no `v0.3.0` tag and npm `latest` is 0.2.0.
- A **draft** GitHub release `v0.3.0` exists, targeting `96de1ee` (PR #130), with four archives of the removed modular product.
- The task `release-0.3.0` is still `In progress`; its only item tells the owner to publish those artifacts and to check `pumice transcribe --version`.

**Impact.** Publishing the draft ships the removed product. Building 0.3.0 from `main` ships a different one under the same number.

**Proposed action.** Delete the draft release, pick the version for the relay-only release, settle the changelog (the 0.3.0 section was never released, so the "do not edit released sections" rule does not obviously cover it), and close `release-0.3.0#1`.

### 4. Every code pull request builds the release executable seven times (low, CI cost)

**Evidence.** `ci.yml:57-58` runs `cargo build --release` on all three systems (53 to 144 s each), then repeats the Windows CRT check (`ci.yml:69-87`, also at `release.yml:145-165`) and uploads the executables (`ci.yml:89-96`). `release.yml` runs on every PR that touches `src/**` (`release.yml:26`) and builds four more. PR #140's last code push produced 16 checks and about 23 runner-minutes (CI 490 s, Release preview 882 s of job time). The Release preview took 17 min of wall-clock time, including about 5 min waiting for the superseded run to finish cancelling and 5 min waiting for an Apple Silicon runner. Those two waits are inferred from job timestamps.

**Cost in money.** None: the repository is public, and standard GitHub-hosted runners are free there. The cost is waiting time and maintenance.

**Proposed action.** See the matrix below.

### 5. Smaller confirmed items

| Item | Evidence | Proposed action |
| --- | --- | --- |
| A documented command no longer exists | `docs/install.md:83` says `.\pumice.exe relay serve`; it exits 2 with "unexpected argument 'relay'" | Change to `.\pumice.exe` |
| `cargo fmt` does not see the provider modules | They are declared inside a macro (`src/providers/mod.rs:147-156`). Run directly, rustfmt reports unformatted code in `claude.rs` (4 places), `codex.rs` (12), `kiro.rs` (6) and `claude/models.rs` (2); `generic.rs`, declared normally, is clean | Declare the modules outside the macro, format once |
| Provider evidence is behind the installed CLIs | `docs/providers.md` covers Claude Code 2.1.310 and Codex 0.163.0; installed are 2.1.322 and 0.167.1. Every flag Pumice passes is still named in their help text; no real call was made | Re-check with a few real calls when convenient (needs the owner's approval for quota) |
| Open notes for removed features | 16 `Deferred` items belong to transcription, the formatter or the orchestrator (listed below the table) | Close as `Discarded`, citing `relay-only/D1` |
| Code kept for the removed `pumice run` | `CleanupScope` and `with_cleanup_scope` (`src/process/tree.rs:217`, `src/process/mod.rs:75`) and the caller deadline (`src/pipeline.rs:251`) have no caller outside tests | Remove with their tests |
| Workflow drift | `ci.yml` uses `actions/checkout@v5` and `ilammy/msvc-dev-cmd@v1`; `release.yml` uses `@v7` and explains why it avoids `msvc-dev-cmd` | Disappears with the CI change below |

The 16 items: `p4b-transcribe-config#2`, `p6-transcribe-http#2`, `p6-transcribe-http#3`, `p6-transcribe-http#4`, `p7-transcribe-cpu-variants#1`, `p8-transcribe-opus#1`, `p10-pumice-entry#2`, `p12-pumice-run#1`, `p12-pumice-run#2`, `p13.4-dictations#1`, `p14.3-npm-bundles#2`, `p14.4-macos-metal#2`, `p14.5-linux-vulkan#1`, `p15-serve-transcriptions#1`, `p15-serve-transcriptions#3`, `p15.3-test-page#2`.

## Platform, build, test and distribution matrix

| Layer | Today | Recommended minimum |
| --- | --- | --- |
| Native targets shipped | Windows x64, Linux x64 (musl), macOS Apple Silicon, macOS Intel | The first three; stop shipping macOS Intel |
| Tests on every PR | Linux, Windows, macOS: fmt, clippy, tests, release build, CRT check, upload | Same three systems: fmt, clippy, tests only; run the Linux tests on the musl target |
| Release preview | Every PR touching `src/**` or packaging: 12 jobs | Only PRs touching build or packaging inputs, plus the manual run before publishing: 10 jobs |
| Channels | GitHub archives and npm (launcher plus 4 native packages), published by hand | Both channels; launcher plus 3 native packages |

**Why three test systems stay.** Windows and Unix run different code: 12 `cfg(windows)` and 22 `cfg(unix)` attributes in `src`, concentrated in process trees, program resolution and handle inheritance. There is no macOS-only code, but the macOS job is the cheapest (109 s) and the only non-Linux Unix.

**Why macOS Intel goes.** Its build is the slowest job by far (307 s of compiling against 57 to 117 s for the others), so it sets the release preview's length: about 7.5 min of chained job time with it, about 4 min without. Rust demoted the target to "Tier 2 with host tools" in 1.90. GitHub's last Intel image, `macos-15-intel`, ends in August 2027 with no successor. Nothing shows a user: the 0.1.1 and 0.2.0 archives have zero downloads on every platform, and npm counts (300 to 415 per package in six weeks, peaking on publish days) look like registry mirrors. If Intel must stay, build it on the Apple Silicon runner instead of a dedicated one.

**Why the release build leaves CI.** `release.yml` builds the same executable with the pinned toolchain whenever shipped inputs change. Removing it saves at least 53 to 144 s per job. A code-only PR would then show 4 checks instead of 16, at an estimated 5 runner-minutes instead of 23.

**Risk of narrowing the release preview.** A source change that breaks only the musl build would surface at release time, not in the PR. Testing Linux on the musl target closes most of that gap, and it tests the libc that is shipped: today tests run on glibc and the musl executable only gets a `--version` check.

## npx-only: what it would and would not change

A distribution channel is how a file reaches the user. A native executable is what runs. Pumice has two channels (archives, npm) and one executable per platform; npm's launcher (`scripts/npm/bin/pumice.js`) only finds and starts that executable.

| Option | Removes native builds? | What it saves | What it costs |
| --- | --- | --- | --- |
| npm as the only channel, same Rust executables | No | `package_release.py`, its tests and `INSTALL.md` (353 lines), about 125 lines of archive checking in `package_npm.py:132-256`, the checksum job, and creating the GitHub release by hand | Node.js 22 becomes mandatory; contradicts AGENTS.md ("delivered by npm or a release archive") |
| One npm package holding every platform's executable | No | One publish instead of five; no optional dependencies, so no "missing native package" failure and no version-mismatch check | Everyone downloads 7.3 MB instead of 1.9 MB |
| Rewrite in JavaScript | Yes | All per-platform builds, linkage checks and the pinned toolchain | 13,161 lines of Rust and 16,265 of tests; Node cannot create Windows Job Objects, so process-tree cleanup gets weaker; contradicts AGENTS.md (Rust, one self-contained executable) |
| WebAssembly executable run by Node | Would, but not feasible | — | Pumice's job is starting CLIs, which WASI does not offer (from knowledge, not tested) |
| Build every target on one Linux runner | No | Fewer runner types | Extra cross-compilers; the executable can no longer be run on its own system in CI; no money saved on a public repository |

**Running through `npx` has measurable costs too.**

- A Node process stays resident beside Pumice: 48 MB against 6 to 7 MB (Linux, Node 24.21).
- `npx @brasillero/pumice@latest` asks the registry on every start (0.37 s warm, 3.2 s cold) and moves to a new version as soon as one is published. That is unsuitable for a service that is updated by hand.
- Anthropic's recommended Claude Code install is now a native executable, Kiro's always was, and this machine's Codex is a standalone build. An npx-only Pumice could be a user's only reason to install Node.js.

**Recommendation.** Keep both channels. The archives cost seconds in CI, are already the input of the npm packaging, and are the only install without Node.js. The savings worth having are in the pull-request pipeline, the Intel target and publishing, not in the channel count.

## Speculative opportunities

Not tested; each needs a small trial first.

| Opportunity | Why it might pay | Uncertainty |
| --- | --- | --- |
| Publish from CI with npm trusted publishing | Replaces downloading artifacts and five manual `pnpm publish` runs with web 2FA; adds provenance automatically | Needs a trusted publisher per package and a deliberate approval step so publishing stays the owner's |
| Replace the custom Cargo cache with `Swatinem/rust-cache` | 615 lines (`scripts/ci_cargo_cache.py`, its tests, two actions) exist because `cargo metadata --all-features` failed on the transcription crate; it now succeeds | The custom keys were tuned to survive weekly runner images; hit rates would need comparing |
| Delete the archived `generic` adapter | 2,019 lines with its tests and fake HTTP server; the only user of four direct dependencies | Archived by owner decision (2026-10-08, `s2.13-archive-providers#1`); the crates stay in the build through axum |
| Replace the launcher process on Unix | `process.execve` (Node 22.15+) would remove the resident 48 MB Node process | Experimental in Node, not available on Windows |

## Checked and not worth work

- **Request overhead.** 2 to 6 ms per request with the fake CLI (curl's own timer). The AI CLI's own start-up and model call dominate; keeping a CLI warm would break the fresh-directory rule.
- **Start-up.** 1 ms for the executable, 29 ms through the launcher. The three provider probes run concurrently; each CLI's `--version` takes 18 to 28 ms here.
- **Build time.** 35 s for a release build, 17 s of it the executable's own LTO and link. The live view (ratatui) has the largest dependency tree, but it is a documented feature.
- **Security basics.** IPv4 loopback bind only (`src/api/mod.rs:139-141`), 10 MiB and 5 s body limits, debug log created `0600`, CLI stdin instead of argv, tools switched off in each plugin.
- **Test layout.** 21 test executables; merging them would save link time, but the test step is already under 80 s in CI.

## Questions for the owner

1. **Windows shim (finding 1).** May the fix be implemented, and who checks it on a real Windows install?
2. **Web requests (finding 2).** Should the relay check `Host` and `Origin`, with explicit settings for extra hosts such as the public tunnel?
3. **Version (finding 3).** Delete the draft `v0.3.0` release? Which version is the relay-only release, and how should the unreleased 0.3.0 changelog section be treated?
4. **macOS Intel.** Stop shipping it, keep it on a dedicated runner, or build it on the Apple Silicon runner?
5. **Pull-request pipeline.** Remove the release build from CI and stop running the release preview for `src/**`-only changes?
6. **Channels.** Keep both (recommended), or go npm-only knowing it needs an AGENTS.md change?
7. **Notes.** Close the 16 items of removed features and `release-0.3.0#1`?
8. **Leftovers.** Remove the `pumice run` leftovers, and delete or keep the archived `generic` adapter?
9. **Publishing.** Is publishing from CI, with an approval step, acceptable?

## Sources

- GitHub runner images, Intel macOS: <https://github.com/actions/runner-images/issues/13045> (August 2027); <https://github.blog/changelog/2025-09-19-github-actions-macos-13-runner-image-is-closing-down/> (says fall 2027)
- Rust, `x86_64-apple-darwin` to Tier 2: <https://blog.rust-lang.org/2025/08/19/demoting-x86-64-apple-darwin-to-tier-2-with-host-tools/>
- GitHub Actions billing for public repositories: <https://docs.github.com/en/billing/concepts/product-billing/github-actions>
- Claude Code installation (native recommended; npm installs the native binary by postinstall): <https://code.claude.com/docs/en/setup>
- Chrome Local Network Access: <https://developer.chrome.com/blog/local-network-access>
- npm trusted publishing: <https://docs.npmjs.com/trusted-publishers>
- Node.js `process.execve`: <https://nodejs.org/docs/latest-v22.x/api/process.html>
- npm registry metadata for `@anthropic-ai/claude-code` 2.1.322, `@openai/codex` 0.167.1 and `@brasillero/pumice`, read on 2026-11-11
