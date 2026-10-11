// Local, read-only viewer for a repository's task notes.
//   node <skill folder>/scripts/serve.mjs [repository]
// Reads <repository>/.task-decisions/ (default: the current directory) and
// serves the viewer page for every page URL, plus:
//   /api/data          the project and every task, read fresh on each request
//   /api/file?path=…   one Markdown file under .task-decisions/ (reports)
//   /icons/<name>.svg  the viewer's icons (Lucide, see THIRD_PARTY_NOTICES.md)
// Listens on 127.0.0.1 only and answers only requests addressed to localhost
// (or to the names in TASK_DECISIONS_HOSTS), so other sites cannot read the
// notes through the browser. PORT changes the port (default 4400). Stop it
// with Ctrl+C. No dependencies, no cache: F5 shows the files as they are now.

import { createServer } from "node:http"
import { readFile } from "node:fs/promises"
import { join, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { loadNotes, notesRoot, readNote } from "./notes.mjs"

const viewer = fileURLToPath(new URL("../assets/viewer/", import.meta.url))
const repo = resolve(process.argv[2] ?? ".")
const port = Number(process.env.PORT ?? 4400)
const hosts = new Set(["localhost", "127.0.0.1", "[::1]", ...(process.env.TASK_DECISIONS_HOSTS ?? "").split(",").map((h) => h.trim().toLowerCase()).filter(Boolean)])

const root = notesRoot(repo)
if (!root) {
  console.error(`No .task-decisions/ folder in ${repo}`)
  process.exit(1)
}

function send(res, status, type, body) {
  res.writeHead(status, { "content-type": type, "cache-control": "no-store", "x-content-type-options": "nosniff" })
  res.end(body)
}

/** The request's host name without the port, lower-cased. */
const hostOf = (req) => (req.headers.host ?? "").toLowerCase().replace(/:\d+$/, "")

createServer(async (req, res) => {
  try {
    if (!hosts.has(hostOf(req))) return send(res, 403, "text/plain", "Forbidden host")
    if (req.method !== "GET" && req.method !== "HEAD") return send(res, 405, "text/plain", "Read-only")
    const url = new URL(req.url, "http://localhost")
    if (url.pathname === "/api/data") {
      return send(res, 200, "application/json", JSON.stringify(loadNotes(repo)?.raw ?? { projects: [] }))
    }
    if (url.pathname === "/api/file") {
      const text = readNote(root, url.searchParams.get("path"))
      return text === null ? send(res, 404, "text/plain", "Not found") : send(res, 200, "text/plain; charset=utf-8", text)
    }
    const icon = /^\/icons\/([a-z0-9-]+\.svg)$/.exec(url.pathname)
    if (icon) {
      const svg = await readFile(join(viewer, "icons", icon[1]), "utf8").catch(() => null)
      return svg === null ? send(res, 404, "text/plain", "Not found") : send(res, 200, "image/svg+xml", svg)
    }
    // Every other URL is a page: the viewer routes it in the browser.
    send(res, 200, "text/html; charset=utf-8", await readFile(join(viewer, "index.html"), "utf8"))
  } catch (error) {
    send(res, 500, "text/plain", String(error))
  }
}).listen(port, "127.0.0.1", () => {
  console.log(`Task decisions viewer: http://localhost:${port} (Ctrl+C to stop)`)
})
