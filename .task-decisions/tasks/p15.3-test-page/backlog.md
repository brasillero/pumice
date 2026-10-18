# Backlog — p15.3-test-page

## Active

## Deferred

### #2 · Deferred · Read the upload before an early refusal so browsers see the error
- type: Improvement
- area: api
- origin: conversation
- ticket: —
- created: 2026-10-18
- reason: valid HTTP today; only the client-side message is affected

**Context.** When `pumice serve` refuses a request early (admission stopped, busy), it answers after reading only the request head and closes the connection. A client still uploading audio can then see a connection reset instead of the 503 or 429 (seen in Windows CI in PR #122, fixed there on the test side).

**To do.**
- [ ] Drain a bounded part of the body, or delay the close, before refusing, and test it from a slow uploader.

## Closed

### #1 · Done · Browser test page on `pumice serve`
- type: Task
- area: api
- origin: p15-serve-transcriptions#2
- ticket: —
- created: 2026-10-18
- closed: 2026-10-18
- outcome: PR #122 — page at `/` of `pumice serve`; live check over Tailscale recorded in the PR

**Context.** The owner wants to try the first version from an HTML page before T3. The page records or uploads audio, lists the models from `GET /v1/models` and calls `POST /v1/audio/transcriptions`, showing the final text and, when present, the raw transcript and timings. It replaces the planned P13.5 page for the dictation route.

**To do.**
- [ ] Serve the page from `pumice serve` and test it with the owner's flows on server-1.
