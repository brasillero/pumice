# Backlog — relay-only

## Active

## Deferred

## Closed

### #1 · Done · Restore the relay-only product and delivery
- type: Task
- area: architecture
- origin: D1
- ticket: —
- created: 2026-11-09
- closed: 2026-11-09
- outcome: PR #140; all 16 checks passed on the implementation head `645a426`; notes finished before merge

**Context.** T3 now supplies the transcription and post-processing the owner needs. The owner explicitly requested removing everything except the relay and making the relay Pumice again.

**To do.**
- [x] Move relay code and tests into the root Pumice package; remove the other module implementations.
- [x] Ship one `pumice` CLI and one initial settings file, preserving provider contracts.
- [x] Remove native engine, GPU, model-download and combined-flow packaging and CI.
- [x] Update current docs and project rules, and verify relay and packaging tests.
- [x] Open the PR and resolve checks for merge.

**Acceptance criteria.**
- The shipped product is the relay only, with its existing HTTP behavior and provider isolation intact.
- Local tests and cross-platform CI pass; release and npm packages carry only the relay.
