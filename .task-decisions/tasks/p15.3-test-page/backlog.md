# Backlog — p15.3-test-page

## Active

## Deferred

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
