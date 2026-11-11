# Installing Pumice

Pumice connects OpenAI-compatible clients to official Claude Code, Codex and Kiro CLIs. Install and log into a supported CLI yourself.

Supported platforms are Windows x64, Linux x64 and macOS Apple Silicon.

Extract the archive and run `./pumice` on Linux/macOS or `.\pumice.exe` in Windows PowerShell. Add the executable to your PATH to run `pumice` from anywhere. `pumice serve` is an alias; `--plain` uses one-line stderr logs. Stop with Ctrl-C.

The service listens on `http://127.0.0.1:7567/v1`. In Handy, choose the Custom post-processing provider, that base URL, an empty API key, and an enabled model (`claude`, `codex` or `kiro`). The client owns the prompt.

## Settings

First start creates `pumice.yml` automatically:

- Windows: `%APPDATA%\pumice\pumice.yml`
- Linux/macOS: `~/.config/pumice/pumice.yml`, or `$XDG_CONFIG_HOME/pumice/pumice.yml`

Edit it to select providers and models. Every setting is required. Existing files are never overwritten. `pumice check-config` validates the file and reports errors with the file and line. `pumice --config PATH` reads an existing file elsewhere. `pumice doctor` reports installed CLIs without spending quota or creating settings.

Before upgrading, rename an existing `relay.yml` or `pumice.yaml` to `pumice.yml`, and change service commands to `pumice --plain`. Keep settings outside the extracted archive.

## Contents

- `pumice` or `pumice.exe`: self-contained service executable
- `LICENSE`: Pumice's MIT license
- `THIRD_PARTY_NOTICES.md`: dependency, Rust standard library and runtime notices for this target
- `pumice.example.yml`: complete reference configuration
- `INSTALL.md`: this guide

Full documentation: https://github.com/brasillero/pumice/blob/main/docs/install.md
