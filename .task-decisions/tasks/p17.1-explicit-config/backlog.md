# Backlog — p17.1-explicit-config

## Active

## Deferred

## Closed

### #1 · Done · Drop the provider descriptors' built-in settings
- type: Tech debt
- area: providers
- origin: p17-serve-model-names/D3
- ticket: —
- created: 2026-10-20
- closed: 2026-10-23
- outcome: removed in `80254be` (branch `p16-provider-effort`): `ProviderDescriptor::defaults`, the per-provider `DEFAULT_TIMEOUT` and the `binary: None` fallback are gone, and `ProviderSettings::binary` is a plain path; `default_binary` stays

**Context.** Each provider descriptor still has a `defaults()` (`binary: None`, `timeout: DEFAULT_TIMEOUT` of 30 s, empty model), and each `build` falls back to its `DEFAULT_BINARY` when `binary` is `None`. Since D3 the loader writes every field from `relay.yml` (`binary` is always set), so these values only shape settings built in code: tests and the archived providers. Detection still uses `default_binary` for providers the file does not list, which is a probe, not a setting.

**To do.**
- [x] Decide whether `ProviderDescriptor::defaults` and the `binary: None` fallback go, or stay as test helpers.
- [x] Keep `default_binary` for detection of unlisted providers and for the `binary` hint in errors.

**Where in the code.**
- `crates/relay/src/providers/mod.rs` (`ProviderDescriptor::defaults`, `ProviderSettings::binary`)
- `crates/relay/src/providers/claude.rs`, `codex.rs`, `kimi.rs`, `kiro.rs` (`defaults`, `build`)

### #2 · Done · Fix the stale relay rename clause in AGENTS.md
- type: Alignment
- area: docs
- origin: p17-serve-model-names/D3
- ticket: —
- created: 2026-10-20
- closed: 2026-10-20
- outcome: authorized by the owner in conversation and removed in `e6b6664`

**Context.** While changing the authorized sentence in AGENTS.md's Configuration rule, the same rule still says "the relay still reads `pumice.yaml` until the rename lands". The rename has landed: the relay reads only `relay.yml` (CHANGELOG Unreleased, `docs/install.md` "Upgrading from 0.2").

**To do.**
- [x] Owner: drop "; the relay still reads `pumice.yaml` until the rename lands" from AGENTS.md.

**Where in the code.**
- `AGENTS.md` (Stack and constraints → Configuration)

### #3 · Done · Complete the owner's live configs before deploying
- type: Task
- area: config
- origin: p17-serve-model-names/reports/2026-10-20-implementation-design.md
- ticket: —
- created: 2026-10-20
- closed: 2026-10-21
- outcome: deployed after PR #125; all live settings and both services verified in p17-live-migration/reports/2026-10-21-live-migration.md

**Context.** The new binaries refuse a config that leaves a setting out, and both live services (`pumice.service` on 7567 and `pumice-serve`) read the owner's per-user files. The migration in the design report (step 4) covers `relay.yml`; the other three files need the same. This pull request never touched `~/.config/pumice`.

**To do.**
- [x] Back up the four YAML files and the binaries.
- [x] `relay.yml`: `port`, `total_timeout_secs`, `max_parallel`, `debug_log.enabled` and `debug_log.path`; on every provider `model` (`null` when disabled), `binary` (the command, e.g. `kiro-cli`) and `timeout_secs`.
- [x] `transcribe.yml`: `default_model`, `language`, `threads` (`auto` for the old default), `backend`, `server.port`, `models_dir` (`null` for the old default) and `models`.
- [x] `formatter.yml`: `formatters` and `default` (`null` for none).
- [x] `orchestrator.yml`: `modules.formatter`, every flow's `enabled` and four `budgets_ms`, and every `serve` key.
- [x] Validate each file with the candidate binaries' `check-config` (orchestrator: `pumice serve` startup), then restart both services and check both ports.

**Acceptance criteria.**
- Both services start with the new binaries and answer as before.

### #4 · Done · Owner: confirm that a one-off `file --model-file` runs without a config
- type: Alignment
- area: transcription
- origin: p17-serve-model-names/D3
- ticket: —
- created: 2026-10-20
- closed: 2026-10-20
- outcome: D1 records the owner's rejection of the exception; implemented in PR #124

**Context.** Requiring `transcribe.yml` for `file --model-file` broke the CI smoke jobs, which run one-off files with flags only (`.github/workflows/transcription.yml`, `release.yml`). The orchestrator read D3 as governing YAML settings: without any config file, `file --model-file PATH` runs from `--language`, `--threads` and `--backend` with their pre-D3 defaults (detect, `auto`, `cpu`); with a file (`--config` or the per-user one) it must be complete, as for every command. `--model ID` and `default_model` still need the file. The design report already kept `file --model-file PATH` "for one-off files".

**To do.**
- [x] Owner: keep this exception, or require a config file for `file` too (then the CI steps need a written `transcribe.yml`).

**Where in the code.**
- `crates/transcription/src/cli.rs` (`file_config`, `flag_defaults`)
- `docs/transcription.md` (`file`)
