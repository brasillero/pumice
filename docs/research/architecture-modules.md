# Pumice modular architecture

Analysis date: 2026-10-12. **Recommendations for owner review, not new decisions.** Scope: independently usable relay, transcription, formatter and orchestrator CLIs, with future interfaces above them. No implementation, model execution, AI CLI calls, production connections or service operations were performed.

Evidence: current `arch-modules` HEAD `d23ca9f0074f987f25daab64cfc239e0f857e37c`; `origin/s10.1-transcription` at `e64490246f00dfec9584b1681f6f18c6299c361d`. Below, **S:** means a file at that transcription ref, readable with `git show origin/s10.1-transcription:<path>`; other repository `file:line` references mean the current branch. **R:** means the locally installed Cargo registry source under `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`. External revisions are pinned in §10. **UNVERIFIED** marks behavior/builds not demonstrated by the inspected evidence.

## 1. Recommended architecture and ownership

Use **one Cargo workspace; a library and a thin binary per product module; separate relay and transcription processes in the running stack**. Keep pure formatter composition callable as a library inside the orchestrator, with its own standalone CLI and optional HTTP adapter. Do not require a formatter daemon to add a system message. Keep today's `pumice` command as a self-contained compatibility entry point; add explicit orchestration subcommands before considering any default change.

This accepts the workspace/library recommendation, qualifies “everything communicates through HTTP,” and rejects immediately repurposing bare `pumice` as a new multi-process stack. Independence means a stable contract and standalone use, not an obligatory server for every operation. Owner authority is `.task-decisions/tasks/arch-modules/decisions.md:10` (D1), `:25` (D2), `:38` (D3), `:53` (D4). D5 at `:66` assigns analysis/review to Codex and implementation to Kimi under Claude's supervision; this report does not change that process.

| Module | Owns | Must not own or depend on |
| --- | --- | --- |
| Relay | Provider routes/config, official AI CLI discovery/invocation, restricted temporary workspaces, relay deadlines/queue, CLI output decoding, existing minimal cleanup, chat API, relay diagnostics/logs/monitor | Audio/engine/model files, formatter presets, capture/hotkeys/paste, other module lifecycle |
| Transcription | Audio decoding/downmix/resampling, engine abstraction, GGUF model catalog/paths, backend/device/thread selection, warm model lifetime, admission/cancellation, transcription API/config/logs | AI subscriptions/credentials, relay providers, formatting prompts, clipboard or microphone capture |
| Formatter | Versioned declarative definitions, explicit preset selection, deterministic request transformation, validation and preview/list commands | Running AI CLIs, inference, provider selection, semantic reply cleanup, audio/models, subprocesses |
| Orchestrator | Explicit flow definitions, selected component endpoints/lifecycle, whole-flow budget, stage correlation/progress, request construction, combined API/CLI and aggregate status | Provider-specific flags, C++/GPU engine internals, formatting rule content, silent fallback/retry of paid calls |

The existing code already separates library from entry point (`src/lib.rs:1`), API from pipeline (`src/api/mod.rs:31`), and monitoring from terminal rendering (`src/monitor.rs:1`, `src/tui/mod.rs:6`). Preserve these seams; do not turn `Provider` into a general audio/formatting plugin (`src/providers/interface.rs:69`). Its current names `format`, `FormatInput`, `FormatOutcome` describe relay history, not the new formatter's ownership.

Relay “pass-through” is the existing **semantic subset**, not byte-for-byte forwarding of all OpenAI JSON: it accepts one user message plus system/developer messages, ignores extra options, reconstructs text, joins instruction messages, and minimally cleans answers (`src/request.rs:18`, `:158`, `src/prompts.rs:81`, `src/cleanup.rs:7`). Extraction must retain these behaviors initially. Fixing the Handy envelope or generalizing conversations is separate work with an open owner decision (`.task-decisions/tasks/plugin-contract/backlog.md:12`). No new module should depend on that envelope.

**Capture belongs outside these three modules.** A future platform integration layer used by the orchestrator/desktop shell owns microphone capture, hotkeys, overlay and paste. Headless audio-file flows are valuable first, but cannot themselves replace Handy. Require explicit user actions for capture/paste; dictated text never becomes a shell command. UIs submit flow requests and display results, without duplicating inference, routing or formatter composition.

## 2. External contracts

### Module interfaces

All proposed new commands support `--help`, `--version`, `--config`, `check-config` and a quota-free `doctor` appropriate to that module. HTTP is enabled only by `serve`; one-shot commands need no listening socket. JSON on stdin means **one bounded UTF-8 JSON document**, not implicit JSONL or interactive input. Binary audio on stdin requires explicit `-`; upload filenames are never filesystem paths.

| Executable / mode | Input → stdout | HTTP adapter |
| --- | --- | --- |
| `pumice-relay serve` | Today's service, same TUI/`--plain` behavior | Existing `POST /v1/chat/completions`, `GET /v1/models`, `/health`, `/` |
| `pumice-relay request --request -` | Chat request JSON → Chat Completion JSON; `--output text` explicitly extracts answer | Same validation/provider service as HTTP; no server needed |
| `pumice-transcribe file.wav --model turbo` | Encoded audio → transcript text; `--response-format json` → `{"text":"…"}` | `POST /v1/audio/transcriptions`, `GET /v1/models`, `/health`, `/ready` |
| `pumice-format --formatter plain --request -` | Chat request JSON → transformed Chat request JSON | Optional `POST /pumice/v1/formatters/{id}/apply`; `GET /pumice/v1/formatters`; `/health`, `/ready` |
| `pumice orchestrate run file.wav --flow notes` | Audio → versioned flow-result JSON; `--output text` explicitly extracts successful final text | `pumice orchestrate serve`: standard proxy routes plus combined endpoint below |

`pumice-format list`, `show <id>` and `check-config` work without relay/AI/model installations. Formatter one-shot exits after transformation; running it does not consume quota. Relay one-shot does consume quota and must be documented accordingly. Service stdout retains the legacy listening line for legacy commands (`src/main.rs:363`); proposed managed mode uses machine-readable startup output instead. All one-shot progress/diagnostics go to stderr; stdout contains only the result. Text output preserves result bytes; document any newline, rather than silently adding one. Failed one-shot runs emit no successful/partial stdout.

Recommend exit **0** success, **2** invalid arguments/config/input, **1** operational failure (unavailable component, model/inference/provider failure, timeout), **130** interrupted on supported platforms. With `--error-format json`, stderr carries a stable safe code plus module/stage; consumers must not parse prose or infer HTTP status from exit 1. This extends today's 0/1/2 and TUI forced-exit 130 (`src/main.rs:128`, `:136`, `src/tui/mod.rs:89`), without silently altering old commands.

OpenAI applies to **chat completion and transcription request/response shapes**, model lists and error envelopes. It does not define formatter transforms, health, lifecycle or audio→formatted-text flows. Document these as Pumice contracts under `/pumice/v1`, never return a request object from `/v1/chat/completions`. Continue existing chat restrictions and buffered SSE (`src/api/handlers.rs:640`); do not advertise full conversation, tools, multimodal or incremental token streaming support. The [official Chat Completions reference](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create) distinguishes request messages from generated choices.

