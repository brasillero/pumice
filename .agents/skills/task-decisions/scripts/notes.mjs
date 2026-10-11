// Reads a repository's .task-decisions/ folder for the viewer and the check.
// Every folder and file is followed to its real path (symlinks included) and
// used only if that path is inside .task-decisions/; anything else is left
// out and listed in `rejected`.

import { execFileSync } from "node:child_process"
import { existsSync, readFileSync, readdirSync, realpathSync, statSync } from "node:fs"
import { basename, join, relative, resolve, sep } from "node:path"

/** The real path of .task-decisions/ in a repository, or null if there is none. */
export function notesRoot(repo) {
  const dir = join(resolve(repo), ".task-decisions")
  if (!existsSync(dir)) return null
  try { return statSync(dir).isDirectory() ? realpathSync(dir) : null } catch { return null }
}

/** The real path of `path` if it exists and is inside `root`, else null. */
export function contained(root, path) {
  try {
    const real = realpathSync(path)
    return real === root || real.startsWith(root + sep) ? real : null
  } catch {
    return null
  }
}

/** The project's name: the repository's name from its git remote (only when the folder is the repository's root), else its folder name. */
export function projectSlug(repo) {
  const dir = resolve(repo)
  const git = (...args) => execFileSync("git", ["-C", dir, ...args], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim()
  try {
    if (realpathSync(git("rev-parse", "--show-toplevel")) === realpathSync(dir)) {
      const name = /([^/\\:]+?)(?:\.git)?[/\\]?$/.exec(git("remote", "get-url", "origin"))?.[1]
      if (name) return name
    }
  } catch {}
  return basename(dir)
}

/** Everything the viewer's parser needs, in its `raw` shape, plus the paths left out. */
export function loadNotes(repo) {
  const root = notesRoot(repo)
  if (!root) return null
  const rejected = []
  const show = (path) => relative(root, path) || "."

  const list = (path, keep) => {
    const dir = contained(root, path)
    if (!dir) { if (existsSync(path)) rejected.push(show(path)); return [] }
    let entries = []
    try { entries = readdirSync(dir, { withFileTypes: true }) } catch { return [] }
    const out = []
    for (const entry of entries) {
      if (entry.name.startsWith(".")) continue
      const real = contained(root, join(dir, entry.name))
      if (!real) { rejected.push(show(join(dir, entry.name))); continue }
      if (keep(entry.name, statSync(real))) out.push(entry.name)
    }
    return out.sort()
  }
  const read = (path) => {
    if (!existsSync(path)) return null
    const real = contained(root, path)
    if (!real) { rejected.push(show(path)); return null }
    try { return readFileSync(real, "utf8") } catch { return null }
  }

  const tasks = list(join(root, "tasks"), (_, s) => s.isDirectory()).map((task) => {
    const dir = join(root, "tasks", task)
    return {
      slug: task,
      readme: read(join(dir, "README.md")),
      backlog: read(join(dir, "backlog.md")),
      decisions: read(join(dir, "decisions.md")),
      reports: list(join(dir, "reports"), (name, s) => s.isFile() && name.endsWith(".md")),
    }
  })
  return { root, rejected, raw: { projects: [{ slug: projectSlug(repo), readme: read(join(root, "README.md")), tasks }] } }
}

/** One Markdown file under .task-decisions/, by its path relative to it, or null. */
export function readNote(root, relativePath) {
  if (!relativePath || !relativePath.endsWith(".md")) return null
  const real = contained(root, resolve(root, relativePath))
  if (!real) return null
  try { return readFileSync(real, "utf8") } catch { return null }
}
