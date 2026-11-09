# Installing and running Pumice

Pumice relays OpenAI-compatible chat requests to the official Claude Code, Codex or Kiro CLI. Install and authenticate the CLI yourself. Pumice inherits its settings, gateway and login; it never reads or reuses credentials.

## Install

Download the archive for your platform from [GitHub Releases](https://github.com/brasillero/pumice/releases), verify its `SHA256SUMS`, and extract it. Put `pumice` (`pumice.exe` on Windows) on your PATH. No developer tools or runtime are needed for an archive install.

| Platform | Release target | Runtime |
| --- | --- | --- |
| Windows x64 | `x86_64-pc-windows-msvc` | Static C runtime; no separate CRT install |
| Linux x64 | `x86_64-unknown-linux-musl` | Static executable, for glibc and musl systems |
| macOS Intel | `x86_64-apple-darwin` | OS libraries only |
| macOS Apple Silicon | `aarch64-apple-darwin` | OS libraries only |

Alternatively, with Node.js 22 or newer:

```sh
npm install -g @brasillero/pumice
# or: pnpm add -g @brasillero/pumice
# or: bun add -g @brasillero/pumice
npx @brasillero/pumice@latest --version
```

The npm package is a small launcher with an exact-version optional dependency for each platform. Your package manager installs the matching native package. Keep optional dependencies enabled. The launcher checks versions and forwards arguments, input, output and exit status to the executable; it performs no downloads or install scripts itself.

## Run the service

Once `pumice` is on your PATH:

```sh
pumice
```

That command starts the relay service and prints the address it listens on,
`http://127.0.0.1:7567/v1`. (`pumice serve` is an alias; `pumice --config <path>` uses an explicit file.) Stop the service with Ctrl-C.

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
./pumice        # Linux and macOS
.\pumice.exe relay serve    # Windows PowerShell
```

## Check that it works

```sh
pumice doctor         # shows which provider CLIs were found; spends no quota
pumice --version
```

`doctor` only probes for installed CLIs, so it is free to run as often as you
like. To verify that a provider is actually logged in and answers,
run `pumice doctor --login-check --provider claude` — that one call spends a
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
The first command that needs settings creates the missing per-user file from [`pumice.example.yml`](../pumice.example.yml).
Existing files are never overwritten or filled in. Edit the file at:

- Windows: `%APPDATA%\pumice\pumice.yml`
- Linux and macOS: `~/.config/pumice/pumice.yml`
  (or `$XDG_CONFIG_HOME/pumice/pumice.yml`)

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
`pumice check-config`; `pumice doctor` also runs before the
file exists. The config lives outside the install folder, so upgrading
never replaces it. Automatic creation writes only `pumice.yml`. An explicit `--config` path must already exist;
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

## Upgrading

Pumice is the relay again. Before the first start after updating:

1. Rename your existing `relay.yml` (or the older `pumice.yaml`) to `pumice.yml` in the same per-user directory. Preserve its provider settings. The previous names are not read automatically; otherwise a fresh initial config would be created.
2. Replace `pumice relay serve` with `pumice` in scripts and services. `pumice --plain` is suitable for a systemd user service. `pumice serve` now starts this same relay on its configured port.
3. Run `pumice check-config`. Every setting must be present, including `effort` for each provider and `thinking` for Claude. Compare with [pumice.example.yml](../pumice.example.yml) when upgrading an older config.
4. For a systemd user service, update `ExecStart=` to the installed binary path followed by `--plain`, then run `systemctl --user daemon-reload` and `systemctl --user restart pumice`.

The chat API and default port 7567 are unchanged; Handy needs no change. Existing user files are never deleted or rewritten. Old settings can stay on disk, but Pumice only loads its single settings file.

Update npm installs with `npm update -g @brasillero/pumice` (or the equivalent pnpm/Bun command); replace an archive install's folder. To remove it, uninstall the package or delete the extracted folder. User settings remain separate.

## Privacy

Pumice listens on `127.0.0.1` only and sends no telemetry. Request text stays out of ordinary logs; `debug_log.enabled: true` explicitly permits text in the debug log and live details. Each official CLI can have its own retention behavior; see [providers.md](providers.md), especially Kiro's local session storage. The client owns the prompt; Pumice never executes dictated instructions.

## Building and publishing releases

The [Release preview workflow](../.github/workflows/release.yml) runs on relevant pull requests and manual dispatch. It never publishes. It:

- Tests the packaging helpers and launcher with fake binaries and a loopback npm registry.
- Builds `pumice` for all four targets using the Rust version pinned by `scripts/third_party_notices.py --print-rust-version`.
- Checks Linux static linkage and Windows static CRT linkage, then tests `--version` from each archive.
- Generates target-specific notices for Cargo dependencies, the Rust standard library and musl. See [the license source notes](../scripts/notices/README.md) when changing the release toolchain.
- Packs four archives, each containing the executable, `LICENSE`, `THIRD_PARTY_NOTICES.md`, `pumice.example.yml` and `INSTALL.md`, plus checksums.
- Packs the front npm launcher and four native packages. Each native package holds just `bin/pumice` (or `.exe`), its manifest, license and generated notices. Archive hashes and tarball contents, modes and hashes are checked before upload. `PACKAGES.md` records sizes; the limits are 50 MB packed and 150 MB unpacked per package.
- Installs the tarballs through a loopback registry on all four platforms and tests npm/npx plus pnpm/Bun on Linux. No real AI calls run.

To publish:

1. Prepare the version and hand-written changelog in a release PR, merge it, then run `gh workflow run release.yml --ref main`. Wait for that manual run to pass.
2. Download its `release-preview` and `npm-preview` artifacts. Verify `SHA256SUMS` in both directories.
3. Create a draft GitHub release `v<version>` targeting the merge commit, attach the four archives and archive checksums, and use the changelog as release notes. Review and publish it.
4. With the owner's npm login/2FA, publish the four native tarballs first, then the front tarball: `pnpm publish <tarball> --access public`.
5. Verify `pnpm dlx @brasillero/pumice@<version> --version` prints `pumice <version>`.

Publishing needs the owner. The npm web approval flow can expire after a few minutes, and registry metadata may lag a successful publish. Check upload output before retrying; a version cannot be overwritten.