For new endpoints, use safe errors shaped `{"error":{"message":"…","type":"…","param":null,"code":"…"}}`; internally preserve typed errors and map them at adapters. Existing relay errors currently contain only message/type (`src/api/types.rs:81`); adding optional fields needs a focused compatibility PR, not an extraction change. Never expose native/CLI error strings, request text, local model paths or filenames. Unsupported requested capabilities fail explicitly, without successful approximation.

### Transcription's supported OpenAI subset

Keep S10.1 D2's multipart endpoint (`S:.task-decisions/tasks/s10.1-transcription/decisions.md:23`). Require uploaded `file` and exact configured `model` ID; support optional `language`, `prompt`, `temperature`, and `response_format=json|text`. Default JSON returns `{"text":"…"}`; plain text returns the engine transcript. The [official audio reference](https://developers.openai.com/api/reference/resources/audio/subresources/transcriptions/methods/create) also defines model-dependent timestamps, streaming and diarization; compatibility does not require claiming those capabilities. Do not use the STT `prompt` as a formatting prompt: it is transcription context.

First API release: PCM WAV, MP3, FLAC, Ogg/Vorbis; reject recognized unsupported codecs with 415. Propose Symphonia plus rubato, as evaluated in `S:.task-decisions/tasks/s10.1-transcription/reports/2026-10-12-endpoint-design.md:63`. M4A/AAC-LC and WebM/Opus should follow actual client upload requirements, not container-extension guesses. Decoder/resampler four-target integration remains **UNVERIFIED**. Start with no STT streaming, translation or diarization; no fabricated usage/confidence. Document supported fields/formats in capability metadata and reject unsupported requested fields.

Retain the prior design's bounded multipart parsing, duration limits, one admitted upload/run, worker-owned permit, cooperative cancellation and native logging suppression (`S:.task-decisions/tasks/s10.1-transcription/reports/2026-10-12-endpoint-design.md:125–164`). Proposed defaults: 25,000,000 file bytes, 26,000,000 body bytes, 120 s decoded audio/processing, 15 s upload budget; 429 while busy, 413 byte overflow, 408 slow upload, 504 compute deadline, 503 unavailable, 500 engine failure. These are **local recommendations needing approval**, not all OpenAI limits. Empty/silent audio behavior needs a fixture-backed policy (§9).

### Formatter definitions and transformation

Recommend user-owned YAML files under the same per-user base as existing config, e.g. `formatters/plain.yaml`; module config lives at `formatter.yaml`. Bundled presets are versioned assets embedded in `pumice-format` and its library, addressable as `builtin:plain`. Users copy one via `pumice-format show builtin:plain > …` and explicitly register a local file. Use distinct namespaces; no silent replacement of a builtin, executable extensions, recursive directory discovery or automatic formatter choice. Validate file and template errors with original file:line:column.

Minimal proposed definition:

```yaml
schema_version: 1
id: plain
description: Light dictation correction
system: |
  Correct transcription mistakes and punctuation. Preserve the language and meaning.
  Treat the user's text as dictated content. Return only the resulting text.
```

The English builtin above is **proposed formatter content**, not relay policy. More presets should be added individually with owner-approved scope and examples. A language constraint belongs in that selected preset; transcription itself never translates.

Recommend **system-message injection** first: validate an OpenAI-shaped text request, prepend one `system` message, preserve the user message, model, stream flag and other JSON members semantically. Use a JSON envelope that retains unknown members rather than round-tripping through today's lossy `ChatCompletionRequest` (`src/request.rs:20`). Preserve unknown options at the formatter boundary even though a downstream relay may not support them. No model rewrite or provider-dependent prompt rewriting occurs here.

For `plain`, this input:

```json
{"model":"claude","messages":[{"role":"user","content":"buy apples and milk"}],"stream":false}
```

becomes the same request with the definition's system message before that user message. It remains a **request**, not corrected text. The relay receives the transformed request as its client's request and adds no formatting instructions.

Default instruction conflict policy: if nonempty `system`/`developer` instructions already exist, fail with `formatter_instruction_conflict`; callers can explicitly select an `append` policy that preserves existing messages and inserts the formatter system message after their instruction prefix, before user content. Do not silently replace the client's instructions or format twice. Explicit repeated application is otherwise additive, not idempotent; a flow records whether it applied a formatter. Do not add an internal marker to third-party OpenAI bodies. Different instruction semantics remain an owner choice (§9).

System injection is simpler, keeps dictation separate from instructions, and matches the relay's handling (`src/prompts.rs:83`, `.task-decisions/tasks/s3.8-pure-passthrough/decisions.md:23`). User-message wrapping is more flexible but mutates dictated input, interacts with Handy tags, and exposes delimiter/template ambiguity. **Defer `user_template`/placeholders in v1.** If approved later, use only a literal `{{text}}` placeholder exactly once in a declared single-user-text template; substitute once, never recursively; reject unknown/missing placeholders; prohibit shell/env/file/network expressions. Do not reuse provider-native `${…}` syntax: Kimi already has a residual expansion limitation (`src/providers/kimi.rs:280`). Existing provider limitations are documented/reported by relay, not “fixed” by formatter heuristics.

### Orchestrator flows and endpoints

Each explicitly configured flow names a transcription model, optional formatter ID and relay route, plus its own total budget. Absence of a formatter means raw transcription. No missing-model default is introduced into relay. A flow may route to third-party loopback OpenAI-compatible transcription or completion servers; remote HTTP is a separate network-policy decision (§9).

On its **separate frontend port**, expose:

- `POST /v1/audio/transcriptions`: transparent raw-transcription proxy; no formatting on this route.
- `POST /v1/chat/completions`: direct relay proxy for existing relay IDs; optional explicitly named formatter routes such as `notes-claude` map to one formatter plus `claude`. Return a genuine Chat Completion, reporting the requested public route ID. No implicit formatting for direct provider IDs.
- `GET /v1/models`: advertised enabled public routes/models without collisions; `GET /pumice/v1/capabilities` specifies endpoint support per ID. OpenAI's model list alone cannot express that distinction.
- `POST /pumice/v1/dictations`: Pumice multipart `file`, required `flow`, optional `language`; version 1 is synchronous and nonstreaming. A flow pins its models/preset; do not overload OpenAI's audio `model` or `prompt` to select an unrelated formatter/provider.
- `/health`: frontend liveness; `/ready`: readiness of enabled flows; `GET /pumice/v1/events`: bounded metadata SSE for a future monitor/UI, separate from OpenAI completion SSE.

For the custom dictation route, recommend a versioned result carrying **raw and final text separately**, e.g. `{"version":1,"id":"…","flow":"notes","transcript":{"text":"…"},"result":{"text":"…"},"timings_ms":{"transcription":0,"relay":0}}`. Actual timings replace the illustrative zeros. For transcription-only flows, `result.text` equals raw text. On formatter/relay failure return non-2xx, a safe error and (only on this custom route) the already completed `transcript`; omit successful `result`. This is response data, not logging or automatic provider fallback. It lets a future UI explicitly offer raw text. Standard OpenAI proxies retain their ordinary error envelope. No transcript exists if transcription failed; the capturing client retains its audio until it accepts a result.

