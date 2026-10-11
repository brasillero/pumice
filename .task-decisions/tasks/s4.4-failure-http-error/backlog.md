# Backlog — s4.4-failure-http-error

## Active

## Deferred

### #1 · Deferred · Make error bodies match OpenAI's exactly
- type: Alignment
- area: api
- origin: conversation
- ticket: —
- created: 2026-10-08
- reason: waiting for the owner's decision

**Context.** Error responses carry only `message` and a status-derived `type`. Research (`~/pumice-research/openai-conformance-2026-10-08.md`, outside the repo, unverified here) found gaps: always serialize `param` and `code` (null when unknown), `param: "model"` for a missing or unknown model, `code: "model_not_found"` on 404, and quota as `insufficient_quota`. OpenAI's Chat API documents 500 and 503 but not 502/504. Was the BACKLOG item "OpenAI error body conformance".

**To do.**
- [ ] Add `param`/`code` and the codes above (no decision needed).
- [ ] Owner decides: keep 502/504 or move internal failures to 500.

**Where in the code.**
- `src/api/log_entry.rs` (`failure_status`), `src/api/types.rs`

### #2 · Deferred · Handy repeats a request that got HTTP 400
- type: Tech debt
- area: api
- origin: conversation
- ticket: —
- created: 2026-10-11
- reason: accepted; revisit if Handy changes its retry

**Context.** When Handy sent reasoning fields (it sends `reasoning_effort: "none"`), it retries a request that gets HTTP 400 or 422 once more without them. A Pumice 400 (no model, malformed request) therefore shows up twice in the log. Harmless. Was a BACKLOG known limitation.

**To do.**
- [ ] Nothing unless Handy changes or the double log entry becomes a problem.

## Closed
