---
name: Pumice
language: en-US
tracker: —
prefix: —
areas: architecture, api, pipeline, providers, transcription, config, cli, distribution, process, docs
---

Pumice lets dictation apps (Handy, OpenWhispr) use the AI subscriptions the user already pays for. It is a small local tool on `127.0.0.1` with an OpenAI-compatible interface: it passes the app's request, unchanged, to the official CLI of a subscription the user is logged into (Claude Code, Codex, Kiro) and returns the answer in the OpenAI format the app expects.

Each task is one branch and pull request; the task folder is named after the branch. Story IDs (`S4.8`) are kept in titles; there is no external tracker. Owner decisions recorded here take precedence over descriptive documentation; current behaviour is documented in the module guides in `docs/`.

Historical task notes may cite product and research documents removed during documentation consolidation. Those references describe the repository at the time; retrieve the cited files from Git history. Current behaviour is documented in the module guides and `docs/providers.md`.

## Vocabulary

- **owner**: the repository owner (`brasillero`), who makes product decisions.
- **client** or **app**: the dictation app that calls Pumice (Handy, OpenWhispr).
- **provider** or **plugin**: one AI CLI integration (`claude`, `codex`, `kiro`), selected by the request's `model` field.
- **dictation** or **transcript**: the text the app transcribed and sends for processing.
- **pass-through**: Pumice adds no instructions and changes the reply as little as possible.
- **story**: a unit of planned work with an ID such as `S4.8` or `P16`.
- **orchestrator**: the agent that plans the work, delegates implementation and merges after review.
