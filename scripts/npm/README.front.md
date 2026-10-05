# pumice

Polish your dictation with the AI subscriptions you already have.

Pumice is a small local service that sits between a dictation app and the AI
coding CLIs you already pay for. It receives the raw text transcribed by
Whisper (through the [Handy](https://github.com/cjpais/Handy) app), lightly
corrects it (transcription mistakes, punctuation, lists), and returns it ready
to paste.

**Status: not published yet.** This package is prepared for the npm registry
but has not been published, and no GitHub release exists yet either. The
commands below are the intended usage once the owner publishes it. In the
meantime you can build pumice from source with Cargo — see the
[repository](https://github.com/brasillero/pumice).

## Requirements

- **Node.js >= 22** to run the launcher, even when you install with pnpm or
  Bun. The pumice service executable itself is a standalone native binary
  with no runtime dependencies.
- One of the supported platforms (see below).

## Install

```sh
npm install -g pumice
pnpm add -g pumice
bun add -g pumice
npx pumice@latest --version
```

The `pumice` package is a small launcher. It selects the native binary for
your platform from the `@brasillero/pumice-<os>-<arch>` optional
dependencies, verifies that the versions match, and runs it. **Do not remove
the optional dependencies** or the launcher reports the missing native
package instead of running. The launcher itself performs no downloads and
runs no install scripts. Your package manager fetches the front package and
the native package matching your platform from the registry.

## Platforms

| Platform | Package |
| --- | --- |
| Linux x64 (glibc and musl) | `@brasillero/pumice-linux-x64` |
| Windows x64 | `@brasillero/pumice-win32-x64` |
| macOS x64 (Intel) | `@brasillero/pumice-darwin-x64` |
| macOS arm64 (Apple Silicon) | `@brasillero/pumice-darwin-arm64` |

Package managers skip the optional dependencies that do not match your
platform, so only one native binary is downloaded.

## Use

```sh
pumice doctor        # shows which AI CLIs are installed and reachable
pumice serve         # starts the service on http://127.0.0.1:7567/v1
pumice check-config  # validates the optional YAML config
```

In Handy: **Settings → Advanced → Experimental Features → Post Processing**,
set the provider to **Custom**, base URL `http://127.0.0.1:7567/v1`, API key
empty, model `claude` or `codex`. Stop the service with Ctrl-C.

Every setting has a built-in default, so a config file is not required. To
customize, create the per-user config and edit it:

- Windows: `%APPDATA%\pumice\pumice.yaml`
- Linux and macOS: `~/.config/pumice/pumice.yaml`

The service listens on `127.0.0.1` only, sends no telemetry and makes no
outbound connections of its own.

## AI CLIs

Pumice formats text **through the official CLIs of your subscriptions**
(currently Claude, Codex and OpenCode), or a local OpenAI-compatible service such as Ollama or LM Studio.
Install and authenticate those separately — the npm package does not install
or log into them, and it cannot provide accounts. (Antigravity support is
dormant and Kimi is not integrated yet.) If every provider fails or times
out, Pumice returns the raw text unchanged.

## Upgrade

```sh
npm update -g pumice
pnpm update -g pumice
bun update -g pumice
```

## Remove

```sh
npm uninstall -g pumice
pnpm remove -g pumice
bun remove -g pumice
```

## License

[MIT](LICENSE)
