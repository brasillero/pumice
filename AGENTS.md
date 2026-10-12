# AGENTS.md

Rules for every AI coding agent working in this repository (Claude Code, Codex, OpenCode, Kimi and others). Read this file and the task notes in [`.task-decisions/`](.task-decisions/README.md) before starting any task. Task notes in `.task-decisions/` follow the task-decisions skill (`.agents/skills/task-decisions/`); do not edit them without it.

## What Pumice is

Pumice lets dictation apps use the AI subscriptions the user already pays for, instead of API keys (no "bring your own key"). Apps such as [Handy](https://github.com/cjpais/Handy) and OpenWhispr send their transcript and their own prompt to an OpenAI-compatible endpoint. Pumice is that endpoint: a small local tool listening on `127.0.0.1` (`/v1/chat/completions`, `/v1/models`). It hands the request to the official CLI of a subscription the user is logged into (Claude Code, Codex, Kimi, Kiro) and returns the CLI's answer in the OpenAI format the app expects.

**Pumice is plumbing, not a formatter.** The client app owns the task: formatting, translating or anything else is decided by the prompt the app sends. Pumice adds no instructions of its own (the only exception is a neutral system prompt some CLIs need, see Provider plugins), does not rewrite the request and changes the reply as little as possible.

The original product spec is in [`docs/spec.md`](docs/spec.md). It predates the pass-through direction; where it conflicts with an owner decision recorded in `.task-decisions/`, the owner decision wins.

## Documents and who edits them

| File | Role | Agents may edit? |
| --- | --- | --- |
| `docs/product.pt-BR.md` | Snapshot of the Portuguese product doc | No |
| `docs/spec.md` | English translation of the product doc | No |
| `.task-decisions/` | Task notes: one task per branch with its backlog (open work, known limitations), decisions and reports | Yes, through the task-decisions skill only; record as decisions only what the owner made or authorized |
| `docs/research/*.md` | Findings from investigation stories | Yes |
| `AGENTS.md` | These rules | No, propose changes in a PR description instead |

When the spec seems wrong, incomplete or contradicted by a finding, do not change it. Write the finding in the relevant `docs/research/` file under **Questions for the owner**, and mention it in the PR.

## Language

Everything in this repository is in English: code, identifiers, comments, commit messages, PR descriptions, docs and research notes. The only exception is `docs/product.pt-BR.md`.

The dictation can be in any language. Pumice never changes it; whether the answer stays in the same language or is translated is up to the client's prompt.

## Stack and constraints

- **Rust (stable)**. The deliverable is a single executable per platform.
- **No runtime dependencies for end users.** The standalone executable needs no Rust, Node, Python or anything else installed. The npm package is only a launcher for that executable and needs Node.js 22 or newer. On Windows (MSVC target), link the C runtime statically (`+crt-static`).
- **Targets:** Windows, Linux and macOS. Windows is the main day-to-day target.
- **Configuration:** one YAML file, validated at startup, with the exact line reported on errors. Nothing is enabled implicitly: providers are an explicit list, each with `enabled` and a `model`. Global settings such as the port have built-in defaults. Each CLI keeps its own user configuration (gateway, routing, login); Pumice inherits it and does not duplicate it in YAML.
- **Network:** listen on `127.0.0.1` only. No telemetry. No outbound calls other than the ones the AI CLIs make themselves.

## Provider plugins

Each provider is a plugin with the same contract:

- **Input:** the client's request, unchanged: its messages and the model route. The plugin only puts it into the form its CLI accepts (stdin, a file, an argument, inline JSON). It never adds prompt text, with one exception: when the client sends no system prompt and the CLI refuses an empty one (Codex, Kimi), the plugin sends the neutral `Follow the user's message.`
- **Output:** the final answer as text, or a safe typed error. The plugin decodes its CLI's output format and returns just the answer.
- **Settings:** always the smallest, cheapest configuration the CLI offers: no extended thinking or the lowest reasoning effort, and never a fast/priority mode. The model itself comes from the user's config.
- **Differences:** CLI limits and features differ (input size, what can be switched off). The plugin absorbs them; keep plugins as similar as possible and record each limitation in its research note.
- **On any failure** (error, timeout, empty answer) or a missing model, Pumice returns an HTTP error with a safe reason and logs why; the app keeps its own text. There is no fallback to another provider.

## Security rules (non-negotiable)

1. Use AI subscriptions **only through the official CLIs**. Never read, copy or reuse login tokens or credential files.
2. Run every CLI **without tools** (no file, shell or web access) wherever the CLI can switch them off. If it cannot, use its most restricted mode and document it.
3. Run every CLI call inside a **fresh temporary directory**. Its working directory starts empty, except for configuration files Pumice itself writes when a CLI can only read them from there (for example a Kiro agent file); document each such case.
4. **Pumice never acts on the request content.** It adds no instructions, executes nothing from the request and passes the client's text through as data. Because CLIs run without tools, or in their most restricted mode, a dictated "delete my files" is treated as text, not as an action.
5. Do not log dictated text unless the debug log is explicitly enabled in the config.

## Development environment

- Development happens on **Linux** (native or WSL). Run builds and tests there.
- **Do not install anything on the owner's Windows side.** Windows and macOS builds are checked in CI (GitHub Actions). Manual Windows checks are done by the owner.
- AI CLIs available locally are the ones the owner has installed and logged into on that machine. Ask the owner before assuming a CLI is available, and never log in on their behalf.

## Testing

- Every plugin must be testable with a **fake CLI** (a small test binary or script that mimics the real CLI's input and output).
- Real CLI calls are only for manual checks and research, never in the automated test suite, because they cost quota and time.
- `cargo test` must pass locally on Linux. CI runs the suite on Windows, Linux and macOS.
- Keep real client requests (Handy, OpenWhispr) and dictation samples as fixtures, to prove requests pass through unchanged.

## Git workflow

- One story or task per branch and per pull request. Story branches are named `s<id>-<short-slug>`, e.g. `s0.1-handy-request`; docs, chores and releases use a descriptive name, e.g. `docs-refresh`, `release-0.2.0`.
- Put the story ID in the PR title when there is one, e.g. `S0.1: capture Handy request format`.
- **Conventional Commits** for every commit (`feat:`, `fix:`, `docs:`, `test:`, `chore:`, `ci:`, `refactor:`).
- **SemVer** for releases. `CHANGELOG.md` follows Keep a Changelog and is written by hand for now; do not edit released sections.
- Keep PRs small and focused. Do not mix refactors with features.
- Keep the story's task notes (`.task-decisions/tasks/<branch>/`) current in the same PR, and finish them (state `Done`) before it merges.

## Scope discipline

Pumice only **relays**: client request in, CLI answer out. Do not build any of these unless the owner asks:

- Content processing: Pumice prompts, formatting rules, reply-cleanup heuristics
- Profiles per kind of text, or per-app styles
- Command mode
- A graphical interface (a `pumice setup` command-line wizard is planned)
- Changes to Handy or other client apps

Do not create a `CLAUDE.md`. This `AGENTS.md` is the single rules file for every agent.

## When to stop and ask the owner

- Anything that needs the Windows GUI (for example, configuring Handy) or a login to an AI service
- Anything that spends paid quota beyond a few test calls
- Any decision listed as open in the spec or in `.task-decisions/`
- Any finding that contradicts the spec or an owner decision in `.task-decisions/`
