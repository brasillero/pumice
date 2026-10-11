# Task decisions — rules

A repository keeps its task notes in `.task-decisions/`, committed with the code: what is left to do in each task (backlog) and what was decided (decisions).

Read this whole file, and the project's `.task-decisions/README.md`, before creating or changing anything. The exact shape of each file is in `shapes.md`: read the section for the file you are about to write.

---

## Structure

```
<repository>/
└── .task-decisions/
    ├── README.md          ← the project: language, tracker, areas, vocabulary
    └── tasks/
        └── <task>/
            ├── README.md      ← the task
            ├── backlog.md     ← what is left to do
            ├── decisions.md   ← what was decided
            └── reports/       ← long material (reviews, investigations)
```

- **One `.task-decisions/` per repository**, at its root, even in a monorepo (packages or apps become `areas`). Each worktree or clone has the copy that belongs to its branch.
- **The project** is the repository. Its name in references is the repository's name, taken from the git remote (`…/shop-web.git` → `shop-web`), not from the folder name: worktree folders are often named after the task. A repository with no remote uses its folder name.
- **`<task>`**: the task's key in the tracker (`ABC-123`) or, without a key, a short lowercase name with hyphens (`commit-conventions`).
- **All work is a task.** There is no "general" folder.
- **Folders never move or get renamed.** A finished task stays where it is; its state lives in its `README.md`.
- New files start from the skill's `assets/templates/` (see "Lifecycle").

## General rules

