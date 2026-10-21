# Backlog — p17-serve-model-names

## Active

### #2 · In progress · Implement the served model ids and explicit settings in Pumice
- type: Improvement
- area: api
- origin: D1
- ticket: —
- created: 2026-10-20

**Context.** D1 to D4 replace `serve.transcription_flows`. Raw transcription must work with the relay off, which today still builds the relay pipeline (`src/orchestrator/flow.rs`, `Runtime::build`).

**To do.**
- [x] Owner: which models folder (D7).
- [x] Design the YAML (expansion options, default relay and preset, global relay timeout) with Codex.
- [x] Accept percent-encoded ids in `GET /v1/models/{id}`.
- [x] Update AGENTS.md's built-in defaults sentence (D3; done in p17.1-explicit-config).

**Where in the code.**
- `src/orchestrator/config.rs`, `src/orchestrator/flow.rs`, `src/orchestrator/serve/`
- `crates/transcription/src/catalog.rs`, `crates/transcription/src/config/`

## Deferred

## Closed

### #1 · Done · Make the T3 voice demo generic
- type: Task
- area: api
- origin: D6
- ticket: —
- created: 2026-10-20
- closed: 2026-10-21
- outcome: generic demo complete on the local-only pumice-voice-demo branch at 1b8b9391; all checklist steps were completed and http://127.0.0.1:5733/ returned 200 on 2026-10-21

**Context.** The demo lives outside this repository (`~/scratch/t3/t3code`, local branch `pumice-voice-demo`, never pushed). D6 sets what it must become.

**To do.**
- [x] Optional API key, server-side, Bearer only when set.
- [x] Searchable model list with "Use '<typed id>'".
- [x] Remove Pumice-specific parsing, warning and texts; update DEMO.md.
- [x] Codex review (approved after 4 rounds: key redaction, safe logs, untraced upstream calls), then restart the running demo.
