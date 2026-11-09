# Backlog — relay-only

## Active

### #1 · In progress · Restore the relay-only product and delivery
- type: Task
- area: architecture
- origin: D1
- ticket: —
- created: 2026-11-09

**Context.** T3 now supplies the transcription and post-processing the owner needs. The owner explicitly requested removing everything except the relay and making the relay Pumice again.

**To do.**
- [ ] Move relay code and tests into the root Pumice package; remove the other module implementations.
- [ ] Ship one `pumice` CLI and one initial settings file, preserving provider contracts.
- [ ] Remove native engine, GPU, model-download and combined-flow packaging and CI.
- [ ] Update current docs and project rules, and verify relay and packaging tests.
- [ ] Open the PR, resolve checks and merge.

**Acceptance criteria.**
- The shipped product is the relay only, with its existing HTTP behavior and provider isolation intact.
- Local tests and cross-platform CI pass; release and npm packages carry only the relay.

## Deferred

## Closed
