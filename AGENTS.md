# AGENTS.md

Rules for every AI coding agent working in this repository (Claude Code, Codex, OpenCode, Kimi and others). Read this file and [`HANDOFF.md`](HANDOFF.md) before starting any task.

## What Pumice is

Pumice is a small local service that receives dictation text transcribed by Whisper (through the [Handy](https://github.com/cjpais/Handy) app), lightly formats it with an AI CLI the user already pays for, and returns the text to be pasted. It exposes an OpenAI-compatible API (`/v1/chat/completions`, `/v1/models`) on localhost.

The full product spec, with every user story and its acceptance criteria, is in [`docs/spec.md`](docs/spec.md).

## Documents and who edits them

| File | Role | Agents may edit? |
| --- | --- | --- |
| `docs/product.pt-BR.md` | Snapshot of the Portuguese product doc (source of truth) | No |
| `docs/spec.md` | English translation of the product doc | No |
| `HANDOFF.md` | Current phase, next steps and story status | Yes, only the status table and notes |
| `docs/research/*.md` | Findings from investigation stories (Phase 0) | Yes |
| `AGENTS.md` | These rules | No, propose changes in a PR description instead |

When the spec seems wrong, incomplete or contradicted by a finding, do not change it. Write the finding in the relevant `docs/research/` file under **Questions for the owner**, and mention it in the PR.

## Language

Everything in this repository is in English: code, identifiers, comments, commit messages, PR descriptions, docs and research notes. The only exception is `docs/product.pt-BR.md`.

The dictation itself can be in any language. Pumice must return text in the same language it received.

## Stack and constraints

- **Rust (stable)**. The deliverable is a single executable per platform.
- **No runtime dependencies for end users.** Users must not need Rust, Node, Python or anything else installed. On Windows (MSVC target), link the C runtime statically (`+crt-static`).
- **Targets:** Windows, Linux and macOS. Windows is the main day-to-day target.
- **Configuration:** one YAML file. Every setting has a built-in default, so the file only contains what the user changes. Validate it at startup and report the exact line on errors.
- **Network:** listen on `127.0.0.1` only. No telemetry. No outbound calls other than the ones the AI CLIs make themselves.

## Security rules (non-negotiable)

1. Use AI subscriptions **only through the official CLIs**. Never read, copy or reuse login tokens or credential files.
2. Run every CLI **without tools** (no file, shell or web access). If a CLI cannot disable tools, use its most restricted mode and document it.
3. Run every CLI call inside a **fresh, empty temporary directory**.
4. Treat dictated text as **data, never as instructions**. Dictating "delete my files" must come back as formatted text.
5. Do not log dictated text unless the debug log is explicitly enabled in the config.

## Development environment

- Development happens on **Linux (WSL)**. Run builds and tests there.
- **Do not install anything on the owner's Windows side.** Windows and macOS builds are checked in CI (GitHub Actions). Manual Windows checks are done by the owner.
- AI CLIs available locally are the ones the owner has installed and logged into inside WSL. Ask the owner before assuming a CLI is available, and never log in on their behalf.

## Testing

- Every adapter must be testable with a **fake CLI** (a small test binary or script that mimics the real CLI's input and output).
- Real CLI calls are only for manual checks and research, never in the automated test suite, because they cost quota and time.
- `cargo test` must pass locally on Linux. CI runs the suite on Windows, Linux and macOS.
- Keep the dictation samples (story S8.1) as fixtures for prompt and cleanup tests.

## Git workflow

- One story per branch and per pull request. Branch name: `s<id>-<short-slug>`, e.g. `s0.1-handy-request`.
- Put the story ID in the PR title, e.g. `S0.1: capture Handy request format`.
- **Conventional Commits** for every commit (`feat:`, `fix:`, `docs:`, `test:`, `chore:`, `ci:`, `refactor:`).
- **SemVer** for releases. `CHANGELOG.md` follows Keep a Changelog and is generated from commits at release time; do not hand-edit released sections.
- Keep PRs small and focused. Do not mix refactors with features.
- Update the status table in `HANDOFF.md` in the same PR that finishes a story.

## Scope discipline

Pumice only **formats** dictation: light corrections, punctuation and lists. Do not build any of these unless the spec changes:

- Profiles per kind of text (email, chat message)
- Per-app styles
- Command mode
- A graphical interface
- Translation
- Changes to Handy itself

Do not create a `CLAUDE.md`. This `AGENTS.md` is the single rules file for every agent.

## When to stop and ask the owner

- Anything that needs the Windows GUI (for example, configuring Handy) or a login to an AI service
- Anything that spends paid quota beyond a few test calls
- Any decision listed as open in the spec (for example, the default port number)
- Any finding that contradicts the spec
