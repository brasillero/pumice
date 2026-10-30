# AGENTS.md

Rules for every AI coding agent working in this repository (Claude Code, Codex, OpenCode and others). Read this file and the task notes in [`.task-decisions/`](.task-decisions/README.md) before starting any task. Task notes in `.task-decisions/` follow the task-decisions skill (`.agents/skills/task-decisions/`); do not edit them without it.

## What Pumice is

Pumice lets dictation use the AI subscriptions the user already pays for, instead of API keys (no "bring your own key"), and aims to eventually replace dictation apps such as [Handy](https://github.com/cjpais/Handy). It is a set of independent modules, each a CLI that works on its own:

- **Relay** (`crates/relay`): an OpenAI-compatible endpoint on `127.0.0.1` (`/v1/chat/completions`, `/v1/models`). It hands the request to the official CLI of a subscription the user is logged into (Claude Code, Codex, Kiro) and returns the answer in OpenAI format. This is what Handy uses today.
- **Transcription** (`crates/transcription`, program `pumice-transcribe`): audio in, raw text out, OpenAI-compatible where an OpenAI contract exists. Independent of the relay.
- **Formatter** (`crates/formatter`, built into `pumice`): injects explicitly selected formatting rules, as a system message, into an incoming request before it reaches the relay.
- **Orchestrator** (`pumice`): runs one module or the whole stack and combines their flows (`pumice run`, `pumice serve`).

Interfaces (a future UI) are built on top of the CLIs and the orchestrator, never by duplicating module logic; CLIs offer machine-readable output for that. Current module architecture and contracts are documented in [`docs/api.md`](docs/api.md), [`docs/formatter.md`](docs/formatter.md), [`docs/orchestrator.md`](docs/orchestrator.md) and [`docs/transcription.md`](docs/transcription.md). The owner's architecture decisions are in `.task-decisions/tasks/arch-modules/`.

**The relay is plumbing, not a formatter.** When a client app sends its own prompt, the app owns the task: the relay adds no instructions of its own (the only exception is a neutral system prompt some CLIs need, see Provider plugins), does not rewrite the request and changes the reply as little as possible. Formatting rules come only from the formatter module, when explicitly selected.

Owner decisions recorded in `.task-decisions/` take precedence over descriptive documentation. Current behavior is documented in the relevant module guides.

## Documents and who edits them

| File | Role | Agents may edit? |
| --- | --- | --- |
| `.task-decisions/` | Task notes: one task per branch with its backlog (open work, known limitations), decisions and reports | Yes, through the task-decisions skill only; record as decisions only what the owner made or authorized |
| `docs/*.md` and crate READMEs | Current user and developer documentation, including provider evidence and the release procedure | Yes; preserve owner decisions recorded in `.task-decisions/` |
| `AGENTS.md` | These rules | No, propose changes in a PR description instead |

When a finding reveals a gap in current documentation or conflicts with an owner decision, record it in `.task-decisions/tasks/<task>/reports/` under **Questions for the owner**, following the task-decisions skill, and mention it in the PR. Do not change an owner decision without authorization.

## Language

Everything in this repository is in English: code, identifiers, comments, commit messages, PR descriptions, documentation and task notes.

The dictation can be in any language. Transcription never translates it; whether the answer stays in the same language or is translated is up to the client's prompt or the selected formatter.

## Stack and constraints

- **Rust (stable)**, one Cargo workspace with a library and a thin binary per module.
- **One CLI, which may be installed as a folder** (`arch-modules/D16`). Users get one `pumice` command; an install may be a folder holding `pumice`, `pumice-transcribe` and its native libraries, delivered together by npm or a release archive. `pumice` (orchestrator, relay and formatter, which are built in) and `pumice-relay` keep their static linkage as an implementation choice, not a product requirement: static musl on Linux, static C runtime on Windows (`+crt-static`), only OS libraries on macOS. They contain no native transcription engine.
- **Transcription is a separate program** and may ship version-matched native libraries, runtime CPU variants and GPU backends. GPU drivers are OS prerequisites, documented as such.
- **No developer runtimes for end users.** Users need no Rust, Python, CMake or compilers. The npm package is only a launcher and needs Node.js 22 or newer.
- **Targets:** Windows, Linux and macOS. Windows is the main day-to-day target.
- **Configuration:** one YAML file per module (`relay.yml`, `transcribe.yml`, `formatter.yml`, `orchestrator.yml`), validated at startup, with the exact line reported on errors. The orchestrator refers to module configs and never copies their settings. Nothing is enabled implicitly: providers, models and modules are explicitly enabled. Settings must be explicit in YAML; omitted settings are startup errors with file and line. Defaults exist only in the initial configuration. Each AI CLI keeps its own user configuration (gateway, routing, login); Pumice inherits it and does not duplicate it in YAML.
- **Backward compatibility** is not required yet: nobody but the owner uses the configuration, so command and config changes need no compatibility window. The live instance (port 7567, the Handy URL used by the owner's friends) is updated by hand when a new version is deployed.
- **Network:** listen on `127.0.0.1` only. No telemetry. Modules may talk to each other over loopback (`127.0.0.1`); remote modules are a later goal that needs its own network and authentication policy. The only other outbound traffic allowed is (1) the calls the AI CLIs make themselves and (2) explicit transcription model downloads from a catalog pinned by revision and SHA-256, never triggered implicitly at startup or by a request. Local model files stay supported.

## Provider plugins

Each provider is a plugin with the same contract:

- **Input:** the client's request, unchanged: its messages and the model route. The plugin only puts it into the form its CLI accepts (stdin, a file, an argument, inline JSON). It never adds prompt text, with one exception: when the client sends no system prompt and the CLI refuses an empty one (Codex), the plugin sends the neutral `Follow the user's message.`
- **Output:** the final answer as text, or a safe typed error. The plugin decodes its CLI's output format and returns just the answer.
- **Settings:** the model, the reasoning effort (`low`, `medium` or `high`, mapped to the CLI's own setting) and, for Claude, thinking come from the user's config. Every other setting is the cheapest the CLI offers, and never a fast/priority mode.
- **Differences:** CLI limits and features differ (input size, what can be switched off). The plugin absorbs them; keep plugins as similar as possible and record each limitation in `docs/providers.md`, with the CLI version, evidence date and verification status.
- **On any failure** (error, timeout, empty answer) or a missing model, Pumice returns an HTTP error with a safe reason and logs why; the app keeps its own text. There is no fallback to another provider.

## Security rules (non-negotiable)

1. Use AI subscriptions **only through the official CLIs**. Never read, copy or reuse login tokens or credential files.
2. Run every AI CLI **without tools** (no file, shell or web access) wherever the CLI can switch them off. If it cannot, use its most restricted mode and document it.
3. Run every AI CLI call inside a **fresh temporary directory**. Its working directory starts empty, except for configuration files Pumice itself writes when a CLI can only read them from there (for example a Kiro agent file); document each such case.
4. **Pumice never acts on the request content.** No module executes anything from a request or a dictation. The relay adds no instructions and passes the client's text through as data; the formatter only adds the rules the user selected; transcription only reads the audio and model files it was given. Because CLIs run without tools, or in their most restricted mode, a dictated "delete my files" is treated as text, not as an action.
5. Do not log dictated text, transcripts or audio unless that module's debug log is explicitly enabled in its config.

## Development environment

- Development happens on **Linux** (native or WSL). Run builds and tests there.
- **Do not install anything on the owner's Windows side.** Windows and macOS builds are checked in CI (GitHub Actions). Manual Windows checks are done by the owner.
- AI CLIs available locally are the ones the owner has installed and logged into on that machine. Ask the owner before assuming a CLI is available, and never log in on their behalf.

## Testing

- Every plugin must be testable with a **fake CLI** (a small test binary or script that mimics the real CLI's input and output). Transcription is testable with a **fake engine**; modules talking to each other are tested with fake components.
- Real CLI calls are only for manual checks and research, never in the automated test suite, because they cost quota and time. Real model inference in CI is limited to an optional small-model smoke test.
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

Build the modules and phases the owner approved in `.task-decisions/` (see `arch-modules`), in the order recorded there. Do not build any of these unless the owner asks:

- Formatting rules or prompts inside the relay, or reply-cleanup heuristics beyond the current minimal cleanup
- Command mode (acting on dictated instructions)
- A graphical interface, microphone capture, hotkeys or pasting text (planned for later, as layers above the CLIs)
- Changes to Handy or other client apps

Do not create a `CLAUDE.md`. This `AGENTS.md` is the single rules file for every agent.

## When to stop and ask the owner

- Anything that needs the Windows GUI (for example, configuring Handy) or a login to an AI service
- Anything that spends paid quota beyond a few test calls
- Any unresolved owner decision or question recorded in `.task-decisions/`
- Any finding that contradicts an owner decision recorded in `.task-decisions/`
