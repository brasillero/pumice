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
  in the *same* environment where Pumice runs — for example `claude` or
  `codex` on your PATH. Pumice never installs or logs into these for you, and
  it never reads their credentials: it only invokes the CLI you already use.
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
- Model: `claude` or `codex` (the CLIs you installed above), `passthrough`
  to get the raw transcript back exactly as dictated, or `inspect` to echo
  the full JSON request body for diagnostics. `inspect` includes Handy's
  attached prompt and pastes the result as JSON; neither `passthrough` nor
  `inspect` spends quota or calls an AI.

No Handy changes are needed beyond this Custom provider configuration.

## Configuration

Without a config file Pumice starts with no providers: every request
returns the original text. To format dictations, create the per-user config
and list the providers you want:

- Windows: `%APPDATA%\pumice\pumice.yaml`
- Linux and macOS: `~/.config/pumice/pumice.yaml`
  (or `$XDG_CONFIG_HOME/pumice/pumice.yaml`)

```yaml
default: claude            # used when the client sends no model (optional)

providers:                 # there is no fallback: a failure returns the raw text
  - id: claude
    enabled: true          # required on every entry
    model: haiku           # required when enabled
  - id: codex
    enabled: false         # kept in the file, not offered, never run
```

Every entry needs `id` and `enabled`; `model` is required on enabled
entries; `timeout_secs`, `binary`, `env` and `options` are optional and
documented in [`pumice.example.yaml`](../pumice.example.yaml), which shows
every supported provider. Validate the file with `pumice check-config`. The
config lives outside the install folder, so upgrading never touches it.
A `pumice setup` wizard that writes this file is planned next.

Which providers are available today:

- **Claude** and **Codex** — implemented; enable them in the YAML (the
  example config shows how) and set the model in Handy to pick one.
- **OpenCode** and **Kimi** — implemented, off by default; enable them in
  the YAML with an explicit model.
- **Local models (Ollama, LM Studio)** — a generic adapter for
  OpenAI-compatible services listening on `127.0.0.1`, off by default.
- **Antigravity** — dormant (protocol implemented, opt-in blocked for now).

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
makes no external network calls itself. AI CLIs contact their providers;
the generic adapter connects only to local services. It never loses a dictation: if the
selected provider fails or times out, the raw text comes back unchanged.
