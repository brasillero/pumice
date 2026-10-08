# AGENTS.md

Rules for every AI coding agent working in this repository (Claude Code, Codex, OpenCode, Kimi and others). Read this file and [`HANDOFF.md`](HANDOFF.md) before starting any task.

## What Pumice is

Pumice lets dictation apps use the AI subscriptions the user already pays for, instead of API keys (no "bring your own key"). Apps such as [Handy](https://github.com/cjpais/Handy) and OpenWhispr send their transcript and their own prompt to an OpenAI-compatible endpoint. Pumice is that endpoint: a small local proxy on `127.0.0.1` (`/v1/chat/completions`, `/v1/models`). It hands the request to the official CLI of a subscription the user is logged into (Claude Code, Codex, Kimi, …) and returns the CLI's answer in the OpenAI format the app expects.

**Pumice is plumbing, not a formatter.** The client app owns the task: formatting, translating or anything else is decided by the prompt the app sends. Pumice adds no instructions of its own, does not rewrite the request and changes the reply as little as possible.

The original product spec is in [`docs/spec.md`](docs/spec.md). It predates the proxy direction; where it conflicts with an owner decision recorded in `HANDOFF.md`, the owner decision wins.

## Documents and who edits them

| File | Role | Agents may edit? |
| --- | --- | --- |
| `docs/product.pt-BR.md` | Snapshot of the Portuguese product doc | No |
| `docs/spec.md` | English translation of the product doc | No |
| `HANDOFF.md` | Current phase, owner decisions in force, next steps and story status | Yes: the status table, the notes and the owner-decision list (only to record decisions the owner made) |
| `BACKLOG.md` | Items to discuss, deferred work and known limitations | Yes |
| `docs/research/*.md` | Findings from investigation stories | Yes |
| `AGENTS.md` | These rules | No, propose changes in a PR description instead |

When the spec seems wrong, incomplete or contradicted by a finding, do not change it. Write the finding in the relevant `docs/research/` file under **Questions for the owner**, and mention it in the PR.

## Language

Everything in this repository is in English: code, identifiers, comments, commit messages, PR descriptions, docs and research notes. The only exception is `docs/product.pt-BR.md`.

The dictation can be in any language. Pumice never changes it; whether the answer stays in the same language or is translated is up to the client's prompt.

## Stack and constraints

- **Rust (stable)**. The deliverable is a single executable per platform.
- **No runtime dependencies for end users.** Users must not need Rust, Node, Python or anything else installed. On Windows (MSVC target), link the C runtime statically (`+crt-static`).
- **Targets:** Windows, Linux and macOS. Windows is the main day-to-day target.
- **Configuration:** one YAML file, validated at startup, with the exact line reported on errors. Nothing is enabled implicitly: providers are an explicit list, each with `enabled` and a `model`. Global settings such as the port have built-in defaults. Each CLI keeps its own user configuration (gateway, routing, login); Pumice inherits it and does not duplicate it in YAML.
- **Network:** listen on `127.0.0.1` only. No telemetry. No outbound calls other than the ones the AI CLIs make themselves. Exception: the generic adapter may call an OpenAI-compatible server on the same machine (loopback only).

## Provider plugins

Each provider is a plugin with the same contract:

- **Input:** the client's request, unchanged: its messages and the model route. The plugin only puts it into the form its CLI accepts (stdin, a file, an argument, inline JSON). It never adds prompt text.
- **Output:** the final answer as text, or a safe typed error. The plugin decodes its CLI's output format and returns just the answer.
- **Settings:** always the smallest, cheapest configuration the CLI offers: no extended thinking or the lowest reasoning effort, and never a fast/priority mode. The model itself comes from the user's config.
- **Differences:** CLI limits and features differ (input size, what can be switched off). The plugin absorbs them; keep plugins as similar as possible and record each limitation in its research note.
- **On any failure** (error, timeout, empty answer), Pumice returns the original transcript unchanged and logs why. There is no fallback to another provider.

## Security rules (non-negotiable)

1. Use AI subscriptions **only through the official CLIs**. Never read, copy or reuse login tokens or credential files.
2. Run every CLI **without tools** (no file, shell or web access) wherever the CLI can switch them off. If it cannot, use its most restricted mode and document it.
3. Run every CLI call inside a **fresh temporary directory**. Its working directory starts empty, except for configuration files Pumice itself writes when a CLI can only read them from there (for example a Kiro agent file); document each such case.
4. **Pumice never acts on the request content.** It adds no instructions, executes nothing from the request and passes the client's text through as data. Because CLIs run without tools, dictating "delete my files" cannot delete anything.
5. Do not log dictated text unless the debug log is explicitly enabled in the config.

## Development environment

- Development happens on **Linux (WSL)**. Run builds and tests there.
- **Do not install anything on the owner's Windows side.** Windows and macOS builds are checked in CI (GitHub Actions). Manual Windows checks are done by the owner.
- AI CLIs available locally are the ones the owner has installed and logged into inside WSL. Ask the owner before assuming a CLI is available, and never log in on their behalf.

## Testing

- Every plugin must be testable with a **fake CLI** (a small test binary or script that mimics the real CLI's input and output).
- Real CLI calls are only for manual checks and research, never in the automated test suite, because they cost quota and time.
- `cargo test` must pass locally on Linux. CI runs the suite on Windows, Linux and macOS.
- Keep real client requests (Handy, OpenWhispr) and dictation samples as fixtures, to prove requests pass through unchanged.

## Git workflow

- One story per branch and per pull request. Branch name: `s<id>-<short-slug>`, e.g. `s0.1-handy-request`.
- Put the story ID in the PR title, e.g. `S0.1: capture Handy request format`.
- **Conventional Commits** for every commit (`feat:`, `fix:`, `docs:`, `test:`, `chore:`, `ci:`, `refactor:`).
- **SemVer** for releases. `CHANGELOG.md` follows Keep a Changelog and is generated from commits at release time; do not hand-edit released sections.
- Keep PRs small and focused. Do not mix refactors with features.
- Update the status table in `HANDOFF.md` in the same PR that finishes a story.

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
- Any decision listed as open in the spec or in `BACKLOG.md`
- Any finding that contradicts the spec or an owner decision in `HANDOFF.md`
