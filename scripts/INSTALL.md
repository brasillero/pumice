# Installing Pumice

Pumice is a local dictation-formatting service. It listens on
`http://127.0.0.1:7567/v1`, receives text transcribed by
[Handy](https://github.com/cjpais/Handy), formats it with an AI CLI you
already have installed and logged into, and returns the result to be pasted.

> **Availability:** check
> [GitHub Releases](https://github.com/brasillero/pumice/releases) for the
> current published versions. For package-manager availability, see the
> [npm registry](https://www.npmjs.com/package/pumice). When available, install
> `pumice` with npm, pnpm, Bun or npx (Node.js >= 22 for the launcher; the
> executable itself is standalone).

## Run the service

Extract this archive anywhere and start the service from the extracted
folder:

```
./pumice
```

On Windows PowerShell, run `.\pumice.exe` from the extracted folder.
To use `pumice` from any directory, put the executable in a directory on your
user PATH. (`pumice serve` is an explicit alias for the same command, and
`pumice --config <path>` starts it with an explicit config file.)

No installation step and no runtime are needed: the executable is
self-contained. It binds to `127.0.0.1` only and makes no outbound
external connections of its own. The optional generic adapter connects only
to local services. Stop it with Ctrl-C.

## Configuration (optional)

Every setting has a built-in default, so a config file is not required. To
customize, copy `pumice.example.yaml` to the per-user config location and
edit it:

- Windows: `%APPDATA%\pumice\pumice.yaml`
- Linux and macOS: `~/.config/pumice/pumice.yaml`
  (or `$XDG_CONFIG_HOME/pumice/pumice.yaml`)

Run `./pumice check-config` (or `.\pumice.exe check-config` on Windows) to validate the file. Keep your config outside
this archive so upgrading is a simple folder replace.

## Point Handy at it

In Handy, set the post-processing endpoint to `http://127.0.0.1:7567/v1`
(Custom provider, API key empty, model `claude`, `codex` or `passthrough`).

## Contents

- `pumice` (or `pumice.exe`) — the service executable
- `LICENSE` — MIT license
- `pumice.example.yaml` — commented reference configuration (optional)
