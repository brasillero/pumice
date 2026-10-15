# Flow design for `pumice run` (Codex, 2026-10-15)

Written by Codex (gpt-6.1-sol) at the supervisor's request, before implementation. The supervisor's decisions on it are D1–D8 in `decisions.md`; where they differ (D5 budgets, D6 flag name), the decisions win.


2026-10-15. Design recommendation for the supervisor, not a recorded owner decision.
Read-only inspection: main `e9b3af6`; formatter worktree `p11-formatter` at `68c170c`.
Both repositories stay untouched; existing main `?? spikes/` is unrelated. No network, builds/tests, model inference, AI CLI calls, ports or services were used.
All APIs/schema changes below are **PROPOSED / UNVERIFIED** until implemented and tested. The reported ~0.35 s turbo model-load cost was not measured here.

## 1. Deliverable and invocation choices

Deliver `pumice run <AUDIO> --flow <ID> [--stack-config <FILE>] [--json]` after the formatter PR merges.
Keep root as the thin orchestrator package; no new workspace crate is necessary. Bare `pumice` continues showing help/exit 2 until stack serving exists. Preserve existing `relay`, `transcribe`, `format` dispatch.

- **Transcription: spawn the trusted sibling** `pumice-transcribe --config ABS file ABS_AUDIO --model ID --json`, capture its envelope. Unlike `pumice transcribe`, `run` must retain its parent process, not Unix-exec the child. Reuse canonical-install-relative lookup in `src/dispatch.rs`; never PATH/cwd lookup, shell commands, automatic installation or downloads. Preserve paths as `OsString`/`PathBuf`, including spaces/non-UTF-8 Unix paths; use absolute paths before launch.
- **Formatter: in-process** `pumice_formatter::apply(request, definition, ConflictPolicy::Reject)`. Resolve configured definitions or explicit `builtin:clean` through its library. No formatter subprocess/server, templates or new prompts.
- **Relay: in-process** `config::load` → `providers::build_from_config(config, Arc<ProcessRunner>)` → `Pipeline`; do not call its service CLI or start HTTP. The relay's existing runner still launches official provider CLIs in private temporary workspaces, with its existing tool restrictions, flags, parsing and cleanup. Exactly one selected route, no retries/fallback.

This follows D6/D10/D13 and needs no listening ports. Cold transcription adds model load once per invocation; that is acceptable for the first file command and avoids daemon readiness, endpoint compatibility, uploads, process ownership ambiguity and a second transport implementation. A warm HTTP STT service will matter for frequent UI dictations; add it as an explicit later transport, using the same result contract. The native engine remains entirely outside core's dependency graph.
In-process relay needs a small deadline/cleanup seam, but provides direct cancellation and avoids dependence on an existing server or contention with the live relay. Separate `run` processes have independent concurrency limits; document that they do not share a server-wide quota/queue.
This **revises the research recommendation** to call relay HTTP (implementation-order §5), not an owner decision. D13 permits embedded relay; the supervisor should explicitly approve this narrower one-shot interpretation. External HTTP can only cancel a request connection, with server-side termination dependent on that server's behavior; no such guarantee is needed here.

## 2. Minimal orchestrator.yml (step 5b subset)

```yaml
schema_version: 1
modules:
  transcription:
    config: ./transcribe.yml
  formatter:
    config: ./formatter.yml
  relay:
    location: in_process
    config: ./relay.yml
flows:
  - id: clean
    enabled: true
    transcription_model: turbo
    formatter: builtin:clean
    relay_route: claude
    budgets_ms:
      total: 180000
      transcription: 120000
      formatter: 1000
      relay: 60000
```

`location` is a required enum with only `in_process` supported in this PR; reject URLs/other modes, rather than reserving a half-working external path. It declares where the relay runs, not its upstream model. `relay_route` is the relay's public request `model` (e.g. `claude`); upstream model/binary/timeouts remain exclusively in `relay.yml`.
Flows are a list with unique nonempty, case-sensitive IDs; `enabled` defaults false. `run` requires the exact ID and rejects disabled/unknown flows. References do not start anything until an enabled flow is explicitly selected. No default flow/model/formatter selection.
`formatter` must be explicitly an ID or YAML `null`; missing is an error. `null` skips injection but still uses the explicitly chosen relay route. A raw-only flow uses `formatter: null` and `relay_route: passthrough`; reject `inspect`, and reject a formatter paired with `passthrough` (it would never execute the rules). Thus no formatter never silently changes relay routing. This interpretation is a supervisor choice (§9).
`modules.transcription` and `.relay` are required; `.formatter` is optional only when all enabled flows use `null`. If any formatter is selected, require its explicit config reference even for a built-in (an empty registry file is sufficient). Supplied references are validated; no implicit module-config lookup.
Budgets are required positive integer milliseconds, each ≤3,600,000; total must exceed the fixed 2,500 ms teardown/output reserve. Stage caps may sum beyond total: remaining total always wins. No provider settings, model paths, language/threads/backend options or rule text in this file; inherit those from the owning module config (D7).

