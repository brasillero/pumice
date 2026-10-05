# Pumice

Polish your dictation with the AI subscriptions you already have.

Pumice is a small local service that sits between a dictation app and the AI coding CLIs you already pay for. It receives the raw text transcribed by Whisper, lightly corrects it (transcription mistakes, punctuation, lists), and returns it ready to paste. No separately billed API keys required.

```
Handy (Whisper, on your machine) ──raw text──▶ Pumice ──▶ Claude / Codex / OpenCode CLI
                                  ◀──polished text──
```

- **Works with [Handy](https://github.com/cjpais/Handy)** through its Custom post-processing provider, and with any app that talks to an OpenAI-compatible endpoint.
- **Uses the official CLIs** of your subscriptions (Claude, Codex, OpenCode; Kimi deferred, Antigravity dormant), or a local OpenAI-compatible service such as Ollama or LM Studio.
- **Never loses a dictation:** if every provider fails or times out, you get the raw text back.
- **Local and private:** listens on localhost only, no telemetry.
- **Single executable** for Windows, Linux and macOS, with nothing else to install.

## Status

Pumice works end to end through Claude and Codex (with a fallback chain), and optionally OpenCode. Run `pumice doctor` to see which CLIs it found.

Nothing is published yet: the first GitHub release and the npm package (`npm install -g pumice`, installable with npm, pnpm, Bun or npx) are prepared and preview-built in CI, but publication is still the owner's call. Until then, run from source below. See the [installation guide](docs/install.md) for the intended install, run and Handy setup flow.

### Run from source (preview)

With [Rust](https://rustup.rs/) installed:

```sh
cargo build --release --locked --bin pumice
./target/release/pumice check-config   # validates pumice.yaml or the built-in defaults
./target/release/pumice doctor         # checks which CLIs are installed
./target/release/pumice                # starts the service
```

In Handy: **Settings → Advanced → Experimental Features → Post Processing**, set the provider to **Custom**, base URL `http://127.0.0.1:7567/v1`, API key empty, model `claude` or `codex`. Choose `passthrough` to return the original transcript exactly, including whitespace and line breaks, without calling an AI. With Handy, only the text inside its `<transcript>` envelope is returned; its formatting prompt is omitted. See [`pumice.example.yaml`](pumice.example.yaml) for every configuration setting. On WSL, use `127.0.0.1`, not `localhost` (see [`docs/research/S0.5-wsl-localhost.md`](docs/research/S0.5-wsl-localhost.md)).

- Product spec: [`docs/spec.md`](docs/spec.md)
- Current plan and status: [`HANDOFF.md`](HANDOFF.md)
- Rules for contributors and AI agents: [`AGENTS.md`](AGENTS.md)

## Roadmap

| Stage | Status |
| --- | --- |
| Local formatting service (Claude, Codex, fallback chain) | Implemented |
| OpenCode, generic loopback adapter, auto-detection | Implemented |
| Antigravity execution, Kimi integration | Deferred |
| Distribution: GitHub release archives + npm package | In review (publication is the owner's call) |
| CLI-managed configuration (one model per provider) | Next |
| Tauri installer, auto-update, start-with-the-system | Deferred |

## License

[MIT](LICENSE)
