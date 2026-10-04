# Handoff — start here

**Current phase: Phase 0 — Investigation.** No product code yet. The goal of this phase is to answer the open technical questions before building the MVP.

Read [`AGENTS.md`](AGENTS.md) for the rules and [`docs/spec.md`](docs/spec.md) for the full spec (stories S0.1 to S0.5 are the ones in scope now).

## Phase 0 gate

Phase 1 (the v0.1 MVP) starts only when both are true:

1. The request format Handy sends to a Custom endpoint is documented (S0.1).
2. At least one CLI, Claude, is confirmed viable: it can be called non-interactively, without tools, within the 30 s target (S0.2 and S0.4).

## Stories in this phase

Each story produces a research note in `docs/research/`. Every note ends with two sections: **Findings** (what was confirmed, with evidence) and **Questions for the owner** (anything that needs a human decision).

### S0.1 — Handy request format

Deliverable: `docs/research/S0.1-handy-request.md`

- Build a throwaway HTTP listener that accepts any route and logs method, path, headers and body, then replies with a minimal valid OpenAI chat completion (echoing the input text). It can live in the S0.4 prototype.
- The owner configures Handy on Windows (Settings > Advanced > Experimental Features > Post Processing, provider **Custom**, base URL pointing at the listener) and triggers a dictation with the post-processing hotkey. Write clear step-by-step instructions for the owner in the note; the agent cannot operate the Windows GUI.
- Answer: route(s) called, whether `/v1/models` is called, how the prompt is sent (all in a user message, or with a separate system message), whether structured output is requested, whether `stream: true` is used, whether the language is sent, and what Handy does with the response.
- Redact any real dictated text in the note.

### S0.2 — Non-interactive mode of each CLI

Deliverable: `docs/research/S0.2-cli-matrix.md`

- Ask the owner which CLIs are installed and logged in inside WSL. Start with Claude.
- For each available CLI, fill a table with: version, non-interactive command, how to pass a system prompt, how to disable tools, output format (plain text or JSON), how to tell "not installed" from "not logged in", and whether a server or warm mode exists.
- Measure latency with one fixed sample text: median of 5 cold runs, plus warm runs if a server mode exists.
- CLIs to cover: Claude (`claude -p`), Codex (`codex exec`), OpenCode (`opencode run`), Kimi (command to be confirmed) and Antigravity (`agy -p`). **Antigravity is high risk** (reports of Google accounts banned for automation): only document it from public docs, do not run it unless the owner explicitly says so.

### S0.3 — Terms of use

Deliverable: `docs/research/S0.3-terms.md`

- For each provider, link the official terms or docs that cover non-interactive or scripted personal use of its CLI, summarize the rule, and note any quota limits.
- Mark each provider as OK, unclear or risky. This is a summary for the owner, not legal advice.

### S0.4 — Validate the stack

Deliverables: a minimal Rust prototype and `docs/research/S0.4-stack.md`

- Create the Cargo project for Pumice with a prototype binary that spawns `claude -p` in an empty temp directory, passes a sample text and reads the response.
- Add a GitHub Actions workflow that builds on Windows, Linux and macOS. On Windows, use the MSVC target with a statically linked C runtime.
- Confirm the Windows executable has no runtime dependencies beyond what ships with Windows (for example, check its imported DLLs).
- In the note, record the path to an installer and auto-update with Tauri for Phase 3. Research only, no implementation.

### S0.5 — Handy on Windows, Pumice in WSL

Deliverable: `docs/research/S0.5-wsl-localhost.md`

- Check that a listener bound to `127.0.0.1` inside WSL is reachable from Windows at `http://localhost:<port>`. This can be done together with S0.1, since Handy on Windows will be calling the listener in WSL.
- Document the WSL networking mode in use and any setting the owner needs to change.

## Status

Update this table in the PR that finishes each story.

| Story | Status | PR | Notes |
| --- | --- | --- | --- |
| S0.1 Handy request format | Done (pending review) | #4 | `GET /v1/models` plus `POST /v1/chat/completions`. One user message with the prompt and a `<transcript>`, no system message. `stream: false`, `reasoning_effort: none`, no language field, no auth. Response pasted verbatim. |
| S0.2 CLI matrix | Not started | | Start with Claude |
| S0.3 Terms of use | Not started | | |
| S0.4 Validate the stack | Done (pending review) | #3 | Rust prototype: `pumice listen` (for S0.1) and `pumice claude-probe` (real call 2.8 s). CI on 3 OSes with a Windows CRT DLL check. |
| S0.5 WSL localhost | Not started | | Can run together with S0.1 |

## After Phase 0

Phase 1 (v0.1) covers the OpenAI-compatible server (E1), the Claude adapter (S2.1, S2.2), prompts and cleanup (E3), timeout with raw-text fallback (S4.1, S4.3), security (E5), the config file (S6.1, S6.2, S6.4) and the fake CLI with tests (S8.2, S8.3). Its gate: a dictation formatted end to end through Handy. This file will be updated with the Phase 1 plan once the Phase 0 gate is met.