Lookup: explicit `--stack-config` (missing/unreadable fails), otherwise the per-user `orchestrator.yml`; absence is an error. Linux/macOS: absolute nonempty `$XDG_CONFIG_HOME/pumice`, else `$HOME/.config/pumice`; Windows: nonempty absolute `%APPDATA%/pumice`. Missing/unusable base fails; no cwd, install-directory, environment override or legacy filename search. Explicit relative CLI path is relative to invocation cwd; module paths are relative to the orchestrator file; model/definition paths remain relative to their module file. No `~`/environment interpolation.
Use `serde_saphyr::Spanned`, `deny_unknown_fields` on every raw struct, one YAML document, duplicate-key rejection, unsupported-tag rejection and snippets disabled (as formatter config does). Keep spans through semantic/reference validation. Parse/semantic errors report original `file:line:column`; missing keys point at their enclosing map, unavailable files have file-only locations. Never quote configuration values in safe errors; identify setting names and locations.

Preflight before audio inference: validate the audio is a regular file within the transcription byte limit (the child still enforces codecs/duration), parse the complete orchestrator file, load referenced relay/formatter configs with their own loaders, check all enabled flows' route/enabled state and formatter IDs, then validate transcription config/model IDs via the sibling's existing `models list --config ABS --json`. This command loads no engine and also validates its config; keep only IDs/exists metadata from its captured result. Enforce a 5 s subprocess cap within remaining total. Missing selected model file is an input/config error, never a download.
Preserve module-reported location fields; unknown cross-file references point to their flow field. Do not deserialize transcription YAML in core or depend on the transcription crate, even with its engine feature disabled. Child protocol errors use fixed messages; do not forward arbitrary child error messages. This is validation for `run`, not a general aggregate status/config-validation command.

## 3. Request construction and literal transcript handling

Save successful transcription text separately before further work. Build exactly `{"model":ROUTE,"stream":false,"messages":[{"role":"user","content":RAW}]}`; apply the one selected formatter once. No implicit language instruction, quoting, transcript tags, client prompt, translation, rewriting or cleanup in the orchestrator.
The formatter only inserts its system message. Validate the resulting known shape (one string user message plus string instruction messages), then construct `ExtractedRequest { model, system_texts, before_text:"", text:RAW, after_text:"", raw_text:RAW }` directly in the orchestrator relay adapter. Public fields already exist. Do **not** call `extract_request`: it recognizes Handy `<transcript>` envelopes and could reject/truncate the interpretation of literal tags in dictation.
This is an explicit internal raw-text boundary, not a change to existing HTTP extraction or resolution of `plugin-contract#1` for other clients. Test complete/malformed/nested literal tags. Relay receives exactly the formatter's instruction and the raw user message; the raw result remains byte-for-byte independent of relay's existing final-output cleanup.
For whitespace-only successful STT, preserve it and finish successfully with identical final text and no provider call (existing pipeline Empty behavior); report downstream stages skipped. No heuristic silence detection. A provider's empty answer remains relay failure, not successful raw output.

## 4. CLI result and events (D10)

