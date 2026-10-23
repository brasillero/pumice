# Pumice

Polish your dictation with the AI subscriptions you already have.

Pumice is a small open-source tool that runs on your computer and connects your dictation app to the AI assistants you already pay for (Claude, Codex or Kiro), using their official command-line apps. The dictation app sends its own prompt and the text transcribed by Whisper; Pumice passes them, unchanged, to the assistant you chose and returns the answer ready to paste. Formatting, translating or anything else is decided by the app's prompt. No separately billed API keys required.

```
Handy (Whisper, on your machine) ──raw text──▶ Pumice ──▶ Claude / Codex / Kiro CLI
                                  ◀──polished text──
```

- **Works with [Handy](https://github.com/cjpais/Handy)** through its Custom post-processing provider, and with any app that talks to an OpenAI-compatible endpoint.
- **Uses the official CLIs** of your subscriptions: Claude, Codex and Kiro.
- **Never loses a dictation:** if the selected provider fails or times out, Pumice answers with an error and the app pastes its own transcript.
- **Local and private:** listens on localhost only, no telemetry.
- **Single executable** for Windows, Linux and macOS, with no runtime to install (you only need the provider CLIs you already use).

## Status

Pumice works end to end through Claude, Codex and Kiro. The local OpenAI-compatible adapter is archived for now, and Kimi and Antigravity support was removed. You choose the providers in an explicit list in the YAML config — only entries marked `enabled: true` run, and there is no fallback: a failure is answered with an HTTP error and the app pastes its own transcript. Each provider entry sets its reasoning effort (`low`, `medium` or `high`), and the Claude entry also says whether Claude may think. Every setting is written in the module's YAML file: there are no built-in defaults, and a missing setting stops the module with an error naming the file and line (the `*.example.yml` files write every setting and are the starting configuration). The first command that needs settings creates missing per-user files automatically and preserves existing files. The starting relay file enables Claude; edit it to choose your providers. Run `pumice relay doctor` to see which CLIs it found.

Pumice supports standalone downloads and the npm package `@brasillero/pumice` (installable with npm, pnpm, Bun or npx; the installed command is `pumice`). Check [GitHub Releases](https://github.com/brasillero/pumice/releases) and the [npm registry](https://www.npmjs.com/package/@brasillero/pumice) for published versions. See the [installation guide](docs/install.md) for installation, startup and Handy setup; building from source is also supported below.

The [latest standalone release](https://github.com/brasillero/pumice/releases/latest) is available, and the npm package installs with `pnpm add -g @brasillero/pumice` (or `npm install -g @brasillero/pumice`). The package is scoped because the registry rejected the unscoped name `pumice` as too similar to an existing package.

### Run from source (preview)

With [Rust](https://rustup.rs/) installed:

```sh
cargo build --release --locked --bin pumice
mkdir -p ~/.config/pumice
cp relay.example.yml ~/.config/pumice/relay.yml   # every setting; edit the providers
./target/release/pumice relay check-config   # validates the per-user config (or --config <path>)
./target/release/pumice relay doctor         # checks which CLIs are installed
./target/release/pumice relay serve          # starts the service (live view; use --plain for one-line stderr logs)
```

With the [transcription module](docs/transcription.md) installed next to `pumice`, one audio file can go through the whole stack — transcribe, format, relay — in a single command driven by a flow from `orchestrator.yml`:

```sh
./target/release/pumice run meeting.wav --flow clean          # plain text on stdout
./target/release/pumice run meeting.wav --flow clean --json   # one JSON document: raw transcript and result apart
```

See [`docs/orchestrator.md`](docs/orchestrator.md) and [`orchestrator.example.yml`](orchestrator.example.yml) for flows, budgets, the result schema and exit codes. `pumice serve` keeps the transcription model loaded and takes the same flows over HTTP (`POST /pumice/v1/dictations`, and OpenAI's `POST /v1/audio/transcriptions` with the real transcription model id for raw text or `model:provider:preset` for AI processing); every HTTP API Pumice serves is summarized in [`docs/api.md`](docs/api.md).

In Handy: **Settings → Advanced → Experimental Features → Post Processing**, set the provider to **Custom**, base URL `http://127.0.0.1:7567/v1`, API key empty, model `claude`, `codex` or `kiro`. Choose `passthrough` to return the original transcript exactly, including whitespace and line breaks, without calling an AI. With Handy, only the text inside its `<transcript>` envelope is returned; its formatting prompt is omitted. Choose `inspect` to echo the full JSON request body that Pumice received, including Handy's attached prompt and every message and option; the result is pasted as JSON, which is useful for diagnostics, and it never calls an AI. See [`relay.example.yml`](relay.example.yml) for every configuration setting. On WSL, use `127.0.0.1`, not `localhost` (see [`docs/research/S0.5-wsl-localhost.md`](docs/research/S0.5-wsl-localhost.md)).

**Upgrading from 0.2:** the old `pumice` / `pumice --plain` commands no longer start the service and the old `pumice.yaml` config is no longer read. Run `pumice relay serve` instead of `pumice`. For a systemd user service (server-1): `mv ~/.config/pumice/pumice.yaml ~/.config/pumice/relay.yml`, change the unit's `ExecStart=` to `<installed binary path> relay serve --plain`, then `systemctl --user daemon-reload && systemctl --user restart pumice`. Handy's URL and port 7567 are unchanged. Full steps: [Upgrading from 0.2](docs/install.md#upgrading-from-02) in the installation guide.

- Product spec: [`docs/spec.md`](docs/spec.md)
- Transcription module guide: [`docs/transcription.md`](docs/transcription.md)
- Formatter module guide: [`docs/formatter.md`](docs/formatter.md)
- Orchestrator guide (`pumice run`, `pumice serve`): [`docs/orchestrator.md`](docs/orchestrator.md)
- HTTP APIs (relay, transcription, `pumice serve`): [`docs/api.md`](docs/api.md)
- Plans, decisions and open work: [`.task-decisions/`](.task-decisions/README.md)
- Rules for contributors and AI agents: [`AGENTS.md`](AGENTS.md)

## Roadmap

| Stage | Status |
| --- | --- |
| Local pass-through service (Claude, Codex, explicit provider list) | Implemented |
| Auto-detection | Implemented |
| Kiro plugin | Implemented |
| Configurable effort per provider, Claude thinking switch | Implemented (unreleased) |
| Generic loopback adapter | Archived; Kimi, Antigravity and OpenCode support removed |
| Distribution: GitHub release archives + npm package | Released (v0.2.0) |
| Module dispatcher: `pumice relay`, `pumice format`, `pumice transcribe` | Implemented (unreleased) |
| Transcription module `pumice-transcribe`: local files, HTTP, model downloads, runtime CPU variants | Implemented (unreleased, built from source) |
| Formatter module (`pumice format`) | Implemented (unreleased) |
| Whole stack: `pumice run`, `pumice serve` | Implemented (unreleased) |
| Windows Vulkan (GPU) transcription | Experimental preview, CI artifact only |
| `pumice setup` configuration wizard | Paused |
| Tauri installer, auto-update, start-with-the-system | Deferred |

## License

[MIT](LICENSE)
