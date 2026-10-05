# Installing and running Pumice

Pumice is a small local service that polishes your dictation with the AI
subscriptions you already have. It listens on `http://127.0.0.1:7567/v1`,
receives text transcribed by [Handy](https://github.com/cjpais/Handy) (or any
app with an OpenAI-compatible post-processing endpoint), formats it through
the official CLI of your subscription, and returns the result to be pasted.

> **Availability:** the first GitHub release and the npm package are not
> published yet (as of 2026-10-05). The commands below describe the intended
> usage once the owner publishes them; until then, build from source with
> Cargo (see the [README](../README.md)). Check
> [GitHub Releases](https://github.com/brasillero/pumice/releases) for the
> current published versions.

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

```sh
npm install -g pumice
pnpm add -g pumice
bun add -g pumice
npx pumice@latest --version   # run without installing
```

Your package manager fetches the `pumice` package and the single native
binary matching your platform. Bun works through the same package, with
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
- Model: `claude` or `codex` (the CLIs you installed above), or `passthrough`
  to get the raw transcript back exactly as dictated — useful to test the
  connection without spending any quota.

No Handy changes are needed beyond this Custom provider configuration.

## Configuration (optional)

Every setting has a built-in default, so most users never need a config file.
Provider and model settings are edited manually in a YAML file for now; CLI
commands to manage them are planned next.

To customize, create the per-user config and edit it:

- Windows: `%APPDATA%\pumice\pumice.yaml`
- Linux and macOS: `~/.config/pumice/pumice.yaml`
  (or `$XDG_CONFIG_HOME/pumice/pumice.yaml`)

Validate it with `pumice check-config`. The config lives outside the install
folder, so upgrading never touches it.

Which providers are available today:

- **Claude** and **Codex** — implemented, enabled by default (set the model
  in Handy to pick one).
- **OpenCode** — implemented, off by default; enable it in the YAML with an
  explicit provider/model.
- **Local models (Ollama, LM Studio)** — a generic adapter for
  OpenAI-compatible services listening on `127.0.0.1`, off by default.
- **Antigravity** — dormant (protocol implemented, opt-in blocked for now).
- **Kimi** — not integrated (deferred).

## Upgrade and remove

Stop the service with Ctrl-C before updating or removing it. Choose the
command for your package manager:

| Package manager | Upgrade | Remove |
| --- | --- | --- |
| npm | `npm update -g pumice` | `npm uninstall -g pumice` |
| pnpm | `pnpm update -g pumice` | `pnpm remove -g pumice` |
| Bun | `bun update -g pumice` | `bun remove -g pumice` |

For archive installs, extract the newer archive into a fresh folder and
replace the old executable. Your per-user config is kept. Restart with
`pumice` after updating. To remove an archive install, delete its folder and
remove that directory from your PATH if you added it.

## Privacy

Pumice binds to `127.0.0.1` only, sends no telemetry, has no auto-updater and
makes no external network calls itself. AI CLIs contact their providers;
the generic adapter connects only to local services. It never loses a dictation: if every
provider fails or times out, the raw text comes back unchanged.