One UTF-8 JSON document plus newline on stdout under `--json`, including parse/config errors. Example success (timings illustrative):
```json
{"schema_version":1,"operation_id":"opaque-id","module":"orchestrator","ok":true,"flow":"clean","transcript":{"text":"raw words","language":"en","audio_seconds":2.1},"result":{"text":"Raw words."},"stages":{"prepare":{"status":"ok","elapsed_ms":8},"transcription":{"status":"ok","elapsed_ms":490,"model_load_ms":350,"inference_ms":120},"formatter":{"status":"ok","elapsed_ms":1},"relay":{"status":"ok","elapsed_ms":800}},"timings_ms":{"total":1300}}
```
Failure after STT uses the same top-level fields, `ok:false`, retained `transcript`, **no `result`**, and e.g. `"error":{"stage":"relay","code":"relay_timeout","message":"Relay deadline exceeded","retryable":false}`. Before completed STT, omit `transcript`; partial child output is not a transcript. `flow` is omitted if arguments were invalid. Never serialize `FormatOutcome.text` as a success when its kind is `Raw`.
Rust types in `src/orchestrator/result.rs`: `RunEnvelope`, `Transcript`, `FinalText`, `StageReport`, `FlowError`, `ErrorLocation`; serialize absent optional fields away. `stages` always contains prepare/transcription/formatter/relay with `ok|error|cancelled|skipped`; skipped has null elapsed, attempted stages measured through failure/cancellation. Total uses a monotonic wall clock through cleanup, not the sum of child-reported times. Optional STT detail is additional to parent-measured elapsed, which includes startup/load/decode/IPC. All numbers are finite/nonnegative.
Stable code families (each stage prefix applies to every suffix in its group): `invalid_arguments`, `invalid_config`, `flow_not_found`, `flow_disabled`; `transcription_not_installed|protocol_error|invalid_input|model_not_found|engine_failed|timeout`; `formatter_not_found|instruction_conflict|invalid_request|timeout`; `relay_route_invalid|not_installed|not_logged_in|quota_exceeded|rate_limited|timeout|invalid_output|failed`; `flow_timeout`, `cancelled`, `cleanup_failed`, `output_failed`, `internal`. Use `error.stage` (`prepare|transcription|formatter|relay|cleanup|output`), fixed messages, optional typed `cause_code` and `location:{file,line,column}`. Map all known module errors explicitly; unknown child codes map to protocol_error, never infer from text.
`retryable:false` for every run failure in v1: the command never authorizes an automatic replay of a possibly paid call. Clients may offer an explicit new attempt.
Exit **0** success; **2** invalid arguments/config/flow/audio/model/formatter/route; **1** runtime/protocol/provider/deadline/cleanup/output failure; **130** Ctrl-C. Explicit cancellation wins over its induced I/O failure. Child nonzero status is mapped, not forwarded blindly; status/envelope `ok` disagreement is a protocol error. Output failure cannot reliably emit another envelope on the broken stream; return 1 and safe stderr metadata if possible.
Without `--json`, write only successful final text to stdout (one display newline); on failure stdout stays empty, stderr explains code/stage and says `--json` returns a completed raw transcript. Do not silently print raw text as final output.
Stderr under `--json` is bounded metadata-only JSONL: `{schema_version,operation_id,seq,module,stage,event,elapsed_ms?,code?}` with started/stage_started/stage_finished/finished; no token/audio progress invented. No ANSI/TUI. Implement nonblocking bounded event delivery; event consumers never block execution. Terminal stdout is authoritative if stderr events are dropped. Generate opaque operation IDs without embedding paths/text.

## 5. Deadline ownership and cancellation

Recommend resolving `plugin-contract#6` **for this command only**: orchestrator owns a monotonic whole-flow deadline starting at run entry, including validation and cold startup. Reserve 2,500 ms for teardown/output; do not start work after `D_work = D_total - 2500 ms`. Stage deadline is `min(D_work, stage_start + configured_stage_cap)`. No reset at boundaries or transfer of a full remaining budget to a restarted call; there are no retries.
Keep module safety limits. Add `Pipeline::format_until_observed(request, started, caller_deadline, progress)`; existing methods delegate with no caller cap. Its effective response end is `min(started + relay.total_timeout, caller_deadline)` before the existing 250 ms response reserve; provider deadline is additionally capped by its configured timeout. Preserve existing 100 ms startup threshold and 150 ms hard-stop slack within that reserve. Do not emulate this by modifying relay config or falsifying `started`. Thus the flow can shorten module budgets and can never extend them.
The root also guards each asynchronous stage with `timeout_at`; it classifies the winning timer as `flow_timeout` if the total-work limit won, otherwise `<stage>_timeout`. Recheck expiration/cancellation before accepting stage success or launching a provider. Formatter is bounded synchronous work (≤1 MiB request, ≤16 KiB definition); check before/after and reject late success. An async timeout cannot preempt synchronous formatting; document that limitation rather than claiming hard real-time interruption.

