# Decisions — p10-pumice-entry

### D1 · Active · Bare `pumice` prints the command overview until the stack orchestration exists
- date: 2026-10-15
- decided by: claude (orchestrator), brasillero (authorized)
- origin: p6-transcribe-http/reports/2026-10-15-implementation-order.md
- area: cli
- revisit if: step 5b (`orchestrator.yml` and managed components) lands

**Decision.** Running `pumice` with no subcommand prints the list of subcommands and exits 2; it no longer starts the relay. `pumice relay serve` starts the relay; `pumice relay` alone defaults to `relay serve`.

**Why.** `arch-modules/D6` makes bare `pumice` the orchestrator, and `D12` removes compatibility aliases; until the stack exists there is nothing for bare `pumice` to run, and silently starting the relay would hide the change.

**Rejected.** Keeping bare `pumice` as the relay; a hidden alias for `pumice --plain`.

### D2 · Active · `pumice transcribe` runs only the `pumice-transcribe` installed next to `pumice`
- date: 2026-10-15
- decided by: claude (orchestrator), brasillero (authorized)
- origin: arch-modules/D13
- area: cli
- revisit if: the orchestrator config (step 5b) adds an explicit executable path

**Decision.** `pumice transcribe …` executes `pumice-transcribe` from the same directory as the running `pumice` executable (resolved through the real executable path), forwarding arguments, stdio, exit status and termination signals. It never searches PATH or the working directory; if the file is missing it fails with a clear message saying transcription is not installed.

**Why.** The architecture plan forbids cwd and arbitrary PATH lookups for the trusted sibling executable; cargo installs both binaries in the same directory.

**Rejected.** PATH lookup; linking transcription into `pumice` (`arch-modules/D13`).

### D3 · Active · The relay reads only `relay.yml`
- date: 2026-10-15
- decided by: claude (orchestrator), brasillero (authorized)
- origin: arch-modules/D7
- area: config
- revisit if: —

**Decision.** The relay's default configuration file is `relay.yml` in the same per-user directory as before; `pumice.yaml` is no longer read and there is no fallback or migration command. The example file becomes `relay.example.yml`.

**Why.** `arch-modules/D7` names the file `relay.yml`; `D12` removes compatibility work because only the owner uses the configuration.

**Rejected.** Reading `pumice.yaml` as a fallback with a warning; a `pumice config migrate` command.

**Consequences.** Before deploying this version on server-1, the owner (or the supervisor) renames `~/.config/pumice/pumice.yaml` to `relay.yml` and changes the service to `pumice relay serve --plain`.
