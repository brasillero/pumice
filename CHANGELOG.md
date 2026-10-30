# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Release sections are written by hand.

## [Unreleased]

### Changed

- Documentation: how each provider CLI is called, its limits, the effort and Claude model-name rules and the subscription-terms findings are now in `docs/providers.md`; the captured Handy request and the `inspect` model are in `docs/api.md`; the WSL notes and the maintainers' release procedure are in `docs/install.md`; the CPU-variant evidence is in `docs/transcription.md`.
- Documentation: the product spec (`docs/spec.md`, `docs/product.pt-BR.md`) and the research notes in `docs/research/` were removed after their still-needed content moved; Git history keeps them. `AGENTS.md` now points at the module guides and `.task-decisions/` instead.

## [0.3.0] — 2026-10-29

Pumice is now a set of modules behind one `pumice` command: the relay (what Handy uses, still on port 7567), local transcription, a formatter, and `pumice run` / `pumice serve`, which combine them. The relay now starts with `pumice relay serve`, and every setting must be written in YAML. Read "Upgrading from 0.2.0" before updating.

### Upgrading from 0.2.0

- **Start the relay with `pumice relay serve`** (`--plain` keeps the one-line log). Bare `pumice` now prints the command overview and exits. Handy's base URL and port 7567 are unchanged. A systemd unit's `ExecStart=` needs the same change (see "Upgrading from 0.2" in `docs/install.md`).
- **Rename `pumice.yaml` to `relay.yml`** in the same folder; the old name is no longer read.
- **Write every relay setting.** Nothing has a built-in default anymore: add `port`, `total_timeout_secs`, `max_parallel` and `debug_log` (`enabled`, `path`), and on every provider entry, disabled ones included, `model` (`null` only when disabled), `binary` (the command, e.g. `binary: claude`), `timeout_secs` and `effort`, plus `thinking` on `claude`. `effort: low` on every entry and `thinking: false` on `claude` keep 0.2.0's behaviour. `pumice relay check-config` names each missing setting with its file and line.
- **Delete any `kimi` or `antigravity` entry** from `providers:`; it now stops the relay with "provider was removed".
- **New module files.** The first command that needs settings creates the missing per-user `relay.yml`, `transcribe.yml`, `formatter.yml`, `orchestrator.yml` and sample formatter from the supplied examples; existing files are never changed. Every setting in them must be written too.
- **npm on Linux needs glibc 2.35 or newer.** On Alpine and other musl distributions, use the portable archive (it has `pumice` only, without transcription).
- **Only if you ran a build from `main` between 0.2.0 and 0.3.0:** `transcribe.yml` no longer takes a `models:` list (put the `.gguf` files in the models folder and name them by id, see below), and `orchestrator.yml` no longer takes `serve.transcription_flows` (`serve.relay` selects the published `model:provider:preset` ids). Both are rejected with a migration message at their line.

### Added

