# Task decisions — shapes

The exact shape of every file in `.task-decisions/`. Read the section for the file you are about to write; the general rules, lifecycle, IDs and origins are in `rules.md`.

- [Project `README.md`](#project-readmemd)
- [Task `README.md`](#task-readmemd)
- [`backlog.md`](#backlogmd): file shape, open items, closed items, moves
- [`decisions.md`](#decisionsmd): file shape, decisions, superseding, revoking, debt
- [`reports/`](#reports)

---

## Project `README.md`

```markdown
---
name: Shop Web
language: en-US
tracker: Jira
prefix: ABC
areas: checkout, catalog, process
---

What the project is, in a few lines.

## Vocabulary

- **order**: …
- **cart**: …
```

| Field | Values |
|---|---|
| `name` | display name of the project; starts as the repository name with `-` and `_` turned into spaces and each word capitalised (`shop-web` → `Shop Web`); the user may change it |
| `language` | language of the entries' text (`en-US`, `pt-BR`…) |
| `tracker` | where tasks live (`Jira`, `GitHub`, `GitLab`…) or `—` |
| `prefix` | prefix of the tracker's keys (`ABC`) or `—` |
| `areas` | the accepted values of the entries' `area` field, comma-separated |

- **Vocabulary:** the project's terms that entries should use.
- **Tracker template** (optional section `## Tracker template`): the language and structure of text written for the tracker. Without it, write the tracker text in the project's `language` with the sections Title, Context, To do, Acceptance criteria, Where in the code.
- **Commits** (optional section `## Commits`): `Commit notes changes as they happen.` to have agents commit each change (rule 5 in `rules.md`), or `Local only.` when the notes are not committed (see "First use in a repository" in `rules.md`).
- **Field names and fixed values** (states, types, section names) in the notes are always the ones in this file, in English, whatever the project's language. Only the free text follows `language`. Text generated for the tracker follows the `## Tracker template` instead (its language and its section names).
- **New area:** add it to the project's `README.md` first, with the user's agreement.

---

## Task `README.md`

```markdown
---
title: New payment flow
state: In progress
ticket: ABC-123
branch: ABC-123
mr: !42
start: 2026-01-10
end: —
---

Summary in a few lines: what the task is and where it stands now.
```

| Field | Values |
|---|---|
| `title` | short text |
| `state` | `In progress` · `Paused` · `Done` · `Abandoned` |
| `ticket` | key in the tracker or `—` |
| `branch` | branch name or `—` |
| `mr` | the merge/pull request (`!42`, `PR #42` or a URL) or `—` |
| `start`, `end` | `YYYY-MM-DD` or `—` |

The summary describes **now**, not the history: the history is in the backlog and the decisions.

---

## `backlog.md`

What is left to do in the task. Items change state and section over time.

### File shape

```markdown
# Backlog — ABC-123

## Active

### #1 · To do · Validate the coupon before closing the order
…

## Deferred

### #3 · Deferred · Merge the two address forms
…

## Closed

### #2 · Done · Fix the total with free shipping
…
```

- The three sections always exist, in this order, even when empty.
- **Active:** `To do`, `In progress`, `Blocked`.
- **Deferred:** `Deferred`.
- **Closed:** `Done`, `Discarded`, `Moved`.
- Within each section, ascending number order.
- Changing an item's state = changing its heading **and** moving it to the right section.

### Shape of an open item (Active and Deferred)

```markdown
### #3 · Deferred · Merge the two address forms
- type: Tech debt
- area: checkout
- origin: D2
- ticket: —
- created: 2026-01-11
- reason: waiting for the checkout redesign

**Context.** Why this exists, in a few sentences.

**To do.**
- [ ] Steps or decisions to make.
- [x] A step already done.

**Acceptance criteria.**
- How to know it is finished.

**Where in the code.**
- `path/to/file.ts` (`symbol`)
```

| Field | Values | Required |
|---|---|---|
| state (in the heading) | `To do` · `In progress` · `Blocked` · `Deferred` | yes |
| `type` | `Bug` · `Improvement` · `Tech debt` · `Alignment` · `Task` · `Process` | yes |
| `area` | one of the `areas` in the project's `README.md` | yes |
| `origin` | see "Origin rule" in `rules.md` | yes |
| `ticket` | key in the tracker or `—` | yes |
| `created` | `YYYY-MM-DD` | yes |
| `reason` | short text | in `Blocked` (what blocks it) and `Deferred` (why, and until when: a checkable trigger such as an event, a ticket or a date, not only "with #n") |

- **Body:** `Context` and `To do` always; `Acceptance criteria` and `Where in the code` when they apply. In this order.
- **Length:** 10 to 25 lines. Larger material goes to `reports/` and the item links to it.
- **Tech debt is a type, not a state:** debt accepted for now is `Deferred`; debt being paid is `In progress`.

### Shape of a closed item

A closed item **keeps its full text**, so whoever follows a reference to it (a later item, another task) still reads its context. Closing it means:

1. Change the state in the heading and move the entry to the Closed section.
2. Remove `reason` (it explained the open state) and add `closed` and `outcome` at the end of the fields.
3. Leave the body as it was; tick the checklist steps that were done.

```markdown
### #2 · Done · Fix the total with free shipping
- type: Bug
- area: checkout
- origin: conversation
- ticket: ABC-123
- created: 2026-01-11
- closed: 2026-01-12
- outcome: fixed in `a1b2c3d`

**Context.** …
```

| Field | Values | Required |
|---|---|---|
| state (in the heading) | `Done` · `Discarded` · `Moved` | yes |
| `closed` | `YYYY-MM-DD` | yes |
| `outcome` | `Done`: evidence: a `commit`, a merge/pull request (`!42`, `PR #42`, `MR #42` or a URL; a bare `#42` is an item), or an existing entry or report that records the outcome (e.g. `D12`); more explanation may follow. `Discarded`: a short reason. `Moved`: `→ ABC-200#1`, the item in the other task | yes |

The other fields and the body follow "Shape of an open item".

**A closed item is final.** After closing, its fields and body are never changed again. If the work comes back (a bug returns, a discarded idea is wanted again), create a new item with `origin: #n`; never reopen the closed one.

**Item moved to another task:** in the new task, create an item with `origin: ABC-123#5`; in the old one, close it as `Moved`, pointing to the new one (it keeps its text as it was when moved; the new item is where it continues).
- The new item keeps `type`, `area`, `created` (the original date) and the body; it takes the destination's next number, a state that fits the destination, and `ticket` as known there.
- If the destination project does not accept the item's `area`, ask the user whether to add that area to the project or which of its areas to use.
- References inside the moved text that were local to the old task (`#6`) become full references (`ABC-123#6`).

---

## `decisions.md`

What was decided in the task. **Append only**: old decisions are not rewritten.

### File shape

```markdown
# Decisions — ABC-123

### D1 · Active · …

### D2 · Superseded by D5 · …
```

- Entries in ascending number order (chronological), with no sections.
- The only change allowed on an old decision is the **state in its heading**.
- **Changed your mind, or confirmed a Provisional decision?** Write a new decision with `origin: D<old>` and change the old one's state to `Superseded by D<new>`.
- **Active or Provisional:** `Provisional` waits for a confirmation of a fact (named in `revisit if`); `Active` is settled, even if it names a condition for revisiting it.
- **After superseding or revoking,** look for open items whose `origin` is the old decision and ask the user whether they are now done or no longer needed. Nothing closes on its own.

### Shape of a decision

```markdown
### D2 · Provisional · Address validated in the browser, not on the server
- date: 2026-01-11
- decided by: alex-m
- origin: reports/2026-01-11-address-validation.md
- area: checkout
- revisit if: the postcode service answers in under 200 ms

**Decision.** One or two sentences: what now holds.

**Why.** The reason, briefly.

**Rejected.** Each alternative considered and why it was not chosen. May be `—`.

**Consequences.** Optional: what changes, or what is now needed.
```

| Field | Values | Required |
|---|---|---|
| state (in the heading) | `Active` · `Provisional` · `Superseded by D<n>` · `Revoked` | yes |
| `date` | `YYYY-MM-DD` | yes |
| `decided by` | who decided, comma-separated: people by their handle on the platform that hosts the repository (the current user's from `gh api user -q .login` / `glab api user` → `username`, anyone else's by asking the user; without a remote, the git user name), groups by their usual name (`backend team`, `team`). Ask if the request does not say | yes |
| `origin` | see "Origin rule" in `rules.md` | yes |
| `area` | one of the `areas` in the project's `README.md` | yes |
| `revisit if` | a concrete condition or `—` | yes; `Provisional` requires a condition |

- **The title states the decision, not the topic:** "Address validated in the browser", not "Address validation". Write it timelessly: no "for now", "later" or "yet", since the title cannot change once written.
- **Temporary choices are decisions.** A choice made "for now", until someone confirms it, is a `Provisional` decision whose `revisit if` names that confirmation. Do not leave it only in an item's text.
- **`revisit if` is concrete:** a condition that can be checked, not "in the future".
- **Revoked:** a decision dropped with nothing in its place. Change its state to `Revoked` and record why in a new decision with `origin: D<n>` (the revoked one's body is not changed).
- **A decision that accepts debt:** record the decision (the why) **and** make sure a `Tech debt` item in `Deferred` tracks the debt, so it does not get lost:
  - if no item covers it yet, create one with `origin: D<n>`;
  - if an item already covers the work, keep its `origin`, change its `type` to `Tech debt` and its state to `Deferred`, and say "accepted as debt in D<n>" in its `reason` (never point the item's `origin` at a decision that was made about it).
- **A closing reason that is itself a decision** (e.g. discarded because the team decided to remove a feature): offer to record it as a decision too.

---

## `reports/`

Long material: reviews, investigations, comparisons, plans.

- Name: `YYYY-MM-DD-short-topic.md`.
- Written once; small corrections are fine, a changed conclusion becomes a new report.
- Items and decisions link to reports; reports do not need to link back.
