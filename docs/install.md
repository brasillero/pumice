# Installing and running Pumice

Pumice is a small local service that polishes your dictation with the AI
subscriptions you already have. It listens on `http://127.0.0.1:7567/v1`,
receives text transcribed by [Handy](https://github.com/cjpais/Handy) (or any
app with an OpenAI-compatible post-processing endpoint), formats it through
the official CLI of your subscription, and returns the result to be pasted.

> **Availability:** check [GitHub Releases](https://github.com/brasillero/pumice/releases)
> and the [npm registry](https://www.npmjs.com/package/@brasillero/pumice) for published
> versions. You can also build from source with Cargo (see the [README](../README.md)).

## What you need

- One of the supported platforms: **Windows x64, Linux x64, macOS x64 (Intel)
  or macOS ARM64 (Apple Silicon)**.
- The **official CLI of at least one AI provider**, installed and logged into
  in the *same* environment where Pumice runs — for example `claude`,
  `codex`, `kimi` or `kiro-cli` on your PATH. Pumice never installs or logs
  into these for you, and it never reads their credentials: it only invokes
  the CLI you already use.
- **Node.js 22 or newer**, only if you install through npm/pnpm/Bun. The npm
  package ships a tiny launcher, but the Pumice service itself is a
  standalone native executable with nothing else to install.

## Install

### Option 1: portable archive (no runtime needed)

Download the archive for your platform from
[GitHub Releases](https://github.com/brasillero/pumice/releases) and extract
it anywhere. The executable inside is self-contained.

### Option 2: package manager (Node.js 22+)

The npm upload is being finalized after correcting the account scope as of 2026-10-05.
Use the standalone GitHub release for now; the commands below work after npm publication.

```sh
npm install -g @brasillero/pumice
pnpm add -g @brasillero/pumice
bun add -g @brasillero/pumice
npx @brasillero/pumice@latest --version   # run without installing
```

Your package manager fetches the `@brasillero/pumice` package and the single
native binary matching your platform. Bun works through the same package, with
Node.js installed for the launcher.

## Run the service

Once `pumice` is on your PATH:

```sh
pumice
```

That single command starts the service and prints the address it listens on,
`http://127.0.0.1:7567/v1`. (`pumice serve` is an explicit alias for the same
command, and `pumice --config <path>` starts it with an explicit config
file.) Stop the service with Ctrl-C.

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
./pumice          # Linux and macOS
.\pumice.exe      # Windows PowerShell
```

## Check that it works

```sh
pumice doctor         # shows which provider CLIs were found; spends no quota
pumice --version
```

`doctor` only probes for installed CLIs, so it is free to run as often as you
like. To verify that a provider is actually logged in and can format text,
run `pumice doctor --login-check --provider claude` — that one call spends a
small amount of your plan quota, which is why it only runs when you ask for
it.

## Point Handy at it

In Handy: **Settings → Advanced → Experimental Features → Post Processing**,
then:

- Provider: **Custom**
- Base URL: `http://127.0.0.1:7567/v1`
- API key: leave **empty**
- Model: `claude`, `codex`, `kimi` or `kiro` (the CLIs you installed
  above), `passthrough` to get the raw transcript back exactly as dictated,
  or `inspect` to echo the full JSON request body for diagnostics. `inspect` includes Handy's
  attached prompt and pastes the result as JSON; neither `passthrough` nor
  `inspect` spends quota or calls an AI.

No Handy changes are needed beyond this Custom provider configuration.

## Configuration

Without a config file Pumice starts with no providers: every request
gets an error and the app keeps its own transcript. To format dictations, create the per-user config
and list the providers you want:

- Windows: `%APPDATA%\pumice\pumice.yaml`
- Linux and macOS: `~/.config/pumice/pumice.yaml`
  (or `$XDG_CONFIG_HOME/pumice/pumice.yaml`)

```yaml
providers:                 # the client's model picks the entry; there is no
  - id: claude             # fallback: a failure, or no model, is an error
    enabled: true          # and the app keeps its own transcript
    model: haiku           # required when enabled
  - id: codex
    enabled: false         # kept in the file, not offered, never run
```

The client picks the provider: Handy sends the `model` the user chose, and
Pumice runs exactly that provider. A request without a model, or any
failure, is answered with an HTTP error, and the app keeps its own
transcript. Every entry needs `id` and `enabled`; `model` is required
on enabled entries; `timeout_secs` and `binary` are optional and
documented in [`pumice.example.yaml`](../pumice.example.yaml), which shows
every supported provider. Gateways, routing and logins are configured in
each CLI itself; Pumice inherits them. Validate the file with `pumice check-config`. The
config lives outside the install folder, so upgrading never touches it.
A `pumice setup` wizard that writes this file is planned next.

`max_parallel` (default 4, at most 32) sets how many requests may run their
provider at the same time. Extra requests wait in line, first come first
served; a request still waiting when its `total_timeout_secs` budget runs
out gets HTTP 503.

Which providers are available today:

Nothing is on until you list it with `enabled: true` and a `model` (the
example config shows how). Then set the model in Handy to the provider's
id to pick it.

- **Claude**, **Codex**, **Kimi** and **Kiro** — supported and tested with real calls.
- **Antigravity** and the **local-model adapter** (Ollama, LM Studio) —
  archived for now; listing them in the config is an error.
- **OpenCode** — support was removed.

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
`pumice` after updating. To remove an archive install, delete its folder and
remove that directory from your PATH if you added it.

## Privacy

Pumice binds to `127.0.0.1` only, sends no telemetry, has no auto-updater and
makes no external network calls itself; only the AI CLIs contact their
providers. It never loses a dictation: if the selected provider fails or
times out, Pumice answers with an error and the app pastes its own
transcript.
