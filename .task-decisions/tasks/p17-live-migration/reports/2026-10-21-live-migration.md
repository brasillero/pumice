# P17 live migration on server-1

Completed on 2026-10-21 after the three implementation PRs merged: explicit
settings (PR #123), models-folder discovery and required file-transcription
settings (PR #124), and served model names (PR #125).

PR #125 merged as `57c4b1ec9b6ee38e23c59ad21e8aa0bd1b258793`, after all 24
checks passed on `c02a4aa5b8791146802d93ca9e92de654569026e`. Its parent and
independent code reviews approved the implementation.

## Installed artifacts and backups

Installed the portable Linux musl core and matching Linux x64 transcription
bundle from successful Release preview run
[39545862112](https://github.com/brasillero/pumice/actions/runs/39545862112).
The core archive matched its aggregate SHA-256 checksum; all 19 transcription
bundle files matched their manifest sizes and checksums. The CPU diagnostic
reported a verified bundle and selected the `alderlake` CPU variant.

Updated `~/.cargo/bin/pumice` and both module installations:
`~/.local/share/pumice-serve/` and `~/.local/share/pumice-transcribe/`.
The existing `~/.cargo/bin/pumice-transcribe` symlink remains valid. The core,
complete old module directories and four old settings files are backed up in:

```text
~/.local/share/pumice/backups/p17-20261021T051037Z/
```

That directory also holds the installed settings snapshots and deployment
metadata. The replaced module directories remain beside their replacements
as `.pumice-serve-before-p17-20261021T051037Z` and
`.pumice-transcribe-before-p17-20261021T051037Z`. No rollback was needed.

## Settings and models

Completed the four per-user module files under `~/.config/pumice/`:

- Relay: retained the enabled providers, models, 60-second timeouts and
  explicitly enabled debug log; wrote the old port, concurrency, executable
  names and log path explicitly. No CLI user settings or credentials changed.
- Transcription: selected `whisper-large-v3-turbo-q8_0`, retained Portuguese,
  wrote CPU, automatic threads and nullable server port explicitly, and used
  `models_dir: null` for the platform models folder. The obsolete `models:`
  alias list was removed rather than carried into the completed settings.
- Formatter: retained the existing `clean` and `clean-en` definitions and
  default, with their paths resolved to the original files.
- Orchestrator: retained all three dictation flows and the allowed browser
  origin, replaced the `turbo` alias with the catalog id, and wrote flow and
  serve budgets explicitly. `serve.relay` uses Codex, defaults to `clean`,
  lists `clean` and `clean-en`, and expands presets to preserve both choices.
  Provider expansion remains off. Module references point to the live files.

Moved both `.gguf` files and `SHA256SUMS` from `~/pumice-models/` to
`~/.local/share/pumice/models/`, with no destination overwrite. Both model
checksums passed before and after the move. The old directory is empty.
Installed model discovery reports both catalog identities without diagnostics:

```text
whisper-large-v3-turbo-q8_0  886381760 bytes  default
whisper-tiny-q8_0            45981088 bytes
```

## Verification

Before the switch, the downloaded binaries started a candidate server on an
ephemeral loopback port against the existing model files. It became ready,
returned the expected model list and shut down by its own PID. No test server
used either live port.

The installed relay, formatter and transcription `check-config` commands
passed. The orchestrator loaded the completed live settings and became ready.
Both `pumice.service` (7567) and `pumice-serve.service` (17570) are active;
`pumice-tunnel.service` stayed active throughout with its original process.
No authentication, Cloudflare account, DNS or tunnel configuration changed.

The serve endpoint lists these three ready models, and each exact,
percent-encoded model-detail lookup succeeds:

```text
whisper-large-v3-turbo-q8_0
whisper-large-v3-turbo-q8_0:codex:clean
whisper-large-v3-turbo-q8_0:codex:clean-en
```

Using the repository's Portuguese audio fixture, one raw request returned
nonempty text with post-processing `off`; one formatted request returned
nonempty text with post-processing `applied`. This used one real AI CLI call.
The installed `pumice transcribe models list` wrapper also succeeded. The
built-in browser test page returned 200, and a curl request to the unchanged
public relay URL returned 200 with Claude, Codex and Kiro available.

Existing clients that saved a former flow model id (`raw`, `clean` or
`clean-en`) must select its new model id from the refreshed list. Dictation
flow ids are unchanged. Initial settings-file creation is a separate owner
question recorded in p17.1-explicit-config/D1; this deployment supplied
complete files and does not implement automatic initialization.
