# Pumice

Use the AI subscriptions you already have from any OpenAI-compatible client.

Pumice is a small local relay for Claude Code, Codex and Kiro. Your app sends its prompt and text to Pumice; Pumice passes them to the selected official CLI and returns the answer in OpenAI format. The app decides what to do with the text. No separately billed API key is needed.

```text
Client → http://127.0.0.1:7567/v1 → Pumice → official AI CLI
Client ← OpenAI chat completion ← Pumice ← final answer
```

- Works with [Handy](https://github.com/cjpais/Handy) and other OpenAI-compatible clients.
- One executable for Windows x64, Linux x64 and macOS Apple Silicon; npm provides an optional Node.js launcher.
- Listens on IPv4 loopback only, with no telemetry.
- Runs the selected provider with restricted tools in a fresh temporary directory.
- Returns an HTTP error on failure; there is no provider fallback.
- Offers a live request view, plain logs, configuration validation and provider diagnostics.

## Install and run

Install a supported official AI CLI and log in to it yourself, then install Pumice from a [release archive](https://github.com/brasillero/pumice/releases) or with Node.js 22+:

```sh
npm install -g @brasillero/pumice
pumice doctor
pumice
```

`pumice` starts the relay; `pumice serve` is an alias. Use `pumice --plain` for one-line stderr logs. Stop with Ctrl-C.

The first start creates **one** settings file, `pumice.yml`, from [pumice.example.yml](pumice.example.yml): `%APPDATA%\pumice\pumice.yml` on Windows, or `~/.config/pumice/pumice.yml` on Linux/macOS (`$XDG_CONFIG_HOME` is respected). Existing files are preserved. Edit the file to select providers and models; every setting must be written explicitly. `pumice check-config` validates it. Use `--config PATH` for an existing file at another location.

For Handy's Custom post-processing provider, use base URL `http://127.0.0.1:7567/v1`, leave the API key empty, and select `claude`, `codex` or `kiro`. `passthrough` returns the transcript without an AI call; `inspect` echoes the request JSON for diagnostics.

**Upgrading:** rename your previous `relay.yml` (or `pumice.yaml`) to `pumice.yml` before starting, and change service commands to `pumice --plain`. See [the migration steps](docs/install.md#upgrading).

## Build from source

With stable Rust on Linux:

```sh
cargo test --locked
cargo build --release --locked --bin pumice
./target/release/pumice doctor
./target/release/pumice
```

## Documentation

- [Installation, configuration, live view and releases](docs/install.md)
- [HTTP API and captured Handy contract](docs/api.md)
- [Provider behavior, limits and evidence](docs/providers.md)
- [Task decisions](.task-decisions/README.md)
- [Contributor and agent rules](AGENTS.md)

## License

[MIT](LICENSE)