Own an operation-wide cancellation signal and child scope. Handle Ctrl-C on all platforms, plus SIGTERM on Unix (map SIGTERM to 143, documented separately from Ctrl-C); register handlers before spawning. Cancellation stops progression, preserves completed STT, cancels the active stage and awaits owned cleanup. Do not reuse Windows dispatch's ignore-and-wait handler for `run`.
Reuse existing Unix process groups / Windows Job Objects from `crates/relay/src/process/tree.rs`, exposing the small lifecycle type through `process`; do not use the AI-specific invocation runner for transcription. Always pipe/drain STT stdout/stderr concurrently (stdin null); cap stdout at 1 MiB for metadata/file envelopes, drain/discard stderr, reject extra documents/trailing non-whitespace or malformed UTF-8/version/shape. No inherited child streams. Kill the owned tree and reap it on timeout, overflow, cancellation and normal exit (descendants may hold pipes). Apply the existing 250 ms pipe-exit grace and 2 s reap limit.
For relay cancellation, dropping the pipeline future already kills provider trees via `ProcessTree::Drop`, but explicit awaited cleanup needs a narrow addition: optional `ProcessRunner::with_cleanup_scope(Arc<CleanupScope>)`, passed through existing provider builders. A scoped tree guard kills synchronously on drop, transfers the unreaped tree to a registered cleanup task, and the orchestrator drains that scope for at most 2 s before publishing its terminal result/runtime shutdown. Normal runner paths retain `finish`; default server behavior stays unchanged. Reuse this scope for STT too. Preserve the Unix unreaped-leader/PGID safeguard; no PID-file or port-based killing. Add no provider trait/cancellation signature changes.
A teardown failure is `cleanup_failed` (or a secondary cleanup code when a primary failure already exists), never successful final output. Kill-on-drop remains a last defense. Uninterruptible OS tasks cannot have a strict termination guarantee; cross-platform termination/reaping, scope races and Windows console delivery are **UNVERIFIED** until fake lifecycle tests pass. Budget bounds execution/teardown attempts; writing to a blocked terminal/filesystem is not a hard real-time guarantee.

## 6. Privacy and bounded data

Only explicit result stdout may contain transcript/final text. No transcript, prompt, raw child stderr, diagnostics, serialized request, audio path or native error in metadata logs/events. Keep audio in its original file; no orchestrator audio copies/temp files. Capture module JSON in memory, extract allowlisted fields, and discard child messages; never log serde parse errors that quote values. Use redacted/manual Debug for content-bearing types. Config errors may carry their requested config file location.
The flow opens no relay HTTP log/debug sinks or monitor/TUI; `relay.yml` debug logging remains an option for relay serving, not inherited flow text logging. Add no flow text-debug option in this PR; strict schema rejects it. Future text-debug sinks must be explicit in that module's config, private, bounded and warned about. JSON results are intentionally sensitive artifacts for their caller to store securely.
Bound the constructed formatter request to 1 MiB before synchronous transform; after STT succeeds, oversize failures retain its raw transcript. Existing relay provider stdout cap remains 10 MiB; terminal JSON may be larger than the STT envelope cap but is bounded by these inputs/outputs. Do not add a transcript-truncation heuristic.

## 7. Files and fake-only verification

Implementation footprint: `src/dispatch.rs` adds `run` and returns structured sibling lookup errors; new `src/orchestrator/{mod,cli,config,result,transcription,relay}.rs`; root Cargo dependencies add clap/serde/serde_json/serde-saphyr/tokio and formatter after merge. Small relay changes in `pipeline.rs`, `process/mod.rs`, `process/tree.rs` plus scoped-cleanup tests. No native STT dependency, HTTP client, new server or module refactor.
Extend `tests/support/transcribe_stub.rs` with an explicit scenario mode for models-list/file envelopes, delays, output overflow, child errors/crash and descendant PID reporting; retain existing dispatcher tests. Scenario files live beside copied stubs, not production env flags. Install copies beside a copied test `pumice`; supply hermetic XDG/APPDATA/HOME and explicit configs.
Use the existing `pumice-test-cli` (`crates/relay/tests/support/fake_cli.rs`) as a configured fake provider binary, using a real adapter (e.g. Claude) and real Pipeline/ProcessRunner. Add a root test target referencing that source, or build/reuse its Cargo binary explicitly in the test command; do not assume a dependency crate's integration-test executable exists. No public production test hooks. Real formatter library/definitions throughout.

