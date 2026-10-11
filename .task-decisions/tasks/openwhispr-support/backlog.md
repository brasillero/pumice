# Backlog — openwhispr-support

## Active

### #1 · To do · Make OpenWhispr requests work
- type: Improvement
- area: api
- origin: conversation
- ticket: —
- created: 2026-10-08

**Context.** OpenWhispr's dictation window is a browser context: it likely needs a restricted CORS/preflight policy before its requests succeed, and it retries 408/429/5xx up to 3 times. It first tries `/v1/responses` and falls back to chat completions on 404. See `~/pumice-research/client-protocols-2026-10-08.md` (outside the repo, unverified here).

**To do.**
- [ ] Check in the real desktop app.
- [ ] Add what it needs (CORS, maybe `/v1/responses`).

## Deferred

## Closed
