# Backlog — release-0.4.0

## Active

## Deferred

### #1 · Deferred · Restrict relay Host and Origin headers
- type: Tech debt
- area: api
- origin: D2
- ticket: —
- created: 2026-11-16
- reason: accepted as debt in D2; revisit when the owner supplies the allowed tunnel hosts and browser origins

**Context.** The router accepts arbitrary Host and Origin headers. Loopback binding alone does not prevent browser-triggered calls. Existing clients and the public tunnel must keep working for this release.

**To do.**
- [ ] Obtain allowed hostnames and browser origins from the owner.
- [ ] Implement and fake-CLI test the selected policy with explicit configuration.

**Acceptance criteria.**
- Unapproved browser requests cannot spend CLI quota; approved clients and tunnel requests work.

**Where in the code.**
- `src/api/mod.rs`, `src/api/handlers.rs`

## Closed
