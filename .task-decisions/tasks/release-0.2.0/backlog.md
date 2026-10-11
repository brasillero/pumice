# Backlog — release-0.2.0

## Active

## Deferred

### #1 · Deferred · Decide on changelog tooling
- type: Alignment
- area: process
- origin: conversation
- ticket: —
- created: 2026-10-11
- reason: waiting for the owner's decision

**Context.** `CHANGELOG.md` is written by hand. git-cliff could generate it from commits; release-plz could also open the version-bump PR. Squash-merge titles such as `S4.8: landing page at /` are not Conventional Commits, so the PR title format in `AGENTS.md` (owner-edited) would change, e.g. to `feat(S4.8): landing page at /`. Generated entries are one line per change instead of the current prose.

**To do.**
- [ ] Owner decides on the tool and the title format.

## Closed