- **Local transcription: `pumice transcribe` (`pumice-transcribe`).** `file` transcribes a local audio file with a Whisper GGUF model through transcribe.cpp, as plain text or a `--json` envelope, and never translates. WAV, MP3, FLAC, Ogg Vorbis, WebM/Opus and Ogg/Opus are detected by content, up to 25 MB and 120 s of audio. Settings live in `transcribe.yml` (`check-config`), and every command reads a complete file, `file --model-file PATH` included. The models are the `.gguf` files in the models folder (`models_dir`, or with `null` the per-user folder, `~/.local/share/pumice/models` on Linux): a file matching a catalog model by size and SHA-256 uses its catalog id even when renamed, any other its file name without the extension; `models list` shows the ids and any problem, and duplicate ids are errors. `pumice transcribe …` runs the `pumice-transcribe` installed next to `pumice`. Guide: `docs/transcription.md`.
- **`pumice-transcribe serve`**, an OpenAI-compatible transcription endpoint on `127.0.0.1` with one loaded model: `POST /v1/audio/transcriptions` (`json` or `text` responses), `GET /v1/models` and `/health`.
- **Model catalog and downloads.** `pumice-transcribe models catalog|download|verify` fetch the two pinned catalog models (`whisper-tiny-q8_0`, `whisper-large-v3-turbo-q8_0`) only when asked, check their exact size and SHA-256, install them atomically and report progress as JSON lines. Nothing is downloaded at startup or by a request; any other `.gguf` file in the models folder works too.
- **Faster CPU transcription.** The transcription engine ships one CPU module per instruction-set level and picks, at startup, the best one the CPU and OS can run (on the owner's server, Large v3 Turbo took about 8 s instead of about 25 s for a 14 s clip).
- **GPU transcription (experimental).** `backend: cpu|auto|gpu` in `transcribe.yml` (or `--backend`) selects the compute backend: Vulkan on Windows and Linux x64, Metal on Apple Silicon. `cpu` never loads a GPU module; `gpu` fails instead of falling back; `auto` uses only a GPU identified as discrete hardware (never Metal for now) and otherwise stays on the CPU with a named reason. `pumice-transcribe backends` reports what is available without loading a model, and every result reports the backend and device actually used. Each bundle's `bundle.json` (size and SHA-256 of every file) is checked at startup.
- **Formatter module: `pumice format` (`pumice-format`).** `apply` inserts the rules of an explicitly selected formatter as one `system` message into an OpenAI chat request and re-emits everything else byte for byte; a request that already carries instructions is refused unless `--on-conflict append` is given. `list`, `show` and `check-config` work with the definitions registered in `formatter.yml`. The built-in `builtin:clean` is adapted from Handy's cleanup prompt (MIT, its notice ships with Pumice). The relay itself still adds no instructions. Guide: `docs/formatter.md`.
- **`pumice run <AUDIO> --flow <ID>`** transcribes one file, applies the flow's formatter and calls the relay in-process, and returns the raw transcript and the final text apart (`--json` for a versioned envelope). Flows (transcription model, formatter, relay route and time budgets) are defined in `orchestrator.yml`. Guide: `docs/orchestrator.md`.
- **`pumice serve`, the local stack.** It listens on `127.0.0.1:17570` (`serve.port`; 7567 is refused) and keeps one transcription model warm in a supervised `pumice-transcribe` child that is restarted if it fails. `POST /pumice/v1/dictations` takes audio and an enabled flow id and returns the raw transcript and the final text; `/health`, `/ready`, `/pumice/v1/status` and `/pumice/v1/flows` report its state. One request runs at a time (others get 429), a client that disconnects cancels its work, and browsers are accepted only from its own origin. `docs/api.md` lists every Pumice HTTP route.
- **`pumice serve` answers OpenAI transcription clients.** `POST /v1/audio/transcriptions` and `GET /v1/models` (plus `/v1/models/{id}`) use the real transcription model id for raw text or `model:provider:preset` for optional AI processing, and `text` is the final text. `json` and `text` responses are supported; `language`, `prompt` and `temperature` go to the transcription only. If the formatter or the AI fails, the answer is still a success with the raw transcript as `text`, and the failure is reported in a `pumice` object and the `Pumice-Postprocessing-Status` header; transcription failures are OpenAI errors. Transcriptions and dictations share one work permit, limits, deadlines and privacy rules. Provider and preset combinations are selected explicitly in `serve.relay`. `serve.relay` names the default provider and preset and whether every provider (`expand_providers`) and every listed preset (`expand_presets`) is published as its own id; `serve.budgets_ms` bounds these requests. With `serve.relay.enabled: false` only raw text is offered, no AI runtime is built and AI dictations report `relay_disabled`.
- **A browser test page on `pumice serve`.** Open `http://127.0.0.1:17570/` to pick a published model (with its preset, AI route and readiness), record from the microphone (WebM/Opus, or WAV) or upload a file, and see the final text with a Copy button, the raw transcript, the post-processing status (a failure highlighted with its reason) and timings; Cancel aborts a request, which also cancels it on the server. The page is built into `pumice`, loads nothing from outside, keeps audio and results in memory only and is served with a strict content security policy behind the usual Host/Origin check. From another machine, use an SSH forward to the same port, or `tailscale serve` with the next item.
- **`serve.allowed_origins`** lists browser origins that `pumice serve` accepts besides `http://127.0.0.1:<port>`, so a reverse proxy on the same machine such as `tailscale serve` (tailnet-only HTTPS) can reach it while it keeps listening on loopback only. `[]` accepts none; entries are exact origins, checked at load. There is no authentication: whoever can reach the proxy can use the service and your AI subscriptions.
- **Initial settings are created automatically.** When a command first needs a missing per-user settings file, Pumice writes the four module configs and sample formatter from its supplied examples before validation. Existing files are preserved; incomplete files and missing explicit `--config` paths remain errors. Help, version and optional diagnostics create no files, and startup never downloads models.
- **npm installs include local transcription.** The Windows x64, Linux x64 and Apple Silicon platform packages now carry `pumice-transcribe` with its engine libraries, CPU variants and GPU module (Vulkan on Windows and Linux, Metal on Apple Silicon) next to `pumice`, so `pumice transcribe` works after `npm install -g @brasillero/pumice`; it runs on the CPU unless asked for the GPU, and models are downloaded separately. The Intel Mac package keeps `pumice` only. The Linux package now needs glibc 2.35 or newer and declares `libc: glibc`, so Alpine and other musl distributions are no longer supported through npm (the portable archive still runs there). The packages grow to about 25 MB (Windows), 27 MB (Linux) and 7 MB (Apple Silicon) compressed.
- **Complete third-party notices.** Every npm package and release archive ships a `THIRD_PARTY_NOTICES.md` generated for what it contains: the Rust dependencies of `pumice` (and of `pumice-transcribe` where included) with their license texts, plus the native code built in (transcribe.cpp, ggml, libopus, the Vulkan headers, musl) and the Rust standard library with its separately licensed parts (Unicode tables, compiler-builtins and libm, the crates it links). MPL-licensed crates (Symphonia) are listed with links to their exact, unmodified source.

### Changed

- **`pumice` is now the module dispatcher.** Bare `pumice` prints the command overview and exits; the relay runs as `pumice relay` / `pumice relay serve` (also in-process: one executable still serves); `pumice transcribe …` runs the `pumice-transcribe` installed next to it — replacing itself with it on Unix, and on Windows waiting out console Ctrl-C and exiting with the child's full exit code. The old `pumice` / `pumice --plain` invocations no longer start the service.
- **Breaking: every setting is written in YAML.** `relay.yml`, `transcribe.yml`, `formatter.yml` and `orchestrator.yml` have no built-in defaults anymore: every setting must be written, and an omitted one stops the module with the file and line of the mapping it belongs in (line 1 for a top-level one). `null` stays valid where it means something: `language: null` (detect), `default_model: null`, `server.port: null` (`serve` needs `--port`) and `models_dir: null` (the per-user models folder) in `transcribe.yml`; `default: null` in `formatter.yml`; `modules.formatter: null`, a flow's `formatter: null` and `serve.transcription_model: null` while serve is disabled in `orchestrator.yml`; `model: null` on a disabled relay provider. Newly required: the relay's `port`, `total_timeout_secs`, `max_parallel`, `providers`, `debug_log.enabled` and `debug_log.path`, and every provider entry's `model`, `binary` (`binary: null` is gone: write the command, e.g. `binary: claude`) and `timeout_secs`; transcription's `default_model`, `language`, `threads` (`auto` keeps the available parallelism, capped at 32), `backend`, `server.port` and `models_dir`; the formatter's `formatters` and `default`; the orchestrator's `modules.formatter`, every flow's `enabled` and four `budgets_ms`, and the whole `serve:` section (`enabled: false` keeps `pumice serve` off). Commands that only report what exists also run without their module file: `pumice relay doctor` (without `--login-check`), `pumice-transcribe models catalog|download|verify` and `backends --backend MODE`, `pumice format list` and `show builtin:clean`. The `*.example.yml` files write every setting with the old default values; `check-config` names each missing one.
- **The relay's default configuration file is now `relay.yml`** (`pumice.yaml` is no longer read; the example file is `relay.example.yml`). The standalone `pumice-relay` binary is unchanged. See "Upgrading from 0.2" in the installation guide.
- **Breaking: every relay provider entry writes `effort`, and the `claude` entry writes `thinking`.** `effort: low`, `medium` or `high` sets the reasoning effort of every call that provider makes, through `pumice relay` and `pumice run`/`pumice serve` alike; Pumice maps it to the CLI's own setting (Claude `--effort`, pinned in `CLAUDE_CODE_EFFORT_LEVEL` too; Codex `-c model_reasoning_effort`; Kiro `--effort`). `thinking: false` keeps Claude's thinking off as before; `thinking: true` lets Claude Code think the way it does by default for the model. Both keys are required, also on disabled entries (`effort: null` is an error), and a missing, `null` or unknown value stops the relay with the file and line; `thinking` on any other provider is rejected at its line. A model the CLI is known to run without effort is refused at its `effort` line, whatever the level, because none would be honoured: Claude Code's `claude-haiku-4-5`, `claude-sonnet-4-5`, `claude-sonnet-4-0`, `claude-opus-4-1`, `claude-opus-4-0` and `claude-3-…` models in every spelling Claude Code reads as them (`claude-sonnet-4`, `claude-opus-4`, dated, Bedrock and Vertex ids), except the four legacy Opus ids it remaps to the default Opus at run time (`claude-opus-4-0`, `claude-opus-4-1` and their first-party dated ids), and Kiro's `claude-haiku-4.5`. Likewise `thinking: false` is refused at its line for Claude models that cannot turn thinking off (Haiku 5.5, Sonnet 5.5, Opus 5.5, Fable 5 and 5.1, Mythos 5.1): Claude Code sends adaptive thinking for them anyway. Aliases such as `haiku` and models Pumice knows nothing about are accepted; on a first-party login (which, for Claude Code, includes a proxy set only through `ANTHROPIC_BASE_URL`) `haiku` is Haiku 5.5, which keeps thinking on even with `thinking: false`. To keep the previous behaviour, add `effort: low` to every entry and `thinking: false` to `claude`. Fast/priority modes stay off, and Codex still requests the standard service tier and no reasoning summary. `check-config` shows each enabled entry's effort and thinking.

### Removed

- **Breaking: the Kimi and archived Antigravity providers are gone.** Their plugins, tests and docs were deleted; a `relay.yml` that still lists `kimi` or `antigravity` stops with "provider was removed" at the entry's line. Delete the entry.

### Fixed

- `cargo install --path .` (and `--path crates/relay`) installs only `pumice` (`pumice-relay`); the test helpers `pumice-test-cli`, `pumice-root-test-cli` and `pumice-test-transcribe` are built only for the test suite.
- `pumice relay serve|check-config|doctor --help` (and `-h`, also on `pumice-relay`) prints that command's help instead of a usage error.
- On Windows, a CLI process left behind by a cancelled call can no longer keep the output pipes of `pumice` and `pumice-relay` open after they exit, which could hang whatever was reading them.

### Security

- On Windows, `pumice-transcribe` loads its engine libraries only from its own folder and System32, so a DLL with the same name in the current directory or on `PATH` is never loaded.

## [0.2.0] — 2026-10-11

### Added

- **Live request view (S4.6).** When `pumice` runs in a terminal it shows an interactive view of each completion request's lifecycle with timing, provider/model metadata, filter, search and details. `--plain` keeps the old one-line-per-request stderr output. Dictated text and replies only appear in the view when the debug log is enabled.
- **Live view: full-screen request details with Summary, Received, Parsed and Sent tabs, and an app column (S4.7).** `Enter` opens a request's details full screen, `Tab`/`1`–`4` switch tabs, `?` shows every key, and the bottom line shows the keys of the current screen. Received shows the request as it arrived (credential headers masked), Parsed the system prompt and the parts before, in and after the input, Sent the exact response; all three only with the debug log on. The app column shows the client's `X-Title` header (Handy sends one).
- **Kimi plugin (S2.4).** `id: kimi` runs the Kimi Code CLI in print mode with a one-off no-tools agent file in the call's temporary directory; the dictation goes in as a single argument (capped at 24 KiB, larger dictations are refused), telemetry and update checks are off. Your `~/.kimi-code` login and settings are used.
- **Kiro plugin (S2.14).** `id: kiro` runs `kiro-cli` headless with no tools trusted and `--effort low`; the dictation goes in on stdin, and the app's system prompt goes into a one-off no-tools agent file in the call's temporary directory (Kiro has no system-prompt flag). A system prompt starting with `file:` is refused, because Kiro would read it as a file reference. Your `~/.kiro` login and settings are used but never changed.
- **A plain landing page at `/`.** Opening Pumice's address in a browser shows a short page saying what it is, with a link to the source code, instead of a JSON 404.
- **Parallel requests.** Up to `max_parallel` requests (default 4, at most 32) run at the same time; extra requests wait in line instead of getting an immediate busy error, and one still waiting when its time budget runs out gets HTTP 503.

### Changed

- **A request that cannot be formatted is answered with an HTTP error** instead of a normal reply containing the transcript: 400 (no model), 404 (unknown or disabled provider), 429 (rate limit or quota), 502 (provider failure or empty reply), 503 (busy), 504 (timeout). The message is a text-free reason, the log line reads `FAILED  HTTP <status>: <reason>`, and the app pastes its own saved transcript (Handy and OpenWhispr both do).
- **Pumice is a pure pass-through.** It no longer adds its own formatting instruction, and plain text is sent as is (no `<transcript>` envelope, no tag escaping). The app's prompt alone decides the task. When the app sends no system message, Claude gets an empty system prompt; Codex and Kimi, which refuse an empty one, get the neutral line `Follow the user's message.`
- Every call uses the cheapest settings, even when the user's CLI config asks for more: Claude runs with `--effort low`, thinking off and fast mode off (also as a per-call settings layer, so a settings `env` block cannot undo them); Codex pins the standard service tier, turns the fast feature off and requests no reasoning summary (effort stays `low`); Kimi forces thinking off for models served by Kimi's own provider.
- Claude, Codex and Kimi no longer reject a reply because the CLI's output shows tool activity: each plugin returns the final answer, and tools stay switched off in the invocation (owner decision).
- **Provider `env:` and `options:` are removed.** Each CLI keeps its own configuration (gateway, routing, login) and Pumice inherits it: set Claude's `ANTHROPIC_BASE_URL` in `~/.claude/settings.json` and Codex's `openai_base_url` in `~/.codex/config.toml`. A file that still has `env:` or `options:` fails at that line. The generic local-server adapter, whose only setting was `options.base_url`, can no longer be built from a config until it returns.
- **The `prompts:` section is removed.** The app that sends the dictation (Handy, OpenWhispr) sends its own prompt; Pumice adds none from the config. A file that still has `prompts:` fails at that line with a message saying so.
- **Antigravity and the generic local-server adapter are archived** until the plugin architecture settles: their code stays, but they are no longer registered, so listing one in the config fails with a message saying so, and `doctor` and `/v1/models` no longer show them.
- **OpenCode support is removed.** A config entry `id: opencode` now fails with a message saying so; delete the entry.
- Cleanup is minimal: Pumice keeps the model's reply as is apart from removing closed `<think>…</think>` or `<thinking>…</thinking>` blocks at the start and trimming outer whitespace. Preamble lines, wrapping quotes and code fences are no longer removed; the prompt is where formatting is controlled (owner decision, deviates from spec S3.4). Cleanup looks only at the reply, never at the dictation, and an opening `<think>` tag that never closes is kept as text. An empty reply fails the request, and the log says why (`the provider returned an empty reply`).
- Provider risk warnings are no longer printed at startup, in `check-config` or in `doctor`: every enabled provider is treated the same way (owner decision). The residual-risk notes stay in each adapter's source and research notes.
- **Configuration format changed in 0.2.** Pumice assumes nothing anymore: no provider is enabled by default, no model is built in, and there is no fallback chain: a request runs exactly one provider. `providers:` is now an explicit ordered list, and the client's `model` field picks the provider; a request without a model, like any failure, gets an HTTP error:
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
- Every completion request writes exactly one log entry: requests refused before processing are logged as `REJECTED`, failed requests as `FAILED`, both with the fixed error message, and a client that disconnects mid-request as `DROPPED`.
- Config validation: removed keys set to `null` (`default: null`, `default_provider: ~`, `fallback_order: null`) are rejected like any other value; an empty `binary`, and a `model` starting with `-` for a CLI provider, are rejected at their line; an empty `APPDATA` no longer resolves the config path relative to the current directory.
- A command-line argument that is not valid Unicode is a usage error (exit 2) instead of a crash.
- Kimi: a dictation full of quotes or backslashes that fits the byte cap but would overflow the Windows command line after escaping is refused as too large, instead of failing to start.
- Adapters reject a CLI reply that is not valid UTF-8 (Codex, Kimi, Antigravity) instead of returning text with `�` in it.
- Quota errors reported with HTTP 429 are logged as quota exhausted, not rate limited (Codex, Kimi); Codex also recognizes OpenAI's "exceeded your current quota" wording.
- Kimi: an early transient retry no longer hides a later login or quota failure.
- Generic: a malformed `tool_calls` value or a non-string `finish_reason` is rejected, and an error status with a stalled body keeps its classification instead of becoming a timeout.
- A CLI that exits successfully without reading its whole prompt is treated as a failure (HTTP 502) instead of answering a truncated prompt.
- A cancelled call's temporary directory is still removed, in the background, with the usual retries; a directory that cannot be removed is reported on stderr.
- An existing debug log file is made private (0600) when Pumice opens it, not only when it creates it.
- A missing `--config` file is reported once (`<path>: the configuration file does not exist`) instead of repeating the path.
- `doctor` rejects a flag given twice (for example two `--provider`) instead of silently using the last one.
- The debug log never overwrites a request body field with the recorded HTTP headers.
- "Unsupported launcher" wording is used everywhere for an unsupported CLI shim.

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
