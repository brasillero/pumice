# Backlog — s4.7-details-tabs

## Active

## Deferred

### #1 · Deferred · Headers are not shown in wire order
- type: Tech debt
- area: cli
- origin: conversation
- ticket: —
- created: 2026-10-08
- reason: accepted; revisit if wire order is needed

**Context.** The Received tab lists headers in `HeaderMap` order (values of one name grouped), not wire order; the HTTP library does not keep it. From the S4.7 review.

**To do.**
- [ ] Nothing unless wire order becomes necessary.

## Closed