Required tests:
1. Happy path: raw Unicode/newlines/whitespace distinct from cleaned final; formatter's exact system text and raw user bytes reach fake CLI; model route resolves to configured upstream model; one call only, private empty cwd/tools disabled; no transcript in argv.
2. Explicit null formatter; explicit raw passthrough; no implicit config defaults; forbidden inspect/formatter+passthrough; whitespace-only STT skips paid call; literal complete/malformed/nested `<transcript>` tags remain data.
3. Strict YAML unknown/duplicate keys, duplicate IDs, missing/disabled/unknown references, missing configs/definitions, unsupported version, relative paths from another cwd; exact source locations including module-file errors; no child inference/provider calls after invalid preflight.
4. Missing/wrong sibling, PATH spoof ignored, symlink install lookup, spaces/Unicode and Unix non-UTF-8 paths; models-list config error, unknown model, missing file; fake transcription verifies argv/model/config exactly.
5. STT crash/nonzero, malformed/version-mismatched/oversized/multiple JSON, extra stderr, mismatched exit/ok, engine error; no transcript exposed from unsuccessful/partial output. Forward-compatible unknown metadata fields may be ignored within schema 1; required fields stay typed.
6. Formatter failure after STT, relay auth/quota/rate-limit/empty output/cleanup rejection; raw transcript retained, result absent, safe stable code/exit; failure never switches providers/retries or prints raw as final.
7. Overall/stage/preflight deadlines (including cold start/queue/setup), provider cap shorter than flow, no budget reset, late success rejected; synchronous formatter late-result check using an injected clock, not real inference or timing-sensitive sleeps.
8. Ctrl-C during STT and provider run, cancellation between stages, SIGTERM on Unix; fake parent/grandchild and held pipes killed/reaped; no later stage starts, raw retained if complete; normal success also cleans descendants/temp workspaces. Exercise Windows Job Objects/console handler in CI.
9. One JSON stdout on all argument/config/runtime failures; ordered metadata-only JSONL; hostile child output/serde errors and redacted Debug contain no sentinel transcript; event backpressure, broken stdout/stderr, cleanup failure and partial timings.

Run Linux `cargo test --workspace --locked` with engines disabled, focused fake integration tests, fmt/clippy per CI; retain Windows/macOS fake CI and static core linkage checks. No real AI/model calls, downloads or existing service connections; any future HTTP tests bind only `127.0.0.1:0`. This analysis ran none of those checks.

## 8. Documentation and exclusions

Add `orchestrator.example.yml`, `docs/orchestrator.md` (setup, flow semantics, budgets, cold load, explicit raw recovery, JSON/event/exit schema, cancellation, sensitive output), README run example; update CLI/API contract and relevant install/source build instructions for the sibling requirement. Keep spec/product/AGENTS untouched. Maintain/finish new PR task notes through the task-decisions skill; record only supervisor-authorized choices with attribution, and link this proposal's divergence from prior research.
No `/pumice/v1/dictations`, frontend HTTP/proxies, managed long-running children, restarts, port allocation, aggregate health/capabilities, UI/mic/hotkeys/paste, streaming, new formatters, provider changes, downloads, packaging rollout or live deployment. No changes to Handy/friends' URL or systemd; no contact with 7567/17568/17569.

## 9. Supervisor decisions before implementation (recommendations)

- **Transport:** approve sibling STT + embedded formatter/relay for this PR; defer both HTTP transports. Accept measured-later cold-load overhead and process-local concurrency. Future UI may call the same `run --json` immediately; warm service transport is an additive later optimization.
- **Deadline ownership (#6):** approve absolute flow cap plus stricter module caps and 2.5 s reserve; retain standalone relay behavior. Record the command-scoped resolution, leaving general future HTTP/supervision policy open.
- **Literal tags (#1):** approve the explicit internal raw-text boundary; leave Handy HTTP envelope extraction unchanged. Reject neither arbitrary spoken tags nor overwrite the stored raw transcript.
- **No formatter / raw flow:** approve null as no injection while relay routing stays explicit; raw-only requires passthrough. This clarifies architecture-modules' ambiguous “absence of a formatter means raw transcription” recommendation, not an active owner decision. If raw-only-on-null is preferred, change the schema to omit relay for those flows before implementation; never silently ignore a configured paid route.
- **Silence, defaults and CLI surface:** preserve existing whitespace-empty success/no-provider behavior; require explicit budgets/flow/refs and `--stack-config`; retain bare-help until serving. Defaults shown here are examples, not latent enabling settings.
- **Scope/size:** include only config references, scoped child cleanup, caller-deadline seam and the file command in M. If cleanup exposes a cross-platform blocker, split that prerequisite explicitly; do not replace it with unverified “drop means reaped” claims or relax lifecycle tests.