Do not call an audio→formatted-text response a transcription response; doing so erases the only copy of the original transcript and misrepresents OpenAI semantics. The combined custom API is justified because no corresponding OpenAI audio-in/formatter/CLI-out contract exists. Audio Chat Completions and Realtime are different APIs and are unnecessary for this first flow.

## 3. Process model, lifecycle, configuration and visibility

| Option | Advantages | Costs / recommendation |
| --- | --- | --- |
| All modules as HTTP processes | Language-independent, warm inference, isolated native crashes, inspectable third-party integration | Ports/supervision, extra daemons; useful for relay/STT, excessive for pure formatter |
| One process importing all libraries | Easy startup, no internal transport, cheap pure composition | Native crash/hang can kill relay; native build graph affects deployment; conflicts with D1's rejected combined relay |
| Libraries + standalone adapters, selective processes | Direct unit tests/CLI reuse; HTTP where useful; static relay without native dependencies | Two adapters need contract tests; **recommended** |

Orchestrator links formatter/core contracts, **never transcribe-cpp**. It normally talks HTTP to long-lived relay/STT children or explicitly configured existing servers. CLI one-shot requests call each module's own service library; they do not spawn that module's server. One-shot orchestration can use existing servers or scoped children that it starts and stops; keep model warm via `orchestrate serve` for repeated dictations.

For every component, configure either `managed` (explicit executable/config paths) or `external` (explicit loopback URL). Never infer ownership from an open port or kill a discovered server. External components outlive orchestrator shutdown. Managed children are grouped/tracked by handles; reuse platform lifecycle primitives, not the current AI-specific `ProcessRunner` API, which assumes temporary workspaces and provider errors (`src/process/mod.rs:33`, `:79`). Unix process groups and Windows Job Objects already exist (`src/process/tree.rs:1`); isolate that reusable primitive only when a second consumer needs it.

Managed modules bind **`127.0.0.1:0`** in an explicit private-launch mode and announce their actual URL/instance ID/protocol version over a startup pipe. Keep the socket bound while announcing; parent-picked “free ports” create a race. Today YAML rejects port 0 (`src/config/mod.rs:210`), so private launch is a new CLI contract, not an unannounced config change. Standalone relay keeps 7567. New public STT/frontend/formatter ports require explicit selection initially; do not reserve adjacent production ports by guess. A fixed-port collision must fail visibly, never evict an existing process.

New modules expose lightweight liveness plus readiness/capability metadata. Readiness verifies model/backend initialization and supported protocol, without transcription or login calls. Managed readiness includes matching instance ID, not just an HTTP 200 from an unrelated process. Load one explicitly selected STT model before ready; missing model fails STT startup only. Keeping a frontend live while STT restarts is useful, but affected flows return 503. Relay stays usable.

Supervise only owned children: stop admission, cancel/drain with a bounded grace period, then terminate the owned process tree if necessary. Distinguish Ctrl-C and service SIGTERM/Windows termination; **current `api::shutdown_signal` only waits for Ctrl-C** (`src/api/mod.rs:189`). Lifecycle support is a separate PR with fake children. Native cancellation latency is **UNVERIFIED**; a hung STT process may require termination/reload. Keep its capacity permit until native computation actually ends. Use bounded exponential restart delay and a circuit breaker for repeated startup/crash failures; never replay a request or switch AI providers automatically. Explicit `backend=auto` may select among compute devices at initialization (§6), not replay failed inference unnoticed.

Recommend **separate strict YAML configs**, with orchestration referring to their paths:

- Legacy `pumice.yaml` remains **relay config with exactly its current root schema**.
- New `transcription.yaml`, `formatter.yaml`, `orchestrator.yaml` own their domain settings. File paths resolve relative to the containing config, not process cwd.
- Orchestrator owns only component locations, flow wiring, frontend/admission/whole-flow budgets and its own observability. It does not duplicate provider settings, formatter rule text or model/backend configuration.
- `orchestrate check-config` validates its file and managed config references, preserving original source spans; component validation remains callable without starting inference or AI CLIs.

An illustrative `orchestrator.yaml` fragment (new schema, not keys for `pumice.yaml`):

```yaml
schema_version: 1
components:
  relay:
    enabled: true
    external: http://127.0.0.1:7567
  transcription:
    enabled: true
    managed:
      binary: /installed/path/pumice-transcribe
      config: ./transcription.yaml
formatter_config: ./formatter.yaml
flows:
  - id: notes
    enabled: true
    transcription_model: turbo
    formatter: builtin:plain
    relay_model: claude
    total_timeout_secs: 60
```

The external relay address illustrates existing configuration only; it was not contacted. Module configs register explicit enabled model/definition IDs and their local paths. Reject component `managed`/`external` combinations, unknown references and duplicate IDs before launch. A standalone one-shot invocation explicitly selects its model/preset; an orchestrator never discovers and enables modules merely because binaries are installed.

One big YAML is convenient initially, but couples standalone modules and upgrades; copying/generated module configs loses provenance and risks diverging settings. A single setup command can later edit several files explicitly. Separate files require several validations but keep ownership clear. Do not add new keys to today's strict file (`src/config/raw.rs:29`), nor revive its rejected `prompts:` (`src/config/mod.rs:203`).

Every module owns metadata logs and optional private debug output. Keep relay TUI unchanged in extraction; new transcription CLI can initially use plain logs. Add generic stage events later rather than fake relay attempts (`src/monitor.rs:78`, `src/logging.rs:142`). Proposed metadata schema: version, module, instance/request/parent IDs, stage, configured model/route, bytes, timings, status, safe error code. Use bounded channels/ring buffers; slow/disconnected UI consumers cannot stall inference. Full monitor replacement is not prerequisite to STT.

Orchestrator correlates its calls with a bounded generated request ID, combines stage metadata, and offers SSE snapshots/progress for UI. It cannot promise every internal event of an external server; advertise available observability. Start by measuring stage calls; add module event adapters when needed. Never copy arbitrary stderr into HTTP errors or UI logs: relay/native diagnostics may contain text. Disable native sinks before model load (`R:transcribe-cpp-0.3.1/src/logging.rs:46`), sanitize subprocess streams, and keep dictation/prompt/raw-request capture opt-in per module. Never log audio bytes; aggregated debug must not implicitly enable a child's debug log. Auth/cookie headers remain masked, and production service logs stay usable without a TUI (`docs/install.md:83–101`).

Whole-flow budgets belong to orchestrator; module queue/inference/CLI deadlines remain enforced for standalone use. Parent requests cap stage budgets by remaining time; validate/cap any internal timeout header, never grant more time. HTTP disconnect cancellation needs explicit drop-guard propagation and fake-process tests: closing a connection alone is not proof that downstream computation stopped. This recommendation addresses the **open** deadline question without declaring it decided (`.task-decisions/tasks/plugin-contract/backlog.md:76`).

## 4. What to copy from T3 Code

Inspected public T3 Code revision `5fe7568e871c0310c83fac0484fb84bba14d9148` (URLs in §10). Its server package installs the `t3` CLI; server commands resolve configuration and invoke the same server implementation, with a headless `serve` mode (`apps/server/package.json:10`, `apps/server/src/cli/server.ts:15`, `:60`). Desktop main composes a backend pool; each backend instance owns child state, output log, readiness and restart lifecycle (`apps/desktop/src/main.ts:190`, `apps/desktop/src/backend/DesktopBackendManager.ts:667`, `:1030`). These are direct source observations, not a claim that its stack is Rust.

