// Checks a repository's task notes against the shapes, with the viewer's own
// parser and validator (no browser needed).
//   node <skill folder>/scripts/check.mjs [repository]
// Prints every problem and exits with 1 if there are any (2 if there are no notes).

import { readFileSync } from "node:fs"
import { resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { loadNotes } from "./notes.mjs"

const repo = resolve(process.argv[2] ?? ".")
const notes = loadNotes(repo)
if (!notes) {
  console.error(`No .task-decisions/ folder in ${repo}`)
  process.exit(2)
}

const html = readFileSync(fileURLToPath(new URL("../assets/viewer/index.html", import.meta.url)), "utf8").replace(/\r\n?/g, "\n")
const script = html.split("<script>")[1].split("</script>")[0]
const logic = script.slice(script.indexOf("// ---------- Shape vocabulary"), script.indexOf("// ---------- Markdown"))

const run = new Function("raw", logic + `
build(raw)
return DB.projects.flatMap((p) => [
  ...p.problems.map((x) => "README.md: " + x.replace(/^README\\.md: /, "")),
  ...p.tasks.flatMap((t) => t.problems.map((x) => "tasks/" + t.slug + ": " + x)),
])`)
const problems = [
  ...notes.rejected.map((path) => `${path}: points outside .task-decisions/ (left out)`),
  ...run(notes.raw),
]
if (problems.length) {
  console.log(problems.join("\n"))
  process.exit(1)
}
console.log("No shape problems.")
