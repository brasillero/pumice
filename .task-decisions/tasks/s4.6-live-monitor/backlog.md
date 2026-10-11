# Backlog — s4.6-live-monitor

## Active

## Deferred

### #1 · Deferred · Show where a request came from in the live view
- type: Improvement
- area: cli
- origin: conversation
- ticket: —
- created: 2026-10-08
- reason: when the owner schedules live-view work

**Context.** Show who called beyond the `X-Title` app name: local or through the public tunnel (Cloudflare adds `cf-ray`/`cf-connecting-ip`), the caller's IP (every request reaches Pumice from 127.0.0.1), possibly the `User-Agent`. The owner wants the IP in the table, shortened if long. Headers can be faked by local callers; fine for a debug view.

**To do.**
- [ ] Add an origin/IP column.

## Closed
