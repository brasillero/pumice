# Installing Pumice

Pumice is a local dictation-formatting service. It listens on
`http://127.0.0.1:7567/v1`, receives text transcribed by
[Handy](https://github.com/cjpais/Handy), formats it with an AI CLI you
already have installed and logged into, and returns the result to be pasted.

> **Distribution status:** the first GitHub release and the npm package are
> not published yet. This guide describes the standalone release archive,
> which needs nothing but the executable. An npm package (`pumice`, with
> per-platform `@brasillero/pumice-<os>-<arch>` native packages and a
> Node.js >= 22 launcher) is prepared in this repository and will provide
> `npm install -g pumice`, `pnpm add -g pumice`, `bun add -g pumice` and
> `npx pumice@latest` once the owner publishes it.

## Run the service

Extract this archive anywhere and start the service from the extracted
folder:

```
./pumice serve
```

On Windows PowerShell, run `.\pumice.exe serve` from the extracted folder.
To use `pumice` from any directory, put the executable in a directory on your
user PATH.

No installation step and no runtime are needed: the executable is
self-contained. It binds to `127.0.0.1` only and makes no outbound
connections of its own. Stop it with Ctrl-C.

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

In Handy, set the post-processing endpoint to `http://127.0.0.1:7567/v1`.

## Contents

- `pumice` (or `pumice.exe`) — the service executable
- `LICENSE` — MIT license
- `pumice.example.yaml` — commented reference configuration (optional)
