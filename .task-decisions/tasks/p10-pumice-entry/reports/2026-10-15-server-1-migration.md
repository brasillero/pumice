# Server-1 migration to `pumice relay serve` — 2026-10-15

Done by the orchestrator (Claude) on the owner's request ("also can migrate all necessary configuration"), from `main` at `5ab3cf3`.

## Steps

1. Built `main` into a staging root (`cargo install --path . --locked --root ~/scratch/pumice-new`) and ran `pumice relay check-config` on a copy of the live `pumice.yaml` renamed to `relay.yml`: valid, unchanged contents, port 7567, providers claude (haiku), codex (gpt-6-luna), kiro (glm-5), kimi disabled.
2. Ran the staged binary on test port 17570 with a separate config (debug log off): `/health` ok, `/v1/models` lists the same providers, one real `claude` request answered `ok` in 2.1 s. Stopped it by PID.
3. Backed up the unit, `pumice.yaml` and both old binaries to `~/scratch/live-backup-2026-10-15/` on server-1.
4. Installed `pumice` (`--bin pumice` only) and `pumice-transcribe` into `~/.cargo/bin`.
5. `mv ~/.config/pumice/pumice.yaml ~/.config/pumice/relay.yml`; only the header comment changed (it pointed at `pumice.example.yaml`). The debug log file name is unchanged (`pumice-debug.jsonl`).
6. Unit: `ExecStart=%h/.cargo/bin/pumice relay serve --plain`; `systemctl --user daemon-reload && systemctl --user restart pumice`. Service active and enabled.
7. Through the public URL: `/health` returned `{"status":"ok","version":"0.2.0"}` and one real `claude` request answered `ok` in 2.0 s. The tunnel service was not touched.

`~/.config/pumice/transcribe.yml` already existed and passes `pumice-transcribe check-config` with the new binary. No `formatter.yml` or `orchestrator.yml` was created: nothing on server-1 uses them, and their rules and flows are the owner's to choose.

## Rollback

Copy the backed-up binaries back to `~/.cargo/bin`, restore `pumice.service` and `pumice.yaml` from the backup folder, then `systemctl --user daemon-reload && systemctl --user restart pumice`.

## Found on the way

- `cargo install --path . --locked` (the documented update command) also installs the test helpers `pumice-test-cli` and `pumice-test-transcribe` into `~/.cargo/bin` (#3).
- `--help` after a `pumice relay` subcommand (`serve`, `doctor`, `check-config`) is rejected; `pumice relay --help` works (#4).
