# Installing Pumice

Pumice is a small local tool that connects your dictation app to the AI
assistants you already use. It listens on `http://127.0.0.1:7567/v1`,
receives the prompt and the text transcribed by
[Handy](https://github.com/cjpais/Handy), passes them unchanged to an AI CLI
you already have installed and logged into, and returns the answer to be
pasted. The app's prompt decides what happens to the text.

> **Availability:** check
> [GitHub Releases](https://github.com/brasillero/pumice/releases) for the
> current published versions. For package-manager availability, see the
> [npm registry](https://www.npmjs.com/package/@brasillero/pumice). Install the
> `@brasillero/pumice` package with npm, pnpm, Bun or npx (Node.js >= 22 for the
> launcher; the executable itself is standalone). The installed command is `pumice`.

## Run the service

Extract this archive anywhere and start the service from the extracted
folder:

```
./pumice relay serve
```

On Windows PowerShell, run `.\pumice.exe relay serve` from the extracted folder.
To use `pumice` from any directory, put the executable in a directory on your
user PATH. (`pumice relay` with no further arguments is the same command, and
`pumice relay --config <path>` starts it with an explicit config file.)

No installation step and no runtime are needed: the executable is
self-contained. It binds to `127.0.0.1` only and makes no outbound
external connections of its own. The optional generic adapter connects only
to local services. Stop it with Ctrl-C.

## Configuration (optional)

Every setting has a built-in default, so a config file is not required. To
customize, copy `relay.example.yml` to the per-user config location and
edit it:

- Windows: `%APPDATA%\pumice\relay.yml`
- Linux and macOS: `~/.config/pumice/relay.yml`
  (or `$XDG_CONFIG_HOME/pumice/relay.yml`)

Run `./pumice relay check-config` (or `.\pumice.exe relay check-config` on Windows) to validate the file. Keep your config outside
this archive so upgrading is a simple folder replace.

## Point Handy at it

In Handy, set the post-processing endpoint to `http://127.0.0.1:7567/v1`
(Custom provider, API key empty, model `claude`, `codex` or `passthrough`).

## Contents

- `pumice` (or `pumice.exe`) — the service executable
- `LICENSE` — MIT license
- `THIRD_PARTY_NOTICES.md` — license notices for adapted third-party content
  in the executable (the Handy-derived built-in formatter prompt)
- `relay.example.yml` — commented reference configuration (optional)
