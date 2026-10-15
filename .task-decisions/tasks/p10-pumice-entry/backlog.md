# Backlog — p10-pumice-entry

## Active

## Deferred

### #1 · Deferred · Migrate the live server-1 service to the new command
- type: Task
- area: distribution
- origin: D3
- ticket: —
- created: 2026-10-15
- reason: the owner's friends use the live relay; the switch waits for the owner (`p6-transcribe-http/D10` lets the supervisor merge, not change the live service)

**Context.** Server-1's `pumice.service` runs `pumice --plain` with `~/.config/pumice/pumice.yaml`. After installing this version, it must run `pumice relay serve --plain` with `relay.yml`; port 7567 and the Handy URL stay the same.

**To do.**
- [ ] `mv ~/.config/pumice/pumice.yaml ~/.config/pumice/relay.yml`.
- [ ] Change `ExecStart=` in `~/.config/systemd/user/pumice.service` to `… relay serve --plain`, then `systemctl --user daemon-reload` and reinstall/restart.
- [ ] Check `/health` and a request through the public URL.

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
