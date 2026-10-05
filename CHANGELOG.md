# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Release sections are generated from Conventional Commits.

## [Unreleased]

## [0.1.0] — 2026-10-05

Initial public release: a local service that lightly formats dictation text with the AI coding CLIs you already pay for, distributed as a single executable per platform and through npm.

### Added

- OpenAI-compatible `/v1/chat/completions` and `/v1/models` API on `127.0.0.1`, served by `pumice` (#17), plus a `/health` route and an opt-in debug log (#20).
- Restricted Claude adapter behind a shared provider interface and process runner (S2.1/S2.2, #8) and a restricted Codex adapter (S2.3, #14).
- Dormant Antigravity adapter: documentation-derived invocation/parser, `enabled: true` refused until a safe per-launch tool policy exists (S2.5, #25).
- OpenCode adapter behind explicit opt-in with a deny-all inline agent (S2.6, #26).
- Generic loopback OpenAI-compatible adapter for local services such as Ollama or LM Studio, off by default (S2.7, #27).
- Provider capabilities in the registry: `disabled_by_default`, risk warnings and full-settings validation with startup warnings (S2.1, #24).
- Startup provider detection with bounded concurrent version probes (S2.8, #28) and `pumice doctor` with an explicit, quota-bearing `--login-check` (S2.8, #29).
- Request extraction and prompt composition with lossless Handy reconstruction (S3.1, #10), optional Pumice prompts from the config (S3.2, #12) and conservative output cleanup (S3.4, #9).
- YAML configuration with a built-in default for every setting, per-user lookup and `pumice check-config` (S6.1/S1.5, #11).
- Pipeline with a total time budget, provider deadlines and raw-text fallback (S4.1, #13), a fallback chain across providers (S4.2, #15) and an explicit `passthrough` model that returns the raw dictation (S4.3 follow-up, #32).
- CLI isolation contract with process groups/Job Objects and Windows shim translation (S5.1, #16).
- Portable release archives for Linux x64 (musl), Windows x64, macOS x64 and macOS ARM64 (S7.1, #36).
- npm distribution: front package `pumice` plus four `@bresillero/pumice-<platform>` native packages, installable with npm, pnpm, Bun and npx (S7.4, #37).
- Bare `pumice` startup command and the end-user installation guide (S7.3, #38).
