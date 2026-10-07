# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Release sections are generated from Conventional Commits.

## [Unreleased]

### Changed

- Provider risk warnings are no longer printed at startup, in `check-config` or in `doctor`: every enabled provider is treated the same way (owner decision). The residual-risk notes stay in each adapter's source and research notes.
- **Configuration format changed in 0.2.** Pumice assumes nothing anymore: no provider is enabled by default, no model is built in, and there is no fallback chain — a request runs exactly one provider and any failure returns the original text. `providers:` is now an explicit ordered list, and the client's `model` field picks the provider: a request without a model returns the original text:
  ```yaml
  # before
  default_provider: claude
  fallback_order: [codex]
  providers:
    claude:
      enabled: true
      model: haiku

  # after
  providers:
    - id: claude
      enabled: true
      model: haiku
    - id: codex
      enabled: false
  ```
  The removed `default_provider` and `fallback_order` keys, and the old mapping form of `providers:`, are rejected with a migration error that points at the offending line. `enabled` is required on every entry and `model` on every enabled entry.

### Fixed

- No more crash on an empty transcript envelope (`<transcript>\n</transcript>`), or on a request field the debug log cannot re-read (#57).
- The temporary directory and control files of each CLI call are private (0700/0600) whatever the umask (#56).
- A whitespace-only dictation returns the original text byte for byte instead of an empty string. A request without a valid model still logs that reason.
- Windows line endings (`\r\n`) around the transcript are stripped like `\n`.
- Every completion request writes exactly one log entry: HTTP errors are logged as `REJECTED` with the fixed error message, and a client that disconnects mid-request as `DROPPED`.
- Config validation: removed keys set to `null` (`default: null`, `default_provider: ~`, `fallback_order: null`) are rejected like any other value; an empty `binary`, and a `model` starting with `-` for a CLI provider, are rejected at their line; an empty `APPDATA` no longer resolves the config path relative to the current directory.
- A command-line argument that is not valid Unicode is a usage error (exit 2) instead of a crash.
- Kimi: a dictation full of quotes or backslashes that fits the byte cap but would overflow the Windows command line after escaping is refused as too large, instead of failing to start.
- Adapters reject a CLI reply that is not valid UTF-8 (Codex, Kimi, Kiro, OpenCode, Antigravity) instead of returning text with `�` in it.
- Quota errors reported with HTTP 429 are logged as quota exhausted, not rate limited (Codex, Kimi, OpenCode); Codex also recognizes OpenAI's "exceeded your current quota" wording.
- Kimi: `tool_calls: null` or `[]` is no longer mistaken for a tool call, and an early transient retry no longer hides a later login or quota failure.
- OpenCode: the result is the final message's text only, the final step must finish with `stop`, and an error the CLI recovered from no longer fails the run.
- Kiro: text after the end of the turn, or a later idle state without `end_turn`, is rejected; an `agent_message` without `messageId` is accepted.
- Generic: a malformed `tool_calls` value or a non-string `finish_reason` is rejected, and an error status with a stalled body keeps its classification instead of becoming a timeout.


## [0.1.1] — 2026-10-06

Claude and Codex now use the credentials and gateway already configured for them, and the request log says which provider and model answered, whether a fallback happened, and why an attempt failed.

### Added

- Built-in `inspect` model that echoes the complete request JSON, for checking what a client sends (#43).
- Readable request log: local time, request number, outcome, the provider and model that produced the text, total time and text length. After a fallback or failure, one line per attempt with its result and time. Startup prints the default route, e.g. `claude (haiku) → codex (gpt-6.1-sol)` (#44).
- Failure cause for every failed attempt: the request log shows the exit code, API status and error type; the opt-in debug log adds the CLI's stderr and output excerpts (#45).

### Changed

- Codex reads the user's own `~/.codex/config.toml` (or `$CODEX_HOME`), so a gateway, provider or profile configured there applies without repeating it in Pumice. MCP servers named there are disabled per call, plugins, apps and `notify` are off, and an unreadable config falls back to ignoring it (#48).

### Fixed

- Claude ignored `~/.claude/settings.json`, including a gateway configured in its `env` block, and fell back to the subscription login: the `--restricted` flag is removed; `--tools ""` and `--safe-mode` keep tools and customizations off (#46).
- Claude's "weekly limit" message is reported as an exhausted quota (#46).
- The startup route line could stop the service when a client closed stdout after reading the first line; it now prints on stderr (#47).
- Restore the original `@brasillero` npm native-package scope after correcting the misspelled account (#41).
- Scope the npm front package as `@brasillero/pumice` (keeping the installed command `pumice`): the registry rejects the unscoped name `pumice` as too similar to the existing package `juice` (#41).

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
