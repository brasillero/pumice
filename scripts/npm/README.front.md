# pumice

Polish your dictation with the AI subscriptions you already have.

Pumice is a small open-source tool that runs on your computer and connects
your dictation app (such as [Handy](https://github.com/cjpais/Handy)) to the AI
assistants you already pay for (Claude, Codex, Kimi or Kiro), using their
official command-line apps. The app sends its own prompt and the text
transcribed by Whisper; Pumice passes them, unchanged, to the assistant you
chose and returns the answer ready to paste. The app's prompt decides what
happens to the text.

**Availability:** see [GitHub Releases](https://github.com/brasillero/pumice/releases)
and the [npm registry](https://www.npmjs.com/package/@brasillero/pumice) for the current
published versions. The commands below are the usage once installed. You can
also build pumice from source with Cargo — see the
[repository](https://github.com/brasillero/pumice).

## Requirements

- **Node.js >= 22** to run the launcher, even when you install with pnpm or
  Bun. The pumice service executable itself is a standalone native binary
  with no runtime dependencies.
- One of the supported platforms: Linux x64 with glibc 2.35 or newer
  (musl-based distributions such as Alpine are not supported), Windows x64,
  macOS x64 (Intel) or macOS ARM64 (Apple Silicon).

On Windows, Linux and Apple Silicon the platform package also carries local
transcription (`pumice transcribe`: the `pumice-transcribe` program, its
engine libraries, CPU variants and GPU module); it runs on the CPU unless
you ask for the GPU, and needs a Whisper model you download separately. The
Intel Mac package has `pumice` only. See the
[transcription guide](https://github.com/brasillero/pumice/blob/main/docs/transcription.md).

## Install

```sh
npm install -g @brasillero/pumice
pnpm add -g @brasillero/pumice
bun add -g @brasillero/pumice
npx @brasillero/pumice@latest --version
```

(The package lives under the `@brasillero` scope because the unscoped npm
name `pumice` is unavailable; the installed command is still `pumice`.)

The `@brasillero/pumice` package is a small launcher. It selects the native binary for
your platform from its optional dependencies, verifies that the versions
match, and runs it. **Do not remove the optional dependencies** or the
launcher reports the missing native package instead of running. The launcher
itself performs no downloads and runs no install scripts. Your package
manager fetches the front package and the single native binary matching your
platform from the registry.

## Use

```sh
pumice relay serve        # starts the service on http://127.0.0.1:7567/v1
pumice relay doctor       # shows which AI CLIs are installed and reachable
pumice relay check-config # validates the YAML config
```

(`pumice relay` with no further arguments is the start command, and
`pumice relay --config <path>` starts it with an explicit config file.)

In Handy: **Settings → Advanced → Experimental Features → Post Processing**,
set the provider to **Custom**, base URL `http://127.0.0.1:7567/v1`, API key
empty, model `claude` or `codex`. Use `passthrough` to return the raw transcript
without calling an AI, or `inspect` to echo the full JSON request body for
diagnostics; `inspect` includes Handy's attached prompt and pastes the result
as JSON. Stop the service with Ctrl-C.

The service needs a config file in which every setting is written (there
are no built-in defaults; a missing setting is reported with its file and
line). The first command that needs settings creates missing per-user
files from the supplied examples. Existing files are never overwritten.
Edit the relay settings at:

- Windows: `%APPDATA%\pumice\relay.yml`
- Linux and macOS: `~/.config/pumice/relay.yml`

```yaml
port: 7567
total_timeout_secs: 30
max_parallel: 4
providers:
  - id: claude
    enabled: true
    model: haiku
    binary: claude
    timeout_secs: 30
debug_log:
  enabled: false
  path: pumice-debug.jsonl
```

`pumice relay doctor` runs before the file exists.

The service listens on `127.0.0.1` only, sends no telemetry and makes no
external connections of its own.

## AI CLIs

Pumice relays your dictation app's request **through the official CLIs of
your subscriptions** (currently Claude, Codex and Kimi). Install and
authenticate those separately — the npm package does not install or log into
them, and it cannot provide accounts. If the selected provider fails or times
out, Pumice answers with an error and the app keeps its own transcript.

## Upgrade

```sh
npm update -g @brasillero/pumice
pnpm update -g @brasillero/pumice
bun update -g @brasillero/pumice
```

## Remove

```sh
npm uninstall -g @brasillero/pumice
pnpm remove -g @brasillero/pumice
bun remove -g @brasillero/pumice
```

## License

[MIT](LICENSE)
