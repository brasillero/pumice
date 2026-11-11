# Pumice

Use your AI subscriptions from an OpenAI-compatible client. Pumice passes the client's prompt and text to an official Claude Code, Codex or Kiro CLI and returns its answer. Install and authenticate the CLI separately.

## Install

Requires Node.js 22+ and Windows x64, Linux x64 (glibc or musl), macOS Apple Silicon.

```sh
npm install -g @brasillero/pumice
# or: pnpm add -g @brasillero/pumice
# or: bun add -g @brasillero/pumice
npx @brasillero/pumice@latest --version
```

The package is a small launcher. Your package manager installs the matching native optional dependency; keep optional dependencies enabled. Each native package contains the standalone `pumice` executable, license and generated third-party notices. The launcher checks versions and forwards arguments, input, output and exit status. It performs no downloads or install scripts itself.

## Use

```sh
pumice doctor        # find installed AI CLIs; no quota spent
pumice               # start http://127.0.0.1:7567/v1
pumice --plain       # use one-line logs
pumice check-config  # validate settings
```

`pumice serve` is an alias for `pumice`. Stop with Ctrl-C. First start creates one complete `pumice.yml` in `%APPDATA%\pumice` on Windows or `~/.config/pumice` on Linux/macOS (`XDG_CONFIG_HOME` is respected). Existing files are preserved. Edit it to select providers and models, or use an existing file with `--config PATH`. Every setting must be present. Before upgrading, rename an older `relay.yml` or `pumice.yaml` to `pumice.yml` and change service commands to `pumice --plain`.

For Handy's Custom post-processing provider, use base URL `http://127.0.0.1:7567/v1`, an empty API key, and an enabled model (`claude`, `codex` or `kiro`). `passthrough` returns the transcript without calling an AI; `inspect` echoes the request JSON for diagnostics. The app decides what happens to its text. On provider failure Pumice returns an HTTP error, without trying another provider.

Pumice listens on IPv4 loopback only, sends no telemetry, and keeps request text out of logs unless debug logging is explicitly enabled.

## Update or remove

```sh
npm update -g @brasillero/pumice
npm uninstall -g @brasillero/pumice
```

The equivalent pnpm and Bun commands work too. User settings remain separate from the install.

[Full documentation](https://github.com/brasillero/pumice/blob/main/docs/install.md) · [Source](https://github.com/brasillero/pumice) · [MIT license](LICENSE)
