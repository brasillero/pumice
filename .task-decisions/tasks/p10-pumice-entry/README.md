---
title: P10: `pumice` entry point and `relay.yml`
state: Done
ticket: —
branch: p10-pumice-entry
mr: PR #102
start: 2026-10-15
end: 2026-10-15
---

`pumice` is now the entry point: `pumice relay …` runs the relay in-process (same parser as `pumice-relay`), `pumice transcribe …` execs the `pumice-transcribe` installed next to the real `pumice` executable (Unix exec, Windows spawn-and-wait), and bare `pumice` prints the command overview (D1–D4). The relay reads only `relay.yml`; `relay.example.yml` replaces the old example. README and `docs/install.md` give the upgrade steps for a systemd service. Implemented by Kimi k2.8-code; two Codex review rounds, final approve with nits. **Not deployed on server-1**: the live service still runs the 0.2 binary with `pumice --plain` and `pumice.yaml`; reinstalling without migrating would stop it.
