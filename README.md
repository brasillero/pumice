# Pumice

Polish your dictation with the AI subscriptions you already have.

Pumice is a small local service that sits between a dictation app and the AI coding CLIs you already pay for. It receives the raw text transcribed by Whisper, lightly corrects it (transcription mistakes, punctuation, lists), and returns it ready to paste. No separately billed API keys required.

```
Handy (Whisper, on your machine) ──raw text──▶ Pumice ──▶ Claude / Codex / Kimi / OpenCode CLI
                                  ◀──polished text──
```

- **Works with [Handy](https://github.com/cjpais/Handy)** through its Custom post-processing provider, and with any app that talks to an OpenAI-compatible endpoint.
- **Uses the official CLIs** of your subscriptions (Claude, Codex, Kimi, OpenCode; Antigravity opt-in), or any OpenAI-compatible API.
- **Never loses a dictation:** if every provider fails or times out, you get the raw text back.
- **Local and private:** listens on localhost only, no telemetry.
- **Single executable** for Windows, Linux and macOS, with nothing else to install.

## Status

Pumice is in **Phase 1 (MVP)**: the service can already format dictation through Claude and Codex, but there is no release or installer yet.

### Run from source (preview)

With [Rust](https://rustup.rs/) installed:

```sh
cargo build --release
./target/release/pumice check-config   # validates pumice.yaml or the built-in defaults
./target/release/pumice doctor         # checks which CLIs are installed
./target/release/pumice serve
```

In Handy: **Settings → Advanced → Experimental Features → Post Processing**, set the provider to **Custom**, base URL `http://127.0.0.1:7567/v1`, API key empty, model `claude` or `codex`. See [`pumice.example.yaml`](pumice.example.yaml) for every configuration setting. On WSL, use `127.0.0.1`, not `localhost` (see [`docs/research/S0.5-wsl-localhost.md`](docs/research/S0.5-wsl-localhost.md)).

- Product spec: [`docs/spec.md`](docs/spec.md)
- Current plan and status: [`HANDOFF.md`](HANDOFF.md)
- Rules for contributors and AI agents: [`AGENTS.md`](AGENTS.md)

## Roadmap

| Version | Goal |
| --- | --- |
| v0.1 | Claude and Codex, fallback chain |
| v0.2 | OpenCode, Antigravity (opt-in), generic adapter, auto-detection |
| v1.0 | Installers, auto-update and start with the system on Windows, Linux and macOS |

## License

[MIT](LICENSE)
