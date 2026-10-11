# Decisions — chore-task-decisions-import

### D1 · Active · Task notes are committed, one task per branch
- date: 2026-10-11
- decided by: brasillero
- origin: conversation
- area: process
- revisit if: —

**Decision.** Notes live in `.task-decisions/`, committed with the code and following the task-decisions skill: one task per branch and pull request, named after the branch. Open work without a branch goes into the task it came from, or into a new task for the planned work.

**Why.** Owner request in conversation: replace `HANDOFF.md` and `BACKLOG.md` with this format.

**Rejected.** Local-only notes; one task per project phase.

### D2 · Active · `.task-decisions/` replaces HANDOFF.md and BACKLOG.md
- date: 2026-10-11
- decided by: brasillero
- origin: #1
- area: docs
- revisit if: —

**Decision.** `HANDOFF.md` and `BACKLOG.md` are removed; plans, decisions and open work live only in `.task-decisions/`, and `AGENTS.md` says so.

**Why.** The owner checked the import (PR #88) and found everything migrated.

**Rejected.** Keeping both, which would drift apart.

**Consequences.** The owner asked an agent to make the `AGENTS.md` edit (PR #89), a one-time exception to its owner-edited rule.
