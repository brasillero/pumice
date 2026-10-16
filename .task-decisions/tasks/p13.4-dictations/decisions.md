# Decisions — p13.4-dictations

### D1 · Active · A client that half-closes its connection is treated as gone
- date: 2026-10-15
- decided by: claude (orchestrator, owner-delegated 2026-10-15)
- origin: conversation
- area: api
- revisit if: a real dictation client needs to half-close (`shutdown(SHUT_WR)`) after sending its request

**Decision.** On `POST /pumice/v1/dictations`, end of input from the client (a FIN, including a deliberate write-half-close after a complete request) cancels the operation with `client_disconnected`, like a reset. Half-close is not supported and is documented as such. This narrows `p13.1-flow-seam/reports/2026-10-15-p13-design.md` §6, which asked to preserve legitimate write-half-close.

**Why.** On the server side a half-close and a client that closed or was killed (a closed browser tab, a killed `curl`) look the same until the response is written. Supporting half-close would keep running the transcription and the paid AI call for clients that are gone. Browsers and dictation apps do not half-close.

**Rejected.** Enabling hyper's half-close support (Codex P13.4 review round 1, finding 2): it would detect only resets as disconnects.