Its [remote architecture](https://github.com/pingdotgg/t3code/blob/5fe7568e871c0310c83fac0484fb84bba14d9148/docs/internals/remote.md#L3) assigns execution/state to the environment and uses HTTP/WebSocket clients. It explicitly separates access from process ownership: an already running server survives client disconnect (`:71–89`). Desktop's renderer can be served from bundled files, while API traffic goes to the chosen backend (`:94–102`). Shared contracts define HTTP/RPC shapes (`packages/contracts/src/environmentHttp.ts:4`, `packages/contracts/src/rpc.ts:24`).

Apply those principles: a reusable headless server/CLI, one lifecycle owner, independent UI clients, shared versioned wire contracts, readiness/capability negotiation, and “stop only what you started.” A future desktop shell may start/attach to `pumice orchestrate serve`; browser/terminal clients simply connect. Optional UI-side OS integration reports capture/paste state over explicit contracts. Embedded legacy TUI is compatible with D4 because it consumes monitor events above the relay library; moving it out is later work.

Do not copy T3's coding tools, credential integrations, persistence/event-sourcing breadth, remote transports, telemetry or updater permissions. Its backend pool manages execution environments; it is **not evidence that every domain module should be a daemon**. Pumice needs a much smaller flow controller and metadata event stream. SSE is sufficient for initial one-way progress; bidirectional WebSocket RPC becomes useful only when actual UI control requirements justify it.

## 5. Repository and dependency layout

Recommended eventual layout; introduce it incrementally, not as a prerequisite rewrite:

```text
Cargo.toml                         workspace, resolver="3", shared version/profile
Cargo.lock                         one product workspace lockfile
crates/
  relay/                           package pumice-relay; lib + pumice-relay binary
  transcription/                   package pumice-transcription; lib + pumice-transcribe
  formatter/                       package pumice-formatter; lib + pumice-format; presets/
  orchestrator/                    package pumice; lib + pumice compatibility/orchestrator CLI
  openai/                          package pumice-openai; wire envelopes only
  support/                         package pumice-support; narrow located-config/privacy helpers
tests/fixtures/                    shared provenance; tests live with their owning crates
scripts/                           build/archive/npm tooling and explicit artifact manifests
spikes/stt/                        isolated research workspace, excluded from product workspace
```

Library dependencies flow toward contracts/support, never toward orchestration. Relay and transcription do not depend on each other. Orchestrator depends on formatter and loopback HTTP adapters; its dependency on relay library exists to keep bare `pumice` self-contained. Native STT is never a dependency of `pumice`, `pumice-relay`, formatter or shared crates. “Both libraries and binaries” means reuse within each domain, **not Rust dynamic ABI plugins**.

Start with root `[package] pumice` plus `[workspace]`, preserving `cargo build --bin pumice`, version extraction and test entry points. Add transcription as a workspace member. Later move relay internals mechanically, retain compatibility imports while moving tests, then adjust packaging before adopting a virtual workspace. Choose `default-members` covering model-free module suites; native engine optional during development with an explicitly feature-enabled production job. The [Cargo workspace reference](https://doc.rust-lang.org/cargo/reference/workspaces.html) explains shared lockfile, resolver and default-member selection.

Move `api`, `pipeline`, `providers`, `process`, `request`, `prompts`, `cleanup`, `doctor`, relay config/logging/monitor/TUI together initially. Splitting providers into four crates or prematurely extracting generic runtime/config traits adds churn without independence. Leave relay's current type names/cleanup behavior untouched during that move. `src/config/mod.rs:21` imports provider settings; it cannot become everyone's generic config crate unchanged. `src/process/mod.rs:33` depends on provider errors; separate lifecycle from AI invocation only for demonstrated reuse.

`pumice-openai` should hold minimal chat/audio/model/error wire envelopes with unknown-field retention where transformations need it, not provider IDs, Handy parsing, Axum router state or native engine types. It need not model all OpenAI features. `pumice-support` can share config source locations/path anchoring, redaction and safe metadata writing; errors remain module-owned enums. The existing `ConfigError` and parser spans are a useful starting point (`src/config/error.rs:12`, `src/config/raw.rs:3`). Keep concrete relay outcomes/debug records inside relay until a second consumer proves a generic abstraction worthwhile.

Only transcription has backend features: e.g. `engine-transcribe`, `backend-vulkan`, `backend-metal`, `backend-cuda`, `dynamic-backends`. Avoid workspace `--all-features`: CUDA/Metal/Vulkan have platform/toolchain restrictions. Build each published package with an explicit feature matrix and isolated native build/cache directory; Cargo feature unification must not contaminate relay. Keep public types independent of engine features so fake-engine API tests compile without C++/models. Pin the known engine until an upgrade has its own validation; current Handy main already uses **0.4.0**, unlike the proved **0.3.1** (`Handy src-tauri/Cargo.toml:79`, §10).

CI should keep fast model-free Linux/Windows/macOS tests, add fake-engine/formatter/flow contract tests, and separately compile native release feature sets. Tests use ephemeral port 0, fake CLIs/components, small licensed audio fixtures and failure/disconnect gates. Real GPU quality/speed checks are optional/manual hardware jobs, not emulated success on GPU-less hosted runners. Expand `release.yml` paths to `crates/**` and explicit native tooling/manifests; today its filters/build know only root `src`/`--bin pumice` (`.github/workflows/release.yml:24`, `:93`).

## 6. Transcription under D2: quality, acceleration and models

Recommend **transcribe-cpp as the first engine**, with Whisper Large v3 Turbo Q8_0 as a documented tested model, not an auto-enabled model. It already shares Handy's GGUF files and has CPU evidence across four targets (`S:docs/research/S10.1-transcription.md:11`, `:120`). A replaceable engine seam permits future ONNX/other engines; there is no evidence here that a rewrite or a new engine is better. One Portuguese sample cannot establish accuracy across languages, microphones, silence or noisy dictation; quality versus Handy remains open (`S:.task-decisions/tasks/s10.1-transcription/backlog.md:34`).

“Best possible” should mean: accurate transcripts on agreed samples; low warm/cold p50/p95 latency on target Windows hardware; bounded RAM/VRAM; working CPU fallback when GPU initialization is unavailable; portable CPU bootstrap; understandable backend diagnostics; reliable installation without development toolchains. Publish model/backend/thread/quantization and correctness results together. Recommend warm inference faster than real time on the main supported machine, measure short clips/long clips, and prioritize end-of-dictation latency. These are proposed acceptance goals, not measured GPU guarantees.

Existing baseline: turbo on server-1's 20-thread i5-14500T took ~5.9 s native, ~7.1 s AVX2, ~20.5 s conservative x86 for 14.19 s audio; RSS ~1 GB and model load ~0.35 s (`S:docs/research/S10.1-transcription.md:68–78`). Musl reruns were ~7.6–8.4 s (`:114`). These cannot predict the owner's Windows GPU/CPU. Thread count must be configurable; 20 is a measurement setting, not a universal optimum.

**Runtime CPU selection is already supported in the proved dependency's source.** `R:transcribe-cpp-0.3.1/Cargo.toml:44` makes `dynamic-backends` imply `shared`; `R:transcribe-cpp-sys-0.3.1/bindings/rust/sys/build.rs:154–168` enables `TRANSCRIBE_GGML_BACKEND_DL`, x86 `GGML_CPU_ALL_VARIANTS` and conservative bootstrap. `ggml/src/CMakeLists.txt:487` requires backend loading for variants; `ggml/src/ggml-backend-reg.cpp:587` scores loadable CPU variants. Ship one baseline host plus matching baseline/AVX2/higher-ISA modules, selected at runtime, rather than globally compiling the host with AVX2. This replaces D5's proposed non-AVX2 exclusion **if the owner approves revisiting D5**.

Dynamic modules require **libtranscribe and ggml runtime libraries too**, not merely extra CPU DLLs. Initialize once before enumeration/model loading from a trusted installation-relative directory (`R:transcribe-cpp-0.3.1/src/backend.rs:120`, `:237`; `R:transcribe-cpp-sys-0.3.1/src/transcribe.cpp:1187`). Never load from the caller's current directory, uploaded filenames or request-controlled paths. Raw ggml's default search includes cwd; its explicit directory path does not (`R:transcribe-cpp-sys-0.3.1/ggml/src/ggml-backend-reg.cpp:539–554`). Stage runtime directories using the sys/wrapper build metadata (`R:transcribe-cpp-sys-0.3.1/bindings/rust/sys/build.rs:487`). Exact packaged variant coverage, baseline behavior and missing-library handling on all targets are **UNVERIFIED**.

| Platform | Recommended standard transcription build | Trade-off / verification needed |
| --- | --- | --- |
| Windows x64 | Runtime CPU variants + Vulkan; native DLLs in private module directory | Broad GPU vendor coverage; drivers required; prove bootstrap/CRT/DLL search and CPU-only behavior on a clean machine |
| Linux x64 | GNU/glibc runtime CPU variants + Vulkan with explicit glibc floor; separate portable CPU musl archive retained initially | Dynamic drivers/modules cannot inherit relay's fully static musl contract; test oldest supported glibc and driver-free startup |
| macOS arm64 | Metal plus portable ARM CPU path, static native core where practical | No x86 variant selection; verify framework/embedded shader packaging, Intel versus Apple GPU coverage separately |
| macOS x64 | Portable CPU first; Metal when verified on supported Intel hardware | CPU already proved; Metal speed/compatibility **UNVERIFIED**; no promise based on ARM results |

Vulkan first on Windows/Linux is consistent with [current Handy's backend matrix](https://github.com/cjpais/Handy/blob/a42131d24c59315fede06775cd354219c4f4a87b/src-tauri/Cargo.toml#L130). Its [build staging](https://github.com/cjpais/Handy/blob/a42131d24c59315fede06775cd354219c4f4a87b/src-tauri/build.rs#L7) uses shared Linux/Windows libraries and static Metal on macOS. This corroborates a packaging approach, not Pumice's GPU correctness/performance. Vulkan depends on a loader and driver-provided implementations ([official loader guide](https://docs.vulkan.org/guide/latest/loader.html)). CUDA is a later optional NVIDIA backend pack/build, not the universal default; validate toolkit/runtime redistribution and driver minimums ([NVIDIA Windows guide](https://docs.nvidia.com/cuda/cuda-installation-guide-microsoft-windows/)). Do not require users to install the developer SDK just to run.

Static musl cannot normally load dynamic backends; the [musl maintainer explains the static/dlopen limitation](https://www.openwall.com/lists/musl/2021/09/24/6). Keep **relay musl** intact while making accelerated STT a GNU target; do not offer a “static GPU executable” based on the CPU spike. Musl/Alpine users can retain the CPU-only portable archive; publishing a dynamic musl/GPU build is later work requiring evidence. GPU drivers are OS/hardware prerequisites, not libraries Pumice can bundle away.

Recommend `backend: auto|cpu|vulkan|metal|cuda` and an optional stable device selector in transcription config. `auto` enables only shipped/probed compatible backends, with CPU available; explicit backend/device fails if unavailable. Report the actual device/backend; do not equate ggml enumeration order or score with measured GPU speed. Mask unwanted backends before registration to avoid initializing a broken/unwanted driver (`R:transcribe-cpp-0.3.1/src/backend.rs:160`, `:237`); backend masks cannot be freely changed after registration. Change backend packs/selection by process restart. Device queries/load failures are safe typed diagnostics; no per-request loading/retrying random DLLs.

Initially keep one loaded model/one admitted inference, retain it while serving, and allow a smaller model explicitly for low-memory machines. The model's compute lock serializes sessions; cloned handles do not create parallel inference capacity (`R:transcribe-cpp-0.3.1/src/model.rs:9`). Multiple workers/models multiply memory; add only after load tests. More complete word timestamps, streaming/VAD and alternate families should be explicit capabilities, not free additions justified by a crate feature name.

**Model ownership:** register IDs with explicit local paths and optional pinned hash/provenance; OpenAI requests select IDs, never arbitrary disk paths/URLs. Resolve paths against `transcription.yaml`. User-supplied external files remain read-only and must never be deleted by Pumice. Permit explicit reuse of Handy's GGUF directory; do not edit Handy settings, duplicate its model database or assume a Windows path from Linux. Current Handy resolves its model directory via portable-aware app data then `models` (`Handy src-tauri/src/managers/model.rs:554–558`); precise installed paths/portable location are **UNVERIFIED**. Offer explicit `models register/list/verify`; `models import` can copy a selected file when the user requests independent storage.

Keep models outside release/npm/install folders in a per-user data directory, separate from configs and versioned backend packs. Never bundle the 886 MB turbo model; inference should work offline. **Recommend local registration first, no downloader.** AGENTS.md:37 still forbids application outbound calls, and `S:.task-decisions/tasks/s10.1-transcription/backlog.md:47–50` leaves the exception open. An eventual explicit `models download <catalog-id>` needs owner approval: pinned revision/hash/license/size, bounded resumable download, atomic verified install, no auto-download at startup, no arbitrary request URL fetching and no telemetry. Research/build-time fetches are not permission for a production downloader.

## 7. Distribution and backwards compatibility

Recommend **independent archives first, optional versioned suite bundle later**. Relay must remain a single self-contained executable: Windows MSVC `+crt-static`, Linux static musl, macOS with only OS libraries/frameworks. “Fully static macOS” is not the existing guarantee; the spike explicitly permits system dynamic libraries (`S:spikes/stt/ci/verify-unix.py:20`). Test relay dependency closure and imports independently of transcription. Canonical `pumice-relay` and legacy `pumice` are distinct build outputs using the same relay library; users downloading legacy `pumice` still need no sibling binary.

Transcription archives include `pumice-transcribe`, exact native library/backend files, licenses and an artifact manifest listing version/ABI/hash/platform/features. Executable and native libraries are an atomic update unit; never mix ggml versions from two installs. Keep all runtime artifacts in the application tree, not Windows system directories or global PATH. Rust and native CMake CRT settings are separate (`S:docs/research/S10.1-transcription.md:136`). Start by retaining `/MT` for Windows components and verify DLL boundaries; if upstream/runtime builds require `/MD`, propose an explicit transcription-only exception and approved app-local redistributable packaging. D2 permits multiple files but does not itself settle CRT policy. Clean-machine CRT/GPU bundle behavior is **UNVERIFIED**.

Preserve `@brasillero/pumice` and its `pumice` bin as the lightweight compatibility package. New independent packages can follow the existing optional per-platform dependency pattern: `@brasillero/pumice-transcribe`, `…/pumice-format`, `…/pumice-relay`; scope/name availability is **UNVERIFIED**. A future opt-in `@brasillero/pumice-suite` can install a coherent full bundle without competing for the existing `pumice` launcher name (e.g. use `pumice-suite` as its entry point). Decide branding before publishing; do not silently make existing relay npm installs download GPU assets or models.

Reuse zero-download/no-shell launchers, exact native-version matching and inherited stdio (`scripts/npm/bin/pumice.js:4`, `:68`, `:84`). Keep Node ≥22 only for npm launcher use; archives need no Node/Rust/CMake/Python. For transcription native npm packages include all manifested sidecars; the current packer extracts only one binary and expects exactly four archives (`scripts/package_npm.py:13`, `:133`), so generalize artifact manifests rather than copying arbitrary files. Models stay outside package-manager ownership. CUDA packs are explicit separate optional installs; avoid postinstall hardware detection/download scripts.

Release workflow changes should be per module, not global STT features: preserve relay build/link checks; add baseline CPU/native backend target lanes, staging, recursive dependency checks, missing-backend/startup smoke tests and licensed manifests. GNU and musl Linux assets must be unambiguous; npm platform selection alone cannot infer an accelerated Linux libc minimum. Keep musl CPU fallback available separately and give a clear unsupported-libc error for GNU accelerated packages. GPU CI compilation/driver-free startup is distinct from real-device inference. Consider Windows signing and macOS signing/notarization when native bundles ship; no automatic publishing/updater is authorized by this report.

The current `release.yml` is **preview-only, manual publication**, not a tag publisher (`.github/workflows/release.yml:3–9`). Retain that governance initially. Changing workspace version placement requires changing `scripts/package_release.py:43` and its tests; it currently reads root `[package].version`. Keep one coordinated SemVer release version initially plus protocol/capability versioning; independent module versions later increase compatibility/test burden. Never ship the fake CLI/engine in release archives (`Cargo.toml:13`).

Install/update experience: Windows users extract a module archive into a fresh version directory and invoke its CLI; suite users later run an explicit setup CLI for config registration. Stop only the affected owned processes before replacing locked DLLs, switch the complete directory, retain the previous version for rollback, preserve models/config. Linux uses equivalent versioned trees and install-relative library paths; macOS native artifacts need consistent signing/framework/shader layout. Updates remain manual initially. Automatic downloads/startup services require separately approved scope, and no owner Windows installations occur during development (`AGENTS.md:60`).

**Required compatibility matrix:**

| Today's behavior / deployment | Migration requirement |
| --- | --- |
| Bare `pumice`, `pumice serve`, `--config`, `--plain`, `check-config`, `doctor`, `--version` | Same behavior, standalone artifact, default service/TUI and exit semantics; orchestration requires `pumice orchestrate …` |
| `pumice.yaml` / Windows `%APPDATA%\pumice` / XDG config base | Same lookup, providers, timeouts, max_parallel, debug settings, relative paths and error locations; no rewrite |
| `127.0.0.1:7567/v1`, `/health`, `/`, `/v1/models` | Remain relay; model IDs, `passthrough`, `inspect`, error/fallback behavior and buffered SSE preserved |
| Handy Custom URL/model/prompt | Works unchanged; requests continue to bypass formatter/orchestration |
| Existing server-1 user service/tunnel/friends | No service/unit/port/tunnel change in architecture/extraction PRs; STT uses another endpoint/process if later installed |
| npm command/update and portable archive names | Existing package/platform assets remain available with lightweight relay behavior |

These behaviors are evidenced by `src/main.rs:25`, `src/config/mod.rs:24`, `src/api/mod.rs:179`, `src/pipeline.rs:160`, `docs/install.md:58`, `:129`, `:147`, `:198`. The live service/tunnel deployment is **owner-provided context; runtime configuration UNVERIFIED** because no production probing was done. Never bind/probe 7567, restart production or overwrite the live executable as part of analysis/testing. New flows can use the existing relay as an external component, whose process the orchestrator does not own. Moving that deployment behind an orchestrator is optional later owner work, not needed for modularity.

## 8. Conflicts, proposed wording and S10.1 disposition

The owner-edit-only rules/spec remain untouched. Latest arch D1–D4 override the old “whole product is relay” interpretation; legacy relay decisions remain valid within relay. Recommended owner edits/clarifications below are **not implementation authorization for open choices**.

| Exact source lines | Conflict or obsolete scope | Proposed wording / disposition |
| --- | --- | --- |
| `AGENTS.md:7–9`, `:29`, `:81–86` | Describes all Pumice as relay; forbids formatter/UI unless asked | “Pumice consists of independent CLIs. Relay stays pass-through; formatter adds explicitly selected rules; interfaces sit above CLI/orchestrator contracts.” Preserve exclusions for command execution and unrequested client changes; D1/D3/D4 authorize analysis of the new scope. |
| `AGENTS.md:33–34` | Whole-product single executable/no runtime sidecars | “Relay and its compatibility entry point remain self-contained; transcription may distribute version-matched native libraries/backends. No developer runtime required.” State GPU driver/OS prerequisites and CRT policy separately. |
| `AGENTS.md:36` | One config for entire expanded product | “Each standalone module owns strict located YAML; orchestrator references module configs. Existing relay YAML stays compatible.” Pending owner approval of this recommendation. |
| `AGENTS.md:37` | Outbound rule would prohibit downloads/remote composition and literally internal HTTP | “No telemetry/external network from application, except official AI CLIs; configured loopback component HTTP is allowed.” Optional model-download/remote exceptions stay open. |
| `AGENTS.md:43`, `:52–54` | “Every CLI”/“adds no instructions” can be read as banning module file access/prompt injection | Qualify tool restrictions/temp workspaces as **AI provider CLI calls**; transcription reads configured models/audio, formatter reads definitions/adds selected rules. No module executes dictated commands. Keep text-log privacy global (`:55`). |
| `docs/spec.md:9`, `:17–22`, `:25`, `:47–70`, `:74–93` | Monolithic service/Handy-first architecture, single executable, formatter presets/GUI excluded | Owner source doc should describe independent modules and eventual Handy replacement; retain current Handy relay integration as supported migration path. |
| `docs/spec.md:21`, `:36`, `:56–57`, `:161–168`, `:173`, `:185–186` | Old optional relay prompts, defaults, cleanup/fallback conflict with existing owner decisions as well as D3 | “Only formatter owns presets; relay keeps selected-provider/no-fallback behavior and current minimal cleanup.” Do not resurrect spec E3 while extracting code. |
| `docs/spec.md:27`, `:214–217`, `:237` | Global same-language/phase plan/adapter formatting guarantee | Language/result instructions belong to selected client/formatter; replace old phase diagram with module phases; do not claim prompt-injection immunity. |
| `docs/spec.md:246` | Default port still open in old spec | Document existing relay 7567 as settled implementation, not a fresh choice; new module ports remain separate. |
| `README.md:5–9`, `:16`, `:43–53`; `docs/install.md:3–8`, `:33`, `:46–47`, `:205–207` | Accurate relay guide, inaccurate umbrella description once modules ship | Label existing downloads/commands as relay-compatible Pumice; add module capability/install matrix and roadmap. Keep today's instructions working; adjust outbound language only for approved exceptions. |
| `.task-decisions/README.md:9`, `:19`, `:21` | Global pass-through description; “orchestrator” means supervising agent | Scope pass-through to relay; distinguish **product orchestrator** from **supervising agent**. Future edit via task-decisions skill only. |
| `.task-decisions/tasks/s3.8-pure-passthrough/decisions.md:10`, `:23`; `.task-decisions/tasks/s3.6-minimal-cleanup/decisions.md:10`; `.task-decisions/tasks/s6.6-remove-default/decisions.md:10`; `.task-decisions/tasks/s4.4-failure-http-error/decisions.md:10` | Would conflict if expanded indiscriminately to formatter/product | Preserve as relay rules; add scoped clarification if owner approves. No reintroduction of relay prompts, provider fallback, broad cleanup or successful raw-text substitution on AI failure. |
| `.task-decisions/tasks/s3.2-optional-prompts/README.md:11` | Historical Done task mentions removed prompt composition | Historical record, not current authorization; retain it, rely on later pass-through decisions/code. |
| `.task-decisions/tasks/plugin-contract/backlog.md:12–16`, `:76–87` | Envelope and deadline choices still open | Keep envelope behavior during extraction; propose layered flow/module deadlines for owner review, not silently close backlog items. |

S10.1 decisions/backlog remain historical records until the supervisor updates them through the skill. In the table below, decision/backlog/report paths are relative to `S:.task-decisions/tasks/s10.1-transcription/`; spike/workflow paths are repository-relative at S:. Map their content explicitly:

| S10.1 source at S: | Remains valid | Superseded / needs owner action |
| --- | --- | --- |
| `decisions.md:10` D1 | Independent transcription; transcription before formatter | Add product orchestrator from arch D1; formatter still not prerequisite |
| `decisions.md:23` D2 | OpenAI multipart transcription endpoint | Host it in transcription module, not relay |
| `decisions.md:36` D3 | CPU-only spike proves first viable engine; useful first increment | CPU-only/single-file as permanent product restriction conflicts with arch D2; owner should record revised scope/order |
| `decisions.md:49` D4 | OpenWhispr investigation remains paused; avoid unrelated client work | No change implied by modular architecture |
| `decisions.md:64–70` D5 provisional | AVX2 evidence and a guarded optional static CPU archive | Global AVX2-only releases/exclusion of runtime modules should be revisited under arch D2; runtime CPU dispatch recommended |
| `backlog.md:5–21` #1 | GGUF engine/API/CPU build proof, owner engine acceptance still required | Choose module engine; no reason to require single-file accelerated STT |
| `backlog.md:23–38` #2 | Measured CPU latency, open Handy quality comparison | Add warm/cold Windows/GPU benchmarks; runtime variants replace “one baseline” question |
| `backlog.md:40–50` #3 | User model/local Handy path; download exception still open | Move model config/management entirely to transcription |
| `backlog.md:52–63` #4 | Multipart/JSON/text, explicit config, bounded decode/service | New transcription crate/process/router/config; no relay YAML/state/routes changes |
| `backlog.md:65–75` #5 | Injectable fake engine, no mandatory model/AI login tests | Module/adapter tests plus later orchestrator fake-component tests |
| `backlog.md:79–91` #6; `:93–104` #7 | CPU variants/source builds and GPU work are useful next improvements | Single-file conflict removed; re-evaluate deferred order with owner; CPU bootstrap/dispatch can be part of the first GPU bundle |
| `spikes/stt/`, `stt-spike.yml:25–33`, `:61–82`, `:112–133` | Isolated reproducible CPU experiments, source fixture, CRT/musl checks, optional real-engine smoke | Keep excluded research workspace; don't copy model download into mandatory CI; not a GPU/DLL package proof |
| `reports/2026-10-12-endpoint-design.md:11–94`, `:125–162` | Protocol/codec research, worker ownership, fake seam, cancellation/privacy rules | Rehome the service and adapters into transcription; clear partial/silence policy still needs review |
| Same report `:102–123`, `:139–164`, `:166–209`, `:215–223` | Some reusable config/test/build techniques | Shared relay YAML/model discovery/AppState/monitor and every `pumice --features stt` release recommendation are superseded; no native STT in relay |

The prior endpoint report is a proposal, not an owner-approved implementation. Even its opening “separate service” recommendation (`:7`) later wires STT into the same relay binary; that embedding is the superseded part. Keep CPU static helpers for the optional portable transcription build; promoted scripts must target the new crate, because `S:spikes/stt/ci/build-musl.sh:3` changes into the spike. The report's one-large-PR plan should become the small module PRs below.

Evidence freshness: `S:docs/research/S10.1-transcription.md:104` and `S:spikes/stt/README.md:71–83` still say CI unverified, but the later research `:125–148` reports all four targets passing. Use the later report, mark it **reported CI evidence, not rerun here**, and retain the Windows Unicode/Handy comparison uncertainty. `S:docs/research/S10.1-transcription.md:160` says no open owner questions, whereas model provenance/engine acceptance/quality remain open in backlog; backlog is the operative pending-work record.

## 9. Phased PRs, risks and Questions for the owner

Smallest valuable increment is **standalone transcription of an explicit local model/audio file**. It works before formatter/orchestrator, leaves Handy/production relay intact, and reuses the successful CPU spike. A brief CPU first increment is delivery sequencing, not a renewed permanent CPU-only restriction. GPU/CPU-dispatch packaging research can proceed without waiting for formatter.

1. Owner approves boundary/config/compatibility choices; supervisor updates decision scope/backlogs through the skill. This report alone does not mark arch analysis/choices Done.
2. **Workspace shell + model-free transcription seam:** keep root package intact, add owning crate/config/service and fake engine; verify old root build/tests and strict located YAML. No relay refactor yet.
3. **Standalone CPU transcribe CLI:** integrate pinned engine/local GGUF and bounded WAV normalization, clean stdout/errors/privacy, CPU guard for any AVX2 archive. Publish testable independent value; reuse proven native build checks.
4. **Audio decoding increment**, then **OpenAI transcription HTTP increment:** add selected codecs/fixtures separately; expose JSON/text, admission/cancellation/health/models. Each PR can pass fake tests before real model validation. Ship on a separate explicit port.
5. **Runtime packaging research**, then focused **Windows CPU-variant/Vulkan**, **macOS Metal**, **Linux GNU/Vulkan** PRs. Include clean-machine/library manifests and owner hardware benchmarks; CUDA later unless real measurements justify earlier investment. Native packaging must be validated before accelerated releases are advertised.
6. **Mechanical relay extraction:** move code/tests into relay crate, keep self-contained `pumice` façade and add canonical `pumice-relay`; separate workspace-aware packaging changes as needed to keep every intermediate PR buildable. No envelope/cleanup/provider refactor bundled into it. Follow with a separate **relay one-shot request** PR using the same validated service, stdout/error contract and fake CLI suite.
7. **Formatter CLI/library:** minimal schema/preset/system injection/unknown-field preservation and instruction conflict policy. Works through shell JSON piping into relay request mode; its HTTP adapter can be a subsequent PR.
8. **Orchestrator one-shot:** explicit external endpoints, named flow, aggregate deadline, raw/result separation and fake-component errors. Then a separate **managed serve/lifecycle** PR for owned children, private ports, readiness/restarts; then metadata events/monitor.
9. **Opt-in suite packaging/setup**, then future **capture/hotkey/paste/UI** stories. UI framework choice is not a blocker for backend contracts. Keep existing service/tunnel unchanged; plan any production migration separately with owner verification/rollback.

Principal risks: native ABI/library omissions; driver crashes; slow non-AVX2 hardware; thread oversubscription/RAM exhaustion; codec bombs and cancellation not stopping work; instruction conflicts/Kimi expansion; unknown OpenAI options mistaken for support; double-formatted requests; repeated paid calls after timeouts; backend version mismatch; production breakage from command/config/port changes. Mitigate with manifests/baseline hosts, independent processes, fixed admission, explicit supported capabilities, fake failure tests and the compatibility matrix. Avoid promising performance or accuracy from successful compilation alone.

### Questions for the owner

These are review choices, not reasons to stop the authorized analysis. Recommendations are explicit so the owner can approve/change them; no task decisions were edited.

| Question | Recommendation |
| --- | --- |
| Approve the workspace/selective-process boundary? | Library + CLI per module; process-isolate relay/STT; pure formatter library inside orchestrator; optional formatter HTTP |
| Should bare `pumice` become orchestration immediately? | Keep bare/serve/doctor/check-config and existing package as relay compatibility; opt in with `orchestrate`; consider any default change only in an announced major migration |
| Single YAML or independent configs? | Preserve `pumice.yaml` unchanged; three new domain configs plus explicit references, located errors, no automatic rewrites |
| Revisit S10.1 D3/D5 and #6/#7 order? | CPU one-shot first delivery; runtime CPU selection + Windows Vulkan standard bundle next; retain optional AVX2 static archive, retire global AVX2-only restriction |
| Approve transcribe-cpp engine/version? | Pin proved 0.3.1 initially; explicitly evaluate 0.4.0 upgrade after baseline/packaging parity, not infer parity from Handy main |
| What hardware must acceleration serve? | Owner supplies Windows CPU/GPU/driver details; prioritize Vulkan on that machine and compare CPU/Metal/CUDA only on relevant hardware; keep reliable CPU path |
| Must every OpenAI audio codec be in first release? | WAV/MP3/FLAC/Ogg-Vorbis first; capture intended client's uploads/options, add AAC-LC or Opus if needed; publish a supported subset |
| Accept STT admission/latency/memory/silence policy? | One warmed model, one admitted request, 429 busy, proposed limits in §2; retain bytes on failure; for verified silence allow successful empty transcript and skip formatter/relay. Corrupt audio still fails; no arbitrary empty result accepted as proof of silence |
| How should formatter handle existing instructions/placeholders? | System-only definitions first; reject existing instruction conflict by default, explicit append opt-in; templates later with one literal placeholder |
| What are the first builtins/formatting scope? | Approve one conservative `builtin:plain`; user YAML presets for additional styles; no mandatory language translation or reply heuristics |
| Approve custom combined API/partial-result handling? | `/pumice/v1/dictations` with named flow, separate transcript/result; non-2xx on failed formatting/relay retains completed transcript for explicit UI recovery; standard OpenAI routes stay standard |
| Can application HTTP connect to other modules / remote relays? | Explicit loopback-only component HTTP exception; keep external remote URLs disallowed until an explicit network/credential policy is approved; existing Cloudflare inbound deployment does not authorize new outbound clients |
| Allow model downloads / dynamic Windows CRT? | Local models first, no downloader; only later explicit catalog downloads under a scoped network exception. Retain static Windows CRT where verified; require separate approval for any app-local runtime redistribution exception |
| Who owns deadlines (plugin-contract #6)? | Orchestrator enforces whole flow; independently usable modules keep local deadlines/admission; propagate remaining budgets/cancellation, no automatic retries |
| What constitutes eventual Handy replacement? | Separate capture/hotkey/paste and UI stories after headless flows; choose platform UI technology with owner, preserve old Handy relay integration throughout |

## 10. Primary external evidence

- [T3 server CLI registration](https://github.com/pingdotgg/t3code/blob/5fe7568e871c0310c83fac0484fb84bba14d9148/apps/server/package.json#L10), [headless/server commands](https://github.com/pingdotgg/t3code/blob/5fe7568e871c0310c83fac0484fb84bba14d9148/apps/server/src/cli/server.ts#L15), [CLI composition](https://github.com/pingdotgg/t3code/blob/5fe7568e871c0310c83fac0484fb84bba14d9148/apps/server/src/binCli.ts#L57).
- [T3 desktop backend composition](https://github.com/pingdotgg/t3code/blob/5fe7568e871c0310c83fac0484fb84bba14d9148/apps/desktop/src/main.ts#L190), [instance ownership/logging](https://github.com/pingdotgg/t3code/blob/5fe7568e871c0310c83fac0484fb84bba14d9148/apps/desktop/src/backend/DesktopBackendManager.ts#L667), [restart control](https://github.com/pingdotgg/t3code/blob/5fe7568e871c0310c83fac0484fb84bba14d9148/apps/desktop/src/backend/DesktopBackendManager.ts#L1030), [HTTP schemas](https://github.com/pingdotgg/t3code/blob/5fe7568e871c0310c83fac0484fb84bba14d9148/packages/contracts/src/environmentHttp.ts#L4), [RPC schemas](https://github.com/pingdotgg/t3code/blob/5fe7568e871c0310c83fac0484fb84bba14d9148/packages/contracts/src/rpc.ts#L24).
- Published sources inspected locally: [transcribe-cpp 0.3.1 archive](https://static.crates.io/crates/transcribe-cpp/transcribe-cpp-0.3.1.crate), [transcribe-cpp-sys 0.3.1 archive](https://static.crates.io/crates/transcribe-cpp-sys/transcribe-cpp-sys-0.3.1.crate). R: paths/line numbers above refer to those unpacked archives; current upstream main must not stand in for the pinned release.
- [Handy engine/backend dependencies](https://github.com/cjpais/Handy/blob/a42131d24c59315fede06775cd354219c4f4a87b/src-tauri/Cargo.toml#L74), [native staging](https://github.com/cjpais/Handy/blob/a42131d24c59315fede06775cd354219c4f4a87b/src-tauri/build.rs#L156), [model-directory resolution](https://github.com/cjpais/Handy/blob/a42131d24c59315fede06775cd354219c4f4a87b/src-tauri/src/managers/model.rs#L554). Snapshot observations only; not proof of the owner's installed Handy version.

All proposed interfaces, backend bundles, migration steps and performance targets remain **UNVERIFIED until implemented and tested**. This report preserves the CPU spike's demonstrated evidence without treating it as proof of the proposed modular/GPU product.
