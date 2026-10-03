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

Early planning. Pumice is in **Phase 0 (investigation)**; there is nothing to install yet.

- Product spec: [`docs/spec.md`](docs/spec.md)
- Current plan and status: [`HANDOFF.md`](HANDOFF.md)
- Rules for contributors and AI agents: [`AGENTS.md`](AGENTS.md)

## Roadmap

| Version | Goal |
| --- | --- |
| v0.1 | MVP: formatted dictation end to end through Handy, with Claude |
| v0.2 | Codex, Kimi, OpenCode, Antigravity and generic adapters, fallback chain, CLI auto-detection |
| v1.0 | Installers, auto-update and start with the system on Windows, Linux and macOS |

## License

[MIT](LICENSE)
