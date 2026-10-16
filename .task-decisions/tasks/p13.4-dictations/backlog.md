# Backlog — p13.4-dictations

## Active

## Deferred

### #1 · Deferred · The browser test page uses `pumice serve`'s dictation route
- type: Task
- area: api
- origin: conversation
- ticket: —
- created: 2026-10-15
- reason: separate PR (P13.5), planned in `p13.1-flow-seam/D2`

**Context.** `tools/transcribe-test/` talks to `pumice-transcribe serve` only. With `POST /pumice/v1/dictations` it can show the raw transcript and the final text side by side with a flow picker. Design §9 and `p13.1-flow-seam/D8`: the page's same-origin helper validates its own origin and strips it before forwarding to `http://127.0.0.1:17570`; its timeout must cover the flow total plus a margin; no Tailscale or tunnel exposure is added.

**To do.**
- [ ] Helper allowlist for `/pumice/v1/dictations`, `/pumice/v1/flows`, `/pumice/v1/status` with the origin check and strip.
- [ ] Flow picker, raw vs final display, failure bodies (transcript kept on non-2xx).
- [ ] Timeout and cancel behavior checked end to end.


## Closed
