# Pumice implementation order — research recommendation

2026-10-15. Read-only review of `main` / local `origin/main` at `ea1dbfb` (PR #96). No fetch, build, inference, AI CLI call, production connection, service operation or repo edit. Existing untracked `spikes/` was left alone. All commands below are proposed examples, not executed.

## Recommended order and authority

**2 HTTP → 4a runtime CPU acceleration → 3 downloader → 5 orchestrator/relay rename → 6 formatter and combined flow.** Move **4b the first relevant GPU backend** immediately after 4a if measured CPU latency is still unacceptable; otherwise do it after 6, followed by other GPU targets. Do not make every GPU platform a gate for usable modules.

Start item 2 now: no separate prerequisite PR is justified. Extract only the shared transcription service/control needed by HTTP inside its first PR. The existing CLI, bounded codecs, config and fake-engine seam already provide the foundation.

Item 1 (release archives and the general Windows/macOS engine-build rollout) is **SKIPPED FOR NOW**, per this request; it is not a dependency or scheduled phase. Initial delivery remains from source on Linux. Target GPU development may eventually need its own target build, but must not silently revive the skipped distribution task.

D1–D4, D6–D10 and D12–D14 govern module boundaries, config, native isolation and UI layering. D11's recorded order put the orchestrator after CPU transcription; this requested proposal advances HTTP and CPU speed first. The owner should authorize that revised order before implementation; this report records no new decision. D12 supersedes the architecture plan's compatibility window/migrator; D8 supersedes S10.1's AVX2-only policy. The old endpoint report's relay embedding is superseded by D1/D2.

Sizes: **S** one narrow change; **M** several connected behaviors; **L** native/platform or lifecycle work split across PRs. These are relative estimates, not calendar promises. Proposed behavior and commands remain **UNVERIFIED** until implemented/tested.

## 1. Item 2 — standalone transcription HTTP (M; first priority)

**Goal/dependencies:** warm, raw speech-to-text usable by HTTP clients; depends only on the current transcription crate. No relay dependency, orchestrator, downloader, backend rewrite or new codecs.

**Small PRs:** **2a (M)** one usable vertical slice: `serve`, strict server settings, warm model, multipart routes, bounded worker/cancellation, safe responses and fake-engine tests. **2b (S)** fixture-backed integration/docs for the owner's selected client; add a codec/option only if its actual uploads require it. This is not permission to modify the client.

**Minimal first PR (2a):**

- Command: `pumice-transcribe serve --config FILE --model turbo --port PORT`; `--model` selects a configured ID, otherwise require `default_model`. Serve explicitly enables the service; never start it from `file`, config validation or model listing.
- Bind **IPv4 `127.0.0.1` only**, with no configurable non-loopback host. Require explicit `--port` or `server.port` initially; no guessed adjacent production default. Support port 0 for tests/future private launches and report the actual address. Collision fails without disturbing the existing listener. Proposed `server` keys/defaults belong only in `transcribe.yml`, validated with original file/line/column.
- Load **one selected configured model once before accepting HTTP requests**; fresh session per inference; startup load failure terminates transcription only. No lazy loading, request-triggered downloads, multiple resident models, reload or automatic model switching. `/v1/models` lists just that served ID in the OpenAI list shape, without paths; CLI `models list` continues listing the whole local registry.
- `POST /v1/audio/transcriptions`: multipart only; exactly one nonempty uploaded `file` and required exact served `model` ID. Requests never contain a filesystem model path/URL; upload filenames never become paths.
- Accept optional `language` using existing validation (request overrides configured hint, otherwise detection when neither exists), `response_format=json|text` (default JSON), `prompt` (client-supplied context, unchanged), finite `temperature` in [0,1], and absent/`false` `stream`. Extend options/adapter for prompt and temperature; never silently discard them.
- Return HTTP 200 `application/json` with `{"text":"…"}`, or `text/plain; charset=utf-8` with exact transcript bytes. Do not use the CLI's `{schema_version,ok,result}` envelope or append its display newline. No formatter, translation, cleanup, fabricated usage/confidence or timestamps.
- Reject unsupported output formats, timestamps, streaming `true`, diarization/chunking/include/unknown fields, duplicates and malformed scalars with safe 400. Reject unsupported media/container/codec with 415; preserve WAV/MP3/FLAC/Ogg-Vorbis and existing Opus rejection. Advertise a supported OpenAI subset, not every upstream capability. [Official OpenAI transcription reference](https://developers.openai.com/api/reference/resources/audio/subresources/transcriptions/methods/create), checked 2026-10-15.
- Reuse existing **25,000,000 file bytes / 120 decoded seconds / mono-stereo / 8–192 kHz** limits. Add **26,000,000 total body bytes**, ≤32 parts, ≤16 KiB per scalar, **15 s total upload deadline** and **120 s arrival-to-result deadline** as proposed defaults. Enforce chunked bodies too, before unbounded buffering; override Axum's multipart extractor cap explicitly. Duration limit is not a guarantee every allowed clip finishes within the deadline at baseline speed.
- **One admitted upload/decode/inference; immediate 429 while busy, no queue.** Acquire capacity before consuming the body. A worker owns the permit through actual completion, even after client disconnect/504. Do not copy the relay's FIFO provider queue: it does not bound audio uploads or waiting memory. Metadata/health routes bypass inference admission.
- Run decoding/resampling and native inference on an owned blocking worker. Add app cancellation/deadline control, decoder packet checks, and `Session::set_cancel_token`; cancel on request-drop/deadline/shutdown. Native cancellation is cooperative: a timed-out request must not release capacity early or publish late/partial output. Abort latency is **UNVERIFIED**; a native hang/crash remains confined to this process.
- Errors always use safe JSON `{error:{message,type,param,code}}`: 400 invalid request/corrupt input/excess duration/unknown model; 413 bytes; 415 unsupported; 408 upload deadline; 429 busy; 503 draining/unavailable; 504 processing deadline; 500 engine/worker/empty-output failure (empty policy subject to approval below). No paths, filenames, prompt, transcript or raw decoder/native errors in HTTP errors/logs.
- `GET /health` is lightweight liveness with version and stays responsive during inference; no model run/login check. Because listening starts after successful load, startup readiness is unambiguous; add separate `/ready`/instance metadata only when orchestration needs them. Handle Ctrl-C and SIGTERM: stop admission, cancel and drain the owned worker. No TUI/event framework prerequisite.

**Reuse and tests:** move the CLI's decode → resample → normalize → engine sequence into a small library service; share model selection/thread/language resolution with `file`. Reuse `config::load`, anchored model paths and audio fixtures. `decode_audio` currently reads its entire reader and trusts its caller's byte budget: HTTP must supply a bounded buffer/reader; avoid unnecessary duplicate 25 MB buffers. Current `Engine` has no cancellation/control argument, so add that seam rather than wrapping only the HTTP future in a timeout. `Error` can carry local/native strings; map error codes to fixed HTTP messages, never serialize it wholesale. Keep native logging disabled before loading and use metadata-only logs; avoid derived Debug on text/prompt-bearing new types.

Fake tests must drive the production router/service: field order and chunked multipart; exact JSON/text Unicode/whitespace; selected-ID discovery; options forwarded unchanged; corrupt/unsupported/oversize/duration boundaries; safe error bodies/privacy. A gate-controlled fake verifies second-request 429 before reading its body, responsive health, cancellation, retained capacity after timeout/drop, failure/panic cleanup and graceful shutdown. Use in-process requests or `127.0.0.1:0`; no public fake route or real model/AI dependency. Require Linux `cargo test --locked` and ordinary three-OS fake tests; do not expand native platform rollout here. This analysis ran no tests.

**Owner can try:** repeated local uploads retain the model in memory; scripts and compatible apps can transcribe independently of the live relay.

```sh
# Proposed future commands; port is illustrative, unprobed and unreserved.
./target/release/pumice-transcribe serve --config ./transcribe.yml --model turbo --port 17568
curl --fail-with-body --max-time 130 http://127.0.0.1:17568/v1/audio/transcriptions -F 'file=@clip.wav' -F model=turbo -F language=pt
curl --fail-with-body --max-time 130 http://127.0.0.1:17568/v1/audio/transcriptions -F 'file=@clip.wav' -F model=turbo -F response_format=text
curl --fail http://127.0.0.1:17568/v1/models
curl --fail http://127.0.0.1:17568/health
```

**Handy-like apps:** an app needs a configurable OpenAI transcription base URL (`http://127.0.0.1:17568/v1`), served model `turbo`, compatible codec/options and sufficient timeout. A placeholder key is needed only if its settings demand one; the proposed local endpoint needs no OpenAI key. Actual Handy/OpenWhispr custom STT support, upload fields/codecs and timeout/retry behavior are **UNVERIFIED**. `docs/install.md` proves only Handy's existing **chat post-processing** configuration; changing that URL to the transcription service would not work. Where supported, configure STT and chat separately, retaining the app's prompt ownership and existing relay post-processing.

**server-1/tunnel:** a client on server-1 can use transcription loopback immediately after an owner-run source deployment. A friend's `127.0.0.1` is their own computer. The current relay/tunnel does not automatically expose a new port or route, and the relay must not absorb audio handling. A later owner-authorized separate tunnel hostname/origin could point at the transcription loopback service; keep existing chat origin/URL untouched. Public exposure, access/auth policy, abuse/admission controls and tunnel/client timeout/upload compatibility require an owner decision and verification. Actual tunnel configuration is **UNVERIFIED**; do not change bind to `0.0.0.0` or assume D14 authorizes remote module composition. No exposure work belongs in 2a.

**Risks/owner choices before 2a:** approve the field subset, exact limits/deadlines, single served model and 429/no-queue policy (recommend as above). Turbo's reported ~1 GB resident RAM needs acceptance. Decide empty/silent output: recommend safe `empty_output` 500 initially, with no invented silence detector; valid silence returning `{"text":""}` is an alternative requiring an explicit contract. Select the first client for 2b; curl alone is sufficient to start 2a. Public exposure is not a prerequisite. Baseline latency is an explicit initial limitation.

## 2. Item 4a — runtime CPU variants, moved forward (M–L)

**Goal/dependencies:** make the endpoint useful for repeated dictation; depends on the current engine seam, with HTTP providing repeatable warm measurements. Downloader and orchestration are independent.

**Why now:** the owner's 19 s / 14 s is a real-time factor of ~1.36; warm loading alone cannot fix it. Earlier S10.1 research reports 20.51 s conservative, 7.08 s AVX2 and 5.87 s native on a 14.19 s sample, with ~0.35 s model load. This is promising CPU evidence, **UNVERIFIED for the current HTTP build, Windows, other samples or sustained load**, not a GPU requirement. Keep one admission slot; adding concurrent model copies is not a latency fix.

**Small PRs:** **4a.1 (S)** reproducible warm/cold and per-stage measurements, model/thread sweep and optional host-tuned source-build recipe in a separate scratch build directory (no installed binary replacement). **4a.2 (M)** feature-gated dynamic backend initialization and baseline/accelerated CPU selection, native manifest/staging for local builds, selected-variant diagnostics and fallback tests. Split trusted-library discovery from variant validation if needed. Never globally force AVX2 or remove the baseline path.

The pinned local `transcribe-cpp` 0.3.1 sources already expose `dynamic-backends` → shared libraries and x86 `GGML_CPU_ALL_VARIANTS`; implementation support exists, packaged behavior is **UNVERIFIED**. Stage matching transcribe/ggml/CPU modules together; load only trusted installation-relative directories. Audit `.cargo/config.toml`'s conservative overrides and separate native caches so ISA flags cannot contaminate the bootstrap or disable intended variants. This is transcription-only local artifact staging, not a downloadable archive project.

**Owner can try:** the same HTTP client and model at measured lower latency, with actual CPU variant reported. **Risks:** SIGILL, OS feature detection, loader search, ABI mismatch, libc/CRT and inaccurate extrapolation from one clip. GNU dynamic STT may need a separate target; retain portable static CPU build and static relay/core. **Before starting:** choose target machine and acceptable p95 end-of-dictation latency; recommend warm inference faster than real time plus a separately agreed short-clip wait target, measured alongside transcript quality. Existing source-only host tuning may be used as a temporary bridge, never as the portable default.

## 3. Item 3 — explicit model downloader (M)

**Goal/dependencies:** remove manual model-fetch friction and supply a UI-ready operation; depends on existing model config/listing and a verified catalog, not GPU or orchestrator. Owner already has local models, so this has less immediate value than CPU speed.

**Small PRs:** **3a (S)** offline catalog (full immutable revision, exact file/URL, SHA-256, bytes, license/provenance), separate catalog/configured/installed views and `models verify`. **3b (M)** `models download ID --json`: bounded HTTPS, permitted redirects, per-model lock, same-filesystem `.part`, whole-file hash/size check and atomic install; JSONL byte/state progress and safe terminal result. **3c (S, if needed)** validated range resume/cancel semantics; clean restart is acceptable initially if advertised. Use fake HTTP transport for ranges, interrupted transfers, hash mismatch, disk-full, concurrency and cancellation tests.

**Owner can try:** fetch/verify a catalog model explicitly, then explicitly register its path in `transcribe.yml`; existing local paths keep working. Downloads never enable models, alter defaults or occur on startup/request. No HTTP download-job API needed yet.

**Risks/choices:** catalog provenance and URLs/hashes in today's docs are **UNVERIFIED** until bytes and source are checked; partial files must never appear installed. Before 3a, approve initial catalog/storage policy; recommend tiny + turbo only, per-user data outside installs, overrideable model directory, no automatic config edits. D9 already authorizes pinned explicit downloads; do not reopen that authorization.

## 4. Item 5 — `pumice` entry point and `relay.yml` (M–L)

**Goal/dependencies:** one CLI to dispatch/supervise selected modules without native STT in the core. Depends on existing relay library and item 2's warm service; CPU variants/downloader are sequencing choices, not hard dependencies.

**Small PRs:** **5a (M)** root dispatch plus `relay.yml` rename as one coherent breaking transition: `pumice relay …` calls the shared relay argument handler (currently it reads process argv internally), `pumice transcribe …` launches a trusted explicit/install-relative executable, preserving OS args/stdio/status/signals. Bare `pumice` targets orchestration, with a clear missing-config error until supervision lands. Update docs/npm expectations/tests together; no old aliases, filename fallback or migrator (D12). **5b (M)** strict `orchestrator.yml` references, explicitly enabled managed/external loopback components, aggregate config validation. **5c (M)** owned-child readiness/instance/version checks, port-0 announcement, shutdown/cancellation and status with fake children. No auto-install, paid retries or frontend proxy required yet.

**Owner can try:** module subcommands after 5a; after 5c, one configured command starts a warm transcription service and owned relay, or attaches to an explicitly external relay. Never stop an external component. Core remains static; transcription remains a separate process (D13).

**Risks/choices:** CLI breakage and accidental ownership of live processes; stop only retained owned child handles, never port discovery or pattern killing. Before 5a/5b, approve exact root CLI/config schema, trusted executable lookup and external-vs-managed behavior (recommend architecture-plan hybrid design stripped of superseded compatibility code). Deployment is a separate owner-run action: manually update server-1's config filename/unit arguments while preserving chat port 7567 and friends' URL; retain old executable/config for rollback. Nothing in this research performs or authorizes that deployment.

## 5. Item 6 — formatter, then the combined flow (M–L)

**Goal/dependencies:** explicit selected rules turn raw dictation into a chat request; orchestration composes audio → raw text → formatter → relay. Formatter itself has no HTTP/engine/provider dependency; combined flow needs items 2 and 5. Downloader/GPU are optional.

**Small PRs:** **6a (M)** `pumice-format` library/thin CLI plus in-process `pumice format`: strict `formatter.yml`, registered declarative definitions, list/show/check/apply, selected system-message injection and preservation of other JSON members. **6b (M)** `pumice run AUDIO --flow ID --json` using module services, an explicitly selected model/preset/relay route, raw transcript separate from final output and safe per-stage errors/events. Call the existing relay HTTP endpoint; a relay one-shot CLI is useful later, not a prerequisite. **6c (M)** warm stack frontend and versioned `/pumice/v1/dictations` only when needed, retaining ordinary raw OpenAI transcription semantics.

**Owner can try:** preview transformed requests without quota after 6a; after 6b, a complete file-to-formatted-result workflow, still without microphone/hotkey/paste. Fake components verify exact request construction, instruction ordering, no fallback/retry, cancellation and partial-stage failure; manual real-provider checks are a separate owner action.

**Risks/choices before starting:** approve first formatter content and instruction-conflict policy; recommend one explicitly selected definition, system injection, reject existing nonempty system/developer instructions unless explicit merge policy chosen, no templating language. Before 6b approve combined result/error contract and deadline ownership (`plugin-contract#6` is still open): recommend overall flow budget capped into module budgets, while standalone module deadlines remain. Preserve completed raw transcript in the custom flow failure result, return non-2xx with no successful final text, never silently substitute it. Existing relay `<transcript>` parsing (`plugin-contract#1`) can affect literal tags; owner must choose scoped resolution/rejection before promising arbitrary transcript pass-through, not bundle an unapproved relay rewrite.

## 6. Item 4b — GPU, staged by demonstrated need (L)

**Goal/dependencies:** meet the agreed latency/CPU-use target where 4a is insufficient. Reuse its trusted native layout and backend diagnostics; no dependency on formatter/downloader. **Small PRs:** explicit backend/device config and fake selection/errors (S); first owner-selected hardware backend (M–L); other Vulkan Windows/Linux and Metal macOS adapters/validation in separate later PRs. Prefer Vulkan for the first relevant Windows/Linux machine, Metal on Apple Silicon. Do not wait for every platform or introduce CUDA now.

**Owner can try:** the same CLI/HTTP protocol with accelerated local inference and reported actual device/backend. **Risks:** drivers, VRAM, clean dependency closure, CRT/library compatibility, output drift, native hangs; speed/compatibility are **UNVERIFIED** until real-device measurements. Before a platform PR, identify actual GPU/driver/test host and accept driver prerequisites; do not assume server-1 has a useful GPU. Explicit unavailable backend fails; approved `auto` may choose CPU at startup, never replay a failed inference. No Windows-side install or general Windows/macOS engine-build/archive phase is scheduled by this recommendation.

## 7. Item 7 — Symphonia replacement and later interface

Replace Symphonia **after HTTP and the first CPU speed increment**, preferably after 6, when actual client codec needs are captured. It is not the current latency evidence's bottleneck and need not block HTTP. Advance it only for a demonstrated codec, maintenance or licensing/distribution requirement; the replacement choice itself is **UNVERIFIED**.

**Size/dependencies/PRs:** M–L depending on codecs; first compare candidates against current fixtures/limits without changing public contracts (S), then replace WAV support and compressed formats in focused tested PRs (M each). Keep strict RIFF validation, rubato/normalization contract, memory/duration limits, fake-engine tests and documented headerless-MP3 exception. **Owner can try:** identical CLI/HTTP inputs with a different decoder and measured parity. **Risks/choices:** codec regressions, new native libraries and maintenance; owner chooses maintained replacement/required formats, recommendation to retain current supported formats and avoid an ffmpeg executable/runtime prerequisite. UI/microphone/hotkey/paste follows a usable combined flow and a separate owner-approved task; implement them above these module contracts, never inside relay/transcription.

## Evidence

- Current code: `crates/transcription/src/{cli.rs,audio.rs,engine.rs,lib.rs,error.rs,config/}`; `crates/relay/src/{api/,cli.rs,pipeline.rs,request.rs,prompts.rs}`; root `src/main.rs`, `.cargo/config.toml`, `.github/workflows/transcription.yml`, `docs/transcription.md`, `docs/install.md`.
- Decisions/status: `AGENTS.md`, `.task-decisions/tasks/arch-modules/decisions.md` D1–D14; complete notes for `p4-transcribe-cpu`, `p5-transcribe-codecs`, `p4b-transcribe-config`; `plugin-contract` open items. Latest user instruction overrides `p4b-transcribe-config#2`'s old “next PR” wording.
- Planning: `docs/research/architecture-plan.md` and transcription/formatter/orchestrator sections of `architecture-modules.md`. Historical endpoint report absent on main: inspected with `git show origin/s10.1-transcription:.task-decisions/tasks/s10.1-transcription/reports/2026-10-12-endpoint-design.md` at `e644902`; benchmark source is `docs/research/S10.1-transcription.md` at that same ref.
- Dependency evidence: locally cached `transcribe-cpp{,-sys}` 0.3.1 source, especially `session.rs`, `family.rs`, `backend.rs` and `bindings/rust/sys/build.rs`. Source capabilities are verified by inspection; Pumice integration/platform execution remains **UNVERIFIED**.
