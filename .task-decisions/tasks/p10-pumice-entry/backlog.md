# Backlog — p10-pumice-entry

## Active

## Deferred

### #2 · Deferred · Windows Ctrl-C handling for `pumice transcribe`
- type: Tech debt
- area: cli
- origin: D4
- ticket: —
- created: 2026-10-15
- reason: review round 2 nits; needs a real Windows console to test

**Context.** On Windows `pumice` registers a console control handler while it waits for `pumice-transcribe`. Review round 2 suggested removing the handler explicitly before exit and on error returns, and adding an isolated Windows test where the child delays its cleanup after Ctrl-C.

**To do.**
- [ ] Remove the handler explicitly on every return path.
- [ ] Add a Windows Ctrl-C test with a slow-cleanup stub.

## Closed

### #1 · Done · Migrate the live server-1 service to the new command
- type: Task
- area: distribution
- origin: D3
- ticket: —
- created: 2026-10-15
- closed: 2026-10-15
- outcome: done on the owner's request; steps and checks in `reports/2026-10-15-server-1-migration.md`

**Context.** Server-1's `pumice.service` runs `pumice --plain` with `~/.config/pumice/pumice.yaml`. After installing this version, it must run `pumice relay serve --plain` with `relay.yml`; port 7567 and the Handy URL stay the same.

**To do.**
- [x] `mv ~/.config/pumice/pumice.yaml ~/.config/pumice/relay.yml`.
- [x] Change `ExecStart=` in `~/.config/systemd/user/pumice.service` to `… relay serve --plain`, then `systemctl --user daemon-reload` and reinstall/restart.
- [x] Check `/health` and a request through the public URL.

### #3 · Done · The documented update command installs the test helper binaries
- type: Bug
- area: distribution
- origin: reports/2026-10-15-server-1-migration.md
- ticket: —
- created: 2026-10-15
- closed: 2026-10-16
- outcome: PR #111 (`chore-small-fixes`): the test helpers need a `test-helpers` feature, so `cargo install` skips them

**Context.** `cargo install --path . --locked`, the update command for a source install, also installs `pumice-test-cli` and `pumice-test-transcribe` from the root package into `~/.cargo/bin`. Unverified: whether the release archives and npm package are affected (they are built by CI, not by `cargo install`).

**To do.**
- [x] Keep the test helpers out of `cargo install` (for example `required-features`, or a separate test-only package), or document `--bin pumice`.

### #4 · Done · `--help` after a `pumice relay` subcommand is rejected
- type: Bug
- area: cli
- origin: reports/2026-10-15-server-1-migration.md
- ticket: —
- created: 2026-10-15
- closed: 2026-10-16
- outcome: PR #111 (`chore-small-fixes`): `--help`/`-h` after each relay subcommand prints its help

**Context.** `pumice relay serve --help` and `pumice relay doctor --help` print `error: unexpected argument '--help'` and the usage line; `pumice relay check-config --help` prints only an error with the usage. Checked on `5ab3cf3`.

**To do.**
- [x] Accept `--help` after each `relay` subcommand and print its help.
