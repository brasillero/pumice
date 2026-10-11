---
name: task-decisions
description: "Manages task notes in a repository's .task-decisions/ folder - a backlog (#1, #2…) and a decision log (D1, D2…) per task, in Markdown with fixed shapes. Use when the user asks to start, resume, pause or finish task notes in this format; to add, close, defer or move an item in a .task-decisions backlog; to record, supersede, revoke or look up a decision and why it was made; to ask what is open, deferred or provisional in .task-decisions; to turn one of its items into tracker text; or to open the notes viewer. Not for backlogs kept in Jira, GitHub Issues or other trackers."
license: MIT
compatibility: The helper scripts need Node.js 20 or newer and git; no dependencies, no network access.
metadata:
  version: "1.1.1" # x-release-please-version
---

# Task decisions

Each task gets a folder in `.task-decisions/tasks/<task>/` with a backlog (`#1`, `#2`…: what is left to do), a decision log (`D1`, `D2`…: what was decided, why, and what was rejected) and reports. The notes are plain Markdown with fixed shapes, so people, agents and the bundled viewer read them the same way.

**Before creating or changing anything, read `references/rules.md` in full** (general rules, lifecycle, IDs, origins), then the project's `.task-decisions/README.md`, then the section of `references/shapes.md` for the file you are about to write. Those files decide everything below; this page is only the map.

## Files in this skill

Paths are relative to the folder that contains this `SKILL.md` (the skill folder), wherever it was installed; resolve it from this file's location, not from the current directory.

| Path | What |
|---|---|
| `references/rules.md` | General rules, branches, lifecycle, IDs, origins, common requests: read in full |
| `references/shapes.md` | The exact shape of each file (project and task README, backlog, decisions, reports): read the section you need |
| `assets/templates/project/README.md` | Skeleton for `.task-decisions/README.md` (first use in a repository) |
| `assets/templates/task/` | Skeleton for a new task folder |
| `scripts/check.mjs` | Shape check |
| `scripts/serve.mjs` | Read-only viewer |
| `assets/viewer/` | The viewer's page and icons (used by the scripts) |

Run the scripts by their absolute path and pass the repository root explicitly:

```sh
node "<skill folder>/scripts/check.mjs" "<repository root>"   # exit 1 and a list when something is out of shape
node "<skill folder>/scripts/serve.mjs" "<repository root>"   # http://localhost:4400 (PORT to change it)
```

## Workflows

**First use in a repository** (no `.task-decisions/` yet, and the user wants notes): follow "First use in a repository" in the rules. Ask whether the notes are committed or local only, and for language, tracker, key prefix and areas when they are not obvious.

**Add an item / record a decision:** only when the user asks or agrees. Find the task (from the request, the current branch or the ticket); create it from the template if it does not exist. Take the next number, write the entry in the exact shape, and ask for any required value the request does not give (who decided, a revisit condition…).

**Close, defer, move an item; supersede or revoke a decision:** follow the matching section of the rules. Closed items keep their full text and are final; decisions are append-only (only the state in the heading changes).

**Answer questions** ("what is open", "why did we do X", "what is provisional"): read the files and answer with references (`ABC-123#2`, `ABC-123/D3`). Do not save the answer unless asked.

**After every change:** run the shape check and fix what it reports. Commit only as rule 5 says, in a commit of its own with only the notes files; never push unless asked.

**Show the notes:** start the viewer in the background and give the user the address. It listens on the local machine only; stop it when the user is done (Ctrl+C, or end the process).