1. **Language and vocabulary:** those of the project's `README.md`. Plain, direct writing; code names in backticks.
2. **Check before writing.** Confirm in the code that the problem exists and that the paths you cite are valid. When you cannot check something, you may still write the entry from the evidence you have: say in its Context (or the decision's body) which claims and paths are unverified, and say so in your answer.
3. **Only create an item or a decision when the user asks or agrees.**
4. **Follow the formats in `shapes.md` exactly.** They exist so that people and tools read entries the same way. After changing notes, run the shape check (`scripts/check.mjs`) and fix what it reports.
5. **Commits:** notes change in their own commits, never mixed with code changes, following the repository's commit conventions (without any, `notes(<task>): <what changed>`, e.g. `notes(ABC-123): add #3`). Stage only the notes files you changed. Commit when the user asks, or when the project's `README.md` says to commit notes changes as they happen. Never push unless asked. If the notes are local only (see "First use"), there is nothing to commit.
6. **Make targeted edits.** Change only the entry in question; never rewrite the whole file.
7. **No stored lists.** Indexes, summaries and views ("everything deferred") are generated on request, never saved in the files.
8. **Dates** are when the thing happened, if the user says so; otherwise today's date. If a date contradicts others (a task ending before it started), ask which one is wrong.
9. **Ask, don't invent.** When a required value is not in the request (who decided, the tracker, a revisit condition), ask the user. Do not fill it with a guess.
10. **The notes never act on code.** Creating a task or finishing one does not create, switch or delete branches, nor change code; the `branch` field only records the name.

## Branches

The notes live on the branch where the work happens and reach the main branch with the code, through the same merge/pull request.

- Each task writes only to its own folder, so parallel branches rarely conflict: only when two branches change the same task or the project's `README.md`. Resolve such a conflict by keeping both sides' entries, renumbering the newer ones if two got the same number.
- **Finish the task's notes before the merge**, as part of the merge/pull request (see "Pausing, finishing, abandoning"). Notes changed after the merge reach the main branch through the repository's usual flow, like any other change.

## Lifecycle

### First use in a repository

1. Ask the user whether the notes are **committed with the code** (the default: the team sees them, and they travel with each branch) or **local only** (private, but not shared, not backed up, lost with the clone, and each worktree keeps its own). If the repository's `.gitignore` already ignores `.task-decisions/`, they are local only. For local only:
   - add `/.task-decisions/` to the file `git rev-parse --git-path info/exclude` prints (in a linked worktree `.git` is a file, not a folder; this exclude file is shared by all worktrees of the clone), then confirm with `git check-ignore .task-decisions/README.md`;
   - if notes are already tracked (`git ls-files .task-decisions`), ignoring does not untrack them: tell the user and ask before running `git rm -r --cached .task-decisions`;
   - record the choice in the project `README.md` (`## Commits`: `Local only.`).
2. Create `.task-decisions/` and copy `assets/templates/project/README.md` into it, plus an empty `tasks/`.
3. Fill in the `README.md`: `name` starts as the repository name with `-` and `_` turned into spaces and each word capitalised (see "Project `README.md`" in `shapes.md`); ask the user for `language`, `tracker`, `prefix` and `areas` when they are not obvious (where the code is hosted does not tell you where the tasks are tracked).
4. If the notes are committed, offer to add one line to the repository's agent instructions (`AGENTS.md`, `CLAUDE.md` or equivalent), so agents without this skill know the format exists: `Task notes in .task-decisions/ follow the task-decisions skill; do not edit them without it.`

### New task

Created when the user starts it or when it needs its first note (an item, a decision or a report), not before. A task may start with an empty backlog.

1. Create `.task-decisions/tasks/<task>/` and copy `assets/templates/task/` into it (`README.md`, `backlog.md`, `decisions.md`, `reports/`).
2. Fill in the `README.md`: `state: In progress`, `start`, `ticket`, `branch`, `mr` as known, and a short summary.

### While the task runs

- Keep the `README.md` summary describing where the task stands **now**; update it when that changes (e.g. "waiting for review", "pushed").
- Fill in `mr` when the merge/pull request exists.

### Pausing, finishing, abandoning

- **Paused:** `state: Paused`; the summary says why and what it waits for.
- **Done** (the work is complete: its merge/pull request is approved and about to merge, or the work finished without one) or **Abandoned:**
  1. Every item in **Active** is closed, moved to another task, or set to `Deferred` with a `reason`. Nothing stays Active in a finished task.
  2. `state: Done` or `Abandoned`, `end` set, and the summary rewritten as the final outcome.
  3. Decisions are not touched: they stay in force until superseded or revoked.
  4. With a merge/pull request, do this on its branch before it merges, so the finished notes reach the main branch with the code; fill in `end` with the expected merge date and correct it if the merge happens on another day.
- Deferred items stay where they are; they are found by asking for open items across tasks.

### Importing existing notes

When the user asks to bring existing notes into `.task-decisions/` (a TODO file, an old backlog, ADRs, another notes folder):

1. Read the whole source first and propose how it maps: which tasks, how many items and decisions, which values are missing. Wait for the user's agreement.
2. Keep the source's real dates; if a date is unknown, say so in the entry's body and use the earliest date the source supports.
3. Ask once for values missing across many entries (who decided, areas); otherwise mark them unverified in the body. Never guess.
4. Finished work becomes closed items whose `outcome` cites the source (`from docs/todo.md`, a commit); open work keeps its state, with a `reason` for anything deferred.
5. Number in the source's order. Keep the old ID in the body ("was T3") and rewrite references to the new IDs.
6. `origin` points to the source or `conversation`, never to an entry created later in the same import.
7. Run the shape check, and leave the source untouched until the user confirms it can go.

## IDs and references

- **Backlog items:** `#1`, `#2`… sequential **within the task folder**.
- **Decisions:** `D1`, `D2`… sequential within the task folder.
- **Next number:** the highest in the file + 1. Numbers are never reused, not even for discarded items.
- **No nested numbering** (`#1.1`, `#1.2`). Parts of an item:
  - **small steps** are a checklist under "To do" (`- [ ] …`, `- [x] …`), with no state or ID of their own;
  - **a part that needs its own state, deferral or ticket** becomes a separate item with `origin: #1`. "Everything that came from #1" is a search for `origin: #1`.
- **How to write a reference**, from shortest to most complete:

  | From | Write |
  |---|---|
  | the same task | `#2`, `D3` |
  | another task in this repository | `ABC-123#2`, `ABC-123/D3` |
  | a task in another repository | `shop-api/ABC-123#2`, `shop-api/ABC-123/D3` |

- **Reports:** path relative to the task folder (`reports/2026-01-15-review.md`); from another task, `ABC-123/reports/…`; from another repository, `shop-api/ABC-123/reports/…`.

### Origin rule

Every entry says **where it came from**, in the `origin` field. The `origin` always points from the newer entry to what prompted it, never forward.

| Situation | `origin` |
|---|---|
| Decision made about an item | `#2` |
| Item created because of a decision | `D3` |
| Decision or item that came from a review or investigation | `reports/…` |
| Item picked up from another task | `ABC-123#3` |
| Came from a conversation, with no other record | `conversation` |

- **A report written afterwards is a valid origin** when it records the review or discussion the entry came from (e.g. a review summary dated the day after the decision).
- **Mentions in the body are fine** (e.g. a decision's **Consequences** naming the item it created at the same time). What is not allowed is going back to an older entry to add references to later outcomes, or keeping back-reference lists.
- **Origin follows when things happened, not when they were written.** A decision taken earlier can be recorded later (with its real `date`); an item it prompted may then replace `origin: conversation` with that decision.
- To find "what depends on D3", search for entries citing `D3`; do not keep back-reference lists.
- **Work that spans repositories** lives in one task: in the repository that owns the work. Mention the other repositories in its Context and refer to their entries with full references; never duplicate an item across repositories.

---

## Common requests

| Request | What to do |
|---|---|
| "what is open in this task" | Read the task's `backlog.md`: Active and Deferred sections |
| "what is open" | Active and Deferred items of every task in `.task-decisions/tasks/` |
| "everything deferred" / "checkout debt" | Read `.task-decisions/tasks/*/backlog.md`, filter by the fields, answer with full references |
| "active decisions about X" | Read `.task-decisions/tasks/*/decisions.md`, filter by state and area |
| "why did we do X" | Search the decisions and follow the `origin` links to the report |
| "ready for the tracker" | Write the item's text following the project's tracker template (see "Project `README.md`" in `shapes.md`); once the ticket is created, fill in `ticket` |
| "what is provisional" | Decisions in state `Provisional`; temporary choices must be recorded that way (see "Shape of a decision" in `shapes.md`) |
| "show the notes" | Start the viewer (see `SKILL.md`) and give the user its address |

The answer is generated on the spot and not saved, unless the user asks for it, explicitly; then it is a report in the task it concerns (the task the request came from, when it spans several).
