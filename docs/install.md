# Installing and running Pumice

Pumice is a small local service that lets your dictation app use the AI
subscriptions you already have. It listens on `http://127.0.0.1:7567/v1`,
receives the prompt and transcript from [Handy](https://github.com/cjpais/Handy)
(or any app with an OpenAI-compatible post-processing endpoint), passes them
unchanged to the official CLI of your subscription, and returns the answer to
be pasted. The app's prompt decides what happens to the text.

> **Availability:** check [GitHub Releases](https://github.com/brasillero/pumice/releases)
> and the [npm registry](https://www.npmjs.com/package/@brasillero/pumice) for published
> versions. You can also build from source with Cargo (see the [README](../README.md)).

## What you need

- One of the supported platforms: **Windows x64, Linux x64, macOS x64 (Intel)
  or macOS ARM64 (Apple Silicon)**.
- The **official CLI of at least one AI provider**, installed and logged into
  in the *same* environment where Pumice runs — for example `claude`,
  `codex` or `kiro-cli` on your PATH. Pumice never installs or logs
  into these for you, and it never reads their credentials: it only invokes
  the CLI you already use.
- **Node.js 22 or newer**, only if you install through npm/pnpm/Bun. The npm
  package ships a tiny launcher, but the Pumice service itself is a
  standalone native executable with nothing else to install. On Linux, the
  npm install also needs glibc 2.35 or newer (see below).

## Install

### Option 1: portable archive (no runtime needed)

Download the archive for your platform from
[GitHub Releases](https://github.com/brasillero/pumice/releases) and extract
it anywhere. The executable inside is self-contained.

The archives carry `pumice` only, without the transcription folder, so
`pumice transcribe` reports that `pumice-transcribe` is not installed. For
transcription, install through npm (Option 2): the Windows x64, Linux x64
and Apple Silicon platform packages carry the complete transcription
folder.

### Option 2: package manager (Node.js 22+)

The package is published on npm as `@brasillero/pumice`; the installed command is `pumice`.

```sh
npm install -g @brasillero/pumice
pnpm add -g @brasillero/pumice
bun add -g @brasillero/pumice
npx @brasillero/pumice@latest --version   # run without installing
```

Your package manager fetches the `@brasillero/pumice` package and the one
platform package matching your system (an optional dependency; the others are
skipped). Bun works through the same package, with Node.js installed for the
launcher. Nothing is downloaded at install or run time beyond those two
packages, and no install scripts run.

What each platform package holds in its `bin/` folder (keep the folder
together; `pumice transcribe` only runs the `pumice-transcribe` next to
`pumice`):

| Platform package | `pumice` | Transcription (`pumice transcribe`) | Size (packed / unpacked) |
| --- | --- | --- | --- |
| `@brasillero/pumice-win32-x64` | yes | `pumice-transcribe.exe`, its engine DLLs, 9 CPU variants, the Vulkan GPU module | about 25 / 71 MB |
| `@brasillero/pumice-linux-x64` | yes (static) | `pumice-transcribe`, its engine libraries, 14 CPU variants, the Vulkan GPU module | about 27 / 77 MB |
| `@brasillero/pumice-darwin-arm64` | yes | `pumice-transcribe`, its engine libraries, the arm64 CPU module, the Metal GPU module | about 7 / 16 MB |
| `@brasillero/pumice-darwin-x64` (Intel Mac) | yes | not included yet: `pumice transcribe` reports that `pumice-transcribe` is not installed | about 3 / 6 MB |

Each package's `bundle.json` lists every transcription file with its size and
SHA-256, and `pumice-transcribe` checks them at startup. Each package also
carries `THIRD_PARTY_NOTICES.md` with the licenses of everything it ships.

- **Linux needs glibc 2.35 or newer** (Ubuntu 22.04, Debian 12, Fedora 36 or
  later). The Linux package declares `libc: glibc`, so on musl-based
  distributions such as **Alpine, which is not supported**, the package
  manager skips it and `pumice` says the platform package is missing. Use
  the portable archive there (it has `pumice` only).
- **Transcription reads a complete settings file.** Its first command that
  reads settings creates missing [per-user settings](transcription.md#configuration)
  from the supplied examples. Edit `transcribe.yml` there, or pass an
  existing file with `--config`. Even `file --model-file PATH` reads it;
  creating settings never downloads models.
  The example chooses `backend: cpu`. The GPU modules are only
  loaded with `backend: gpu` (or `auto` where it applies); they need a GPU
  driver with Vulkan (Windows, Linux) or an Apple Silicon Mac. See
  [Compute backend](transcription.md#compute-backend-cpu-auto-gpu) and the
  [Windows](transcription.md#windows-vulkan-preview-experimental),
  [Linux](transcription.md#linux-vulkan-preview-experimental) and
  [macOS](transcription.md#macos-metal-preview-experimental) sections for the
  prerequisites. Models are not included: see
  [Getting models](transcription.md#getting-models).
- **macOS:** npm, pnpm and Bun download and unpack the packages themselves,
  so the files carry no `com.apple.quarantine` attribute and Gatekeeper does
  not block them (checked in CI with npm; `xattr -lr` shows the attribute if
  some other tool added it, and `xattr -dr com.apple.quarantine <folder>`
  removes it). The binaries are ad-hoc signed, not notarized.

## Run the service

Once `pumice` is on your PATH:

```sh
pumice relay serve
```

That command starts the relay service and prints the address it listens on,
`http://127.0.0.1:7567/v1`. (`pumice relay` with no further arguments is the
same command, and `pumice relay --config <path>` starts it with an explicit
config file.) Stop the service with Ctrl-C.

When `pumice` runs in a terminal, it opens a live interactive view of every
completion request: arrived, queued, CLI started, CLI ended, responded or
dropped, with timing and provider/model metadata. Use `↑`/`↓` or `j`/`k` to
select a request, `PgUp`/`PgDn` to page through the table, `Enter` to open its
full-screen details, `f` to cycle filters, `/` to search, `Esc` to clear the
filter/search, and `q` or `Ctrl-C` to quit (requests already running finish
first; press it again to quit at once). Press `?` (except while typing a
search) to open a help screen listing every key; `?`, `Esc` or `q` close it.
The bottom line always shows the keys of the current screen.

In full-screen details, the top shows the request number and four tabs:
**Summary**, **Received**, **Parsed** and **Sent**. Use `Tab`/`Shift-Tab` or
`1`–`4` to switch tabs, `↑`/`↓` or `j`/`k` to scroll, `PgUp`/`PgDn` or
`Space` to page, `Home`/`End` or `g`/`G` to jump to the top/bottom, and
`←`/`→` or `h`/`l` to move to the previous/next request while keeping the
current tab. `Esc`, `Enter` or `q` return to the table (`q` does not quit
here). The mouse wheel scrolls too in terminals that turn it into arrow keys,
such as Windows Terminal; Pumice does not capture the mouse, so you can still
select and copy text.

- **Summary**: the request timeline, any diagnostic and, when allowed, the
  input and reply.
- **Received**: the request headers (credential headers such as
  `Authorization` or `Cookie` are masked as `[hidden]`) and the body as Pumice
  received it, JSON indented with its keys in their original order.
- **Parsed**: the model, system prompts, and the parts of the user message
  before, during and after the dictated input.
- **Sent**: the HTTP response Pumice sent back: status, headers and body.

Each text the view keeps (a body, a prompt, the input, a reply) is cut at
64 KiB, marked with its full size, so a stream of large requests cannot fill
the memory.

Dictated text, request bodies, response bodies and headers only appear when
`debug_log.enabled` is `true`; otherwise those tabs show only metadata and the
message "Text hidden: debug_log is off." The `app` column in the table shows
the value of the client's `X-Title` header (for example `Handy`) when the
request names one; otherwise it shows `-`. Run with `--plain` to keep the old
one-line-per-request output on stderr instead of the live view.

If the executable is not on your PATH — for example right after extracting an
archive — run it from its folder with an explicit path:

```sh
./pumice relay serve        # Linux and macOS
.\pumice.exe relay serve    # Windows PowerShell
```

## Check that it works

```sh
pumice relay doctor         # shows which provider CLIs were found; spends no quota
pumice relay --version
```

`doctor` only probes for installed CLIs, so it is free to run as often as you
like. To verify that a provider is actually logged in and answers,
run `pumice relay doctor --login-check --provider claude` — that one call spends a
small amount of your plan quota, which is why it only runs when you ask for
it.

## Point Handy at it

In Handy: **Settings → Advanced → Experimental Features → Post Processing**,
then:

- Provider: **Custom**
- Base URL: `http://127.0.0.1:7567/v1`
- API key: leave **empty**
- Model: `claude`, `codex` or `kiro` (the CLIs you installed
  above), `passthrough` to get the raw transcript back exactly as dictated,
  or `inspect` to echo the full JSON request body for diagnostics. `inspect` includes Handy's
  attached prompt and pastes the result as JSON; neither `passthrough` nor
  `inspect` spends quota or calls an AI.

No Handy changes are needed beyond this Custom provider configuration. The
exact request Handy sends is described in
[api.md](api.md#captured-handy-contract).

### Handy on Windows, Pumice in WSL

Handy on Windows reaches a Pumice running inside WSL 2 with WSL's default
NAT networking, with nothing installed on Windows and no setting changed
(tested on 2026-10-03 and 2026-10-04 with Windows 11 build 26200 and WSL
2.7.14.0, `wslinfo --networking-mode` reporting `nat`):

- **Use `http://127.0.0.1:7567/v1` in Handy, not `localhost`.** WSL forwards
  only IPv4 loopback: `http://[::1]:…` failed. Windows tries `::1` first for
  `localhost`, so the first request took about 2.1 s before falling back to
  IPv4; with `127.0.0.1` every request took about 14 ms.
- **Pumice stays bound to `127.0.0.1`.** Forwarded requests arrive inside
  WSL as loopback connections (`127.0.0.1:<port>`), so there is no need to
  bind `0.0.0.0` or open a firewall port.
- **WSL must be running.** The forwarding only exists while the WSL VM is
  up, and after a Windows restart Pumice has to be started again inside WSL.
  Starting it automatically at Windows login was not pursued (owner
  decision, 2026-10-04).
- **Mirrored networking** (`networkingMode=mirrored`) was not tested.

## Configuration

The relay needs a config file, and every setting in it must be written:
an omitted one stops the relay with an error that names the file and line.
The first command that needs settings creates missing per-user files from
the supplied examples, including [`relay.example.yml`](../relay.example.yml).
Existing files are never overwritten or filled in. Edit the relay file at:

- Windows: `%APPDATA%\pumice\relay.yml`
- Linux and macOS: `~/.config/pumice/relay.yml`
  (or `$XDG_CONFIG_HOME/pumice/relay.yml`)

```yaml
port: 7567
total_timeout_secs: 30
max_parallel: 4
providers:                 # the client's model picks the entry; there is no
  - id: claude             # fallback: a failure, or no model, is an error
    enabled: true          # and the app keeps its own transcript
    model: haiku
    binary: claude         # a bare name is looked up on PATH
    timeout_secs: 30
    effort: low            # low, medium or high
    thinking: false        # Claude only
  - id: codex
    enabled: false         # kept in the file, not offered, never run
    model: null            # null only on a disabled entry
    binary: codex
    timeout_secs: 30
    effort: low            # written on disabled entries too
debug_log:
  enabled: false
  path: pumice-debug.jsonl
```

The client picks the provider: Handy sends the `model` the user chose, and
Pumice runs exactly that provider. A request without a model, or any
failure, is answered with an HTTP error, and the app keeps its own
transcript. Every provider entry writes `id`, `enabled`, `model`, `binary`,
`timeout_secs` and `effort`, and the `claude` entry also writes `thinking`.
With no provider enabled every AI request gets an
error and the app keeps its own transcript (the built-in `passthrough` and
`inspect` still work). Gateways, routing and logins are configured in each
CLI itself; Pumice inherits them. Validate the file with
`pumice relay check-config`; `pumice relay doctor` also runs before the
file exists. The config lives outside the install folder, so upgrading
never replaces it. Automatic creation writes all four module files and the
sample formatter definition. An explicit `--config` path must already exist;
help, version and optional diagnostic commands do not create files.

`effort` is the reasoning effort of every call the provider makes: `low`,
`medium` or `high`, mapped to the CLI's own setting (Claude and Kiro
`--effort`, Codex `model_reasoning_effort`). Higher effort can follow the
app's prompt more closely, but answers take longer and use more of your
plan. A model the CLI is known to run without effort (Claude Code's
`claude-haiku-4-5`, `claude-sonnet-4-5`, `claude-opus-4-1` and older, Kiro's
`claude-haiku-4.5`) is refused at startup with the file and line, because it
would ignore every level; an alias such as `haiku`, or a model Pumice knows
nothing about, is accepted. Claude's `thinking: false` turns thinking off
where the model allows it; `thinking: true` lets Claude Code think the way
it does by default for that model. Every other CLI setting stays the
cheapest one, and fast or priority modes stay off. The other providers have
no `thinking` key: they reason through effort alone.

`max_parallel` (1 to 32) sets how many requests may run their provider at
the same time. Extra requests wait in line, first come first served; a
request still waiting when its `total_timeout_secs` budget runs out gets
HTTP 503.

Which providers are available today:

Nothing is on until you list it with `enabled: true` and a `model` (the
example config shows how). Then set the model in Handy to the provider's
id to pick it.

- **Claude**, **Codex** and **Kiro** — supported and tested with real calls.
  How each CLI is called, and what each one keeps or cannot switch off, is
  in [providers.md](providers.md). Two Kiro limits to know: `kiro-cli`
  saves every run, dictated text included, in your own `~/.kiro/sessions`,
  and Kiro's built-in instructions stay under the app's prompt, so with some
  models it answers a dictation instead of formatting it
  ([Kiro](providers.md#kiro)).
- The **local-model adapter** (Ollama, LM Studio) — archived for now;
  listing it in the config is an error.
- **Kimi**, **Antigravity** and **OpenCode** — support was removed; listing
  them in the config is an error.

## Upgrading from 0.2

The 0.2 commands `pumice` and `pumice --plain` no longer start the
service: bare `pumice` now prints the command overview and exits, and the
service starts with `pumice relay serve` (or `pumice relay serve --plain`).
The old `pumice.yaml` configuration file is no longer read, with no
fallback: rename it to `relay.yml` in the same directory
(`~/.config/pumice/relay.yml` on Linux and macOS,
`%APPDATA%\pumice\relay.yml` on Windows), then add every setting it leaves
out, since none has a built-in default anymore (see
[Configuration](#configuration); `pumice relay check-config` names each
missing one).
Handy's base URL and port 7567 are exactly the same, so no Handy changes
are needed. The standalone `pumice-relay` binary keeps its old arguments
if you run it directly.

### Updating a systemd user service (server-1)

When Pumice runs as a systemd user service, rename the file and update the
unit before restarting:

1. `mv ~/.config/pumice/pumice.yaml ~/.config/pumice/relay.yml`
2. In `~/.config/systemd/user/pumice.service`, change `ExecStart=` from
   `pumice --plain` to the installed binary path followed by
   `relay serve --plain` (for example
   `ExecStart=/home/you/.cargo/bin/pumice relay serve --plain`), keeping the
   unit's existing user and environment. If the unit passes an explicit
   `--config .../pumice.yaml`, point that path at `relay.yml` instead.
3. Reload and restart the service:

   ```bash
   systemctl --user daemon-reload && systemctl --user restart pumice
   ```

Handy's URL and port 7567 are unchanged.

## Upgrade and remove

Stop the service with Ctrl-C before updating or removing it. Choose the
command for your package manager:

| Package manager | Upgrade | Remove |
| --- | --- | --- |
| npm | `npm update -g @brasillero/pumice` | `npm uninstall -g @brasillero/pumice` |
| pnpm | `pnpm update -g @brasillero/pumice` | `pnpm remove -g @brasillero/pumice` |
| Bun | `bun update -g @brasillero/pumice` | `bun remove -g @brasillero/pumice` |

For archive installs, extract the newer archive into a fresh folder and
replace the old executable. Your per-user config is kept. Restart with
`pumice relay serve` after updating. To remove an archive install, delete its folder and
remove that directory from your PATH if you added it.

## Privacy

Pumice binds to `127.0.0.1` only, sends no telemetry, has no auto-updater and
makes no external network calls itself; only the AI CLIs contact their
providers. It never loses a dictation: if the selected provider fails or
times out, Pumice answers with an error and the app pastes its own
transcript.

## Building and publishing releases (maintainers)

This section is for whoever cuts a release; users need none of it. Nothing
publishes automatically: the **Release preview** workflow
([`.github/workflows/release.yml`](../.github/workflows/release.yml)) runs on
pull requests that touch release-related paths and on manual dispatch, with
`permissions: contents: read` and no secrets, and only builds and checks
artifacts. GitHub releases and npm publishing are done by hand from one
preview run.

### What a preview run builds

| Target | Runner | Archive | npm platform package |
| --- | --- | --- | --- |
| `x86_64-unknown-linux-musl` | `ubuntu-latest` | `.tar.gz` | `linux-x64` |
| `x86_64-pc-windows-msvc` | `windows-latest` | `.zip` | `win32-x64` |
| `x86_64-apple-darwin` | `macos-15-intel` | `.tar.gz` | `darwin-x64` |
| `aarch64-apple-darwin` | `macos-15` | `.tar.gz` | `darwin-arm64` |

- **Version.** `scripts/package_release.py --print-version` reads
  `[package].version` from the workspace `Cargo.toml` (Python's `tomllib`,
  so Python 3.11 or newer; the workflow pins 3.13). The raw and the packaged
  `pumice` must print `pumice <version>`, and `pumice-relay` must print
  `pumice-relay <version>`.
- **Linkage checks.** The Linux musl binary must have no dynamic interpreter
  and no `NEEDED` library (`readelf`); the Windows `pumice.exe`, linked with
  `+crt-static` (`.cargo/config.toml`), must not depend on `vcruntime`,
  `msvcp`, `ucrtbase` or `api-ms-win-crt` (`dumpbin /dependents`, found
  through `vswhere`). macOS builds run on native runners of each
  architecture and use only OS libraries.
- **Archives.** `scripts/package_release.py` (stdlib only) packages exactly
  these files under one top-level folder `pumice-<version>-<target>/`:
  `pumice` (`pumice.exe`), `LICENSE`, `THIRD_PARTY_NOTICES.md` (the core-only
  notices from `scripts/third_party_notices.py --core-only`),
  `relay.example.yml` and `INSTALL.md` (from `scripts/INSTALL.md`). It never
  walks the repository or `target/` and keeps the executable mode on Unix.
  The archives carry `pumice` alone, without transcription
  (`p14.3-npm-bundles/D5`). The `checksums` job writes `SHA256SUMS`
  (basenames only) and uploads the four archives with it as the
  `release-preview` artifact (kept 7 days).
- **Transcription bundles.** At the same commit, the composite actions in
  `.github/actions/` (`windows-vulkan-bundle`, `linux-vulkan-bundle`,
  `macos-metal-bundle`, shared with the transcription workflow) build the
  Windows x64 Vulkan, Linux x64 Vulkan and Apple Silicon Metal transcription
  folders. No artifact from another run is used. Intel Macs get no bundle.
- **npm packages.** `scripts/package_npm.py` (stdlib only) stages the five
  packages and packs each with `npm pack --ignore-scripts`, then uploads
  them as the `npm-preview` artifact with `PACKAGES.md` (sizes). Before
  staging it checks that the input holds exactly the four release archives
  and their `SHA256SUMS` (strict parsing: malformed or duplicate lines fail),
  and it extracts only the expected binary from each archive (symlinks, hard
  links, directories and traversal names fail; no blind `extractall`), so
  the npm binary is byte for byte the archive's.
- **Bundle validation.** Each transcription folder must hold only regular
  files and directories (npm drops symlinks), with safe names, every path on
  a per-platform allowlist and listed in its `bundle.json`, nothing
  unlisted, the identity (version, target, feature) and every size and
  SHA-256 matching, the three core libraries, one GPU module and the
  baseline CPU module present, and POSIX modes as staged (0755 directories
  and executables; Linux libraries 0644, Mach-O libraries 0755). On Linux,
  core libraries must sit under their ELF SONAME, and every `DT_NEEDED` of
  every shipped ELF file must be a shipped core library or on the reviewed
  OS-library allowlist (`libvulkan.so.1` for the GPU module only). After
  packing, each tarball is read back and must hold exactly the staged files
  with their modes and hashes (npm 12.2 keeps on-disk modes, so the packer
  sets them). A native package fails above 50 MB packed or 150 MB unpacked;
  npm's own limit is not documented.
- **Notices.** `scripts/third_party_notices.py` (stdlib, `cargo metadata
  --locked --filter-platform`) writes one `THIRD_PARTY_NOTICES.md` per
  package: every crate with its license files, the native components from a
  curated list and the formatter's Handy notice. Two crates publish no
  license file (`ebml-iterable-specification` and its derive crate, MIT) and
  get the standard MIT text with their authors. The Rust version every
  shipping job installs is pinned there (`RUST_VERSION`); see
  [`scripts/notices/README.md`](../scripts/notices/README.md).
- **Install smoke.** `scripts/npm_registry_fixture.py` serves the generated
  tarballs from a loopback-only registry (it derives packuments from them,
  `libc` included), so installs go through the real resolution path without
  publishing. On all four hosts, from a prefix with spaces and an unrelated
  working directory, Node 22: `npm install -g` into an isolated prefix with
  empty npm configs, `pumice --version`, executable bits after install,
  `pumice transcribe --version`, `backends --json` (`bundle: verified`), a
  whisper-tiny CPU transcription and npx; on the Intel Mac, the "not
  installed next to pumice" error instead. On Linux, `pnpm add -g` and
  `bun add -g` too. On macOS, no npm-installed file carries
  `com.apple.quarantine`.

The packaging helpers have their own tests, which the workflow runs first and
which need no AI calls or network: `python3 -m unittest discover -s scripts
-p "test_*.py" -v` (`test_package_release.py`, `test_package_npm.py`,
`test_third_party_notices.py`, `test_npm_launcher.py` with a fake binary,
and `test_npm_install.py`, which installs fake tarballs with npm, pnpm and
Bun through the loopback registry, isolating each manager's config and cache
without overriding `HOME`).

### npm package layout

- **Front package `@brasillero/pumice`**: the zero-dependency CommonJS
  launcher `bin/pumice.js` (installed as the `pumice` command,
  `engines.node >= 22`), `LICENSE` and a README. Its `optionalDependencies`
  pin the four platform packages at the exact `Cargo.toml` version, so the
  front and its native payload cannot drift apart. The unscoped name
  `pumice` was refused by the registry as too similar to an existing package
  and `pumice-cli` was taken, so everything lives under the owner's
  `@brasillero` scope (owner decision, 2026-10-05); scoped packages publish
  with `--access public`.
- **Platform packages `@brasillero/pumice-<platform>`** (`win32-x64`,
  `linux-x64`, `darwin-x64`, `darwin-arm64`) carry `os`/`cpu` selectors, so
  package managers install only the matching one. `bin/` holds `pumice` and,
  for `win32-x64`, `linux-x64` and `darwin-arm64`, the complete
  transcription folder of the same commit; `darwin-x64` holds `pumice` alone
  (`p14.3-npm-bundles/D1`–`D3`). `linux-x64` declares `libc: ["glibc"]`
  because its transcription libraries need glibc 2.35 or newer.
- **Launcher.** It finds this platform's package among the front's optional
  dependencies, resolves it from the launcher's own directory
  (`require.resolve(<name>/package.json, {paths: [__dirname]})`, which works
  with pnpm's isolated layout; the caller's working directory is deliberately
  not searched, so a planted `node_modules` there cannot satisfy it), checks
  that both versions match and that the binary exists, then spawns it
  directly with inherited stdio, working directory and environment. Exit
  codes pass through; SIGINT and SIGTERM are forwarded, and a child killed
  by a signal makes the launcher die by the same signal. It reports an
  unsupported platform, a missing platform package (naming the glibc
  requirement on musl Linux), a version mismatch, a missing binary or a
  spawn error. It never downloads anything and no install scripts run.

### Publishing a release

1. Merge the release PR. On `main`, run `gh workflow run release.yml --ref
   main` and wait for it to pass.
2. `gh run download <run-id> -n release-preview -D dist` and
   `gh run download <run-id> -n npm-preview -D dist/npm`; then
   `cd dist && sha256sum -c SHA256SUMS`.
3. `gh release create v<version> --target <merge-commit> --draft --title
   v<version>` with the four `pumice-<version>-*` archives, `SHA256SUMS`
   and the CHANGELOG section as notes; review the draft, then publish.
4. Publish the **four platform packages first**, then the front package:
   `pnpm publish <tarball> --access public` for each
   `brasillero-pumice-<platform>-<version>.tgz`, then
   `brasillero-pumice-<version>.tgz`.
5. Smoke test: `pnpm dlx @brasillero/pumice@<version> --version` prints
   `pumice <version>`, and on a bundled platform `pumice transcribe
   --version` prints `pumice-transcribe <version>`.

Notes from earlier publications (0.1.0 on 2026-10-05, 0.2.0 on 2026-10-11):

- The npm account uses 2FA for authorization and writes, so publishing
  needs the owner. `pnpm publish` (and `pnpm login`) use npm's web flow:
  they print an `https://www.npmjs.com/login/cli/…` or `…/auth/cli/…` link
  to approve in the browser and only poll for the approval after Enter is
  pressed at "Press ENTER to open the URL". The link expires after about
  three minutes. After the first approval, the next publishes in the same
  session went through without another prompt.
- Without a terminal, publishing fails with `ERR_PNPM_OTP_NON_INTERACTIVE`;
  running it under `script` gives it one. An unauthenticated scoped publish
  fails with HTTP 404, not 401.
- Newly published packages took tens of minutes to show up in `npm view`
  and install metadata. A same-day 404 after a successful upload can be
  replication lag: check the publish output ("cannot publish over
  previously published version") before retrying.
