# Pumice

Polish your dictation with the AI subscriptions you already have.

Pumice is a small local service that sits between a dictation app and the AI coding CLIs you already pay for. The app sends its own prompt and the text transcribed by Whisper; Pumice hands that request, unchanged, to the official CLI of a subscription you are logged into and returns the answer ready to paste. Formatting, translating or anything else is decided by the app's prompt. No separately billed API keys required.

```
Handy (Whisper, on your machine) ──raw text──▶ Pumice ──▶ Claude / Codex / Kimi CLI
                                  ◀──polished text──
```

- **Works with [Handy](https://github.com/cjpais/Handy)** through its Custom post-processing provider, and with any app that talks to an OpenAI-compatible endpoint.
- **Uses the official CLIs** of your subscriptions: Claude, Codex, Kimi and Kiro.
- **Never loses a dictation:** if the selected provider fails or times out, Pumice answers with an error and the app pastes its own transcript.
- **Local and private:** listens on localhost only, no telemetry.
- **Single executable** for Windows, Linux and macOS, with nothing else to install.

## Status

Pumice works end to end through Claude, Codex, Kimi and Kiro. Antigravity and the local OpenAI-compatible adapter are archived for now. You choose the providers in an explicit list in the YAML config — nothing is enabled by default, and there is no fallback: a failure returns the raw dictation. Run `pumice doctor` to see which CLIs it found.

Pumice supports standalone downloads and the npm package `@brasillero/pumice` (installable with npm, pnpm, Bun or npx; the installed command is `pumice`). Check [GitHub Releases](https://github.com/brasillero/pumice/releases) and the [npm registry](https://www.npmjs.com/package/@brasillero/pumice) for published versions. See the [installation guide](docs/install.md) for installation, startup and Handy setup; building from source is also supported below.

The [latest standalone release](https://github.com/brasillero/pumice/releases/latest) is available, and the npm package installs with `pnpm add -g @brasillero/pumice` (or `npm install -g @brasillero/pumice`). The package is scoped because the registry rejected the unscoped name `pumice` as too similar to an existing package.

### Run from source (preview)

With [Rust](https://rustup.rs/) installed:

```sh
cargo build --release --locked --bin pumice
./target/release/pumice check-config   # validates pumice.yaml or the built-in defaults
./target/release/pumice doctor         # checks which CLIs are installed
./target/release/pumice                # starts the service (live view; use --plain for one-line stderr logs)
```

In Handy: **Settings → Advanced → Experimental Features → Post Processing**, set the provider to **Custom**, base URL `http://127.0.0.1:7567/v1`, API key empty, model `claude`, `codex`, `kimi` or `kiro`. Choose `passthrough` to return the original transcript exactly, including whitespace and line breaks, without calling an AI. With Handy, only the text inside its `<transcript>` envelope is returned; its formatting prompt is omitted. Choose `inspect` to echo the full JSON request body that Pumice received, including Handy's attached prompt and every message and option; the result is pasted as JSON, which is useful for diagnostics, and it never calls an AI. See [`pumice.example.yaml`](pumice.example.yaml) for every configuration setting. On WSL, use `127.0.0.1`, not `localhost` (see [`docs/research/S0.5-wsl-localhost.md`](docs/research/S0.5-wsl-localhost.md)).

- Product spec: [`docs/spec.md`](docs/spec.md)
- Current plan and status: [`HANDOFF.md`](HANDOFF.md)
- Rules for contributors and AI agents: [`AGENTS.md`](AGENTS.md)

## Roadmap

| Stage | Status |
| --- | --- |
| Local pass-through service (Claude, Codex, explicit provider list) | Implemented |
| Kimi adapter, auto-detection | Implemented |
| Kiro plugin | Implemented |
| Antigravity, generic loopback adapter | Archived; OpenCode support removed |
| Distribution: GitHub release archives + npm package | Released (v0.1.1); 0.2.0 after local testing |
| `pumice setup` configuration wizard | Next |
| Tauri installer, auto-update, start-with-the-system | Deferred |

## License

[MIT](LICENSE)
