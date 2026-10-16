# Pumice test and cleanup audit

2026-10-16 · main `9f9018844c0510ea83b8cdedc89ee26716e04df5` · read-only review

## Assessment

The suite mostly protects the current contracts. I found **no verified active test that requires relay prompt injection, provider fallback, broad reply cleanup, OpenCode execution, automatic `pumice.yaml` lookup, or bare root `pumice` starting the relay**. Several names/comments look obsolete, but their assertions are current. Do not delete suites based on those names.

The best cleanup is: (1) bound process/pipe waits and replace ordering sleeps; (2) consolidate repetitive config/API cases and remove the helper-binary name collision; (3) remove a verified unused fixture and unnecessary warning suppressions, and refresh a few stale comments/docs. Archived adapter code is intentionally retained by an owner decision; deleting it would be a separate product decision.

Priority findings:

- **High:** the known Windows dictation test has an unbounded stderr-reader join after its bounded process wait. Similar unbounded operations occur in other process tests. The Windows root cause remains **UNVERIFIED**, but the missing bound is verified.
- **Medium:** FIFO/cancellation/lock tests sometimes use elapsed sleeps as proof a prerequisite occurred; some disconnect tests never assert their prerequisite was reached.
- **Medium:** two packages define the same `pumice-test-cli` output; Cargo warns this may become a hard error.
- **Low:** repetitive config tests, one unused `.ps1` fixture, stale archive/fallback/CPU-status language, and unnecessary `allow(dead_code)` attributes.

## Scope, authority and execution

Read AGENTS.md, `.task-decisions/README.md`, every task's `decisions.md`, the task-decisions skill and its rules, architecture-plan.md, spec.md, relevant backlogs, tests/support/fixtures, source test modules, workflows and scripts. Decision references below mean `.task-decisions/tasks/<task>/decisions.md`, with the stated D number. Existing historical research/proposals are evidence, not current implementation requirements.

Executed once: `cargo test --workspace --locked`, exit 0, **1,116 passed**, no libtest ignores/failures, **44.39 s wall** including an **18.55 s** build. Custom managed-child runner contributes 11 of these and does not print a duration. Individual targets run tests concurrently, so durations below are target elapsed times, not per-test costs. This is one Linux sample, not a benchmark or flake-rate measurement. Effective UID was 1000.

Executed: `cargo +stable clippy --workspace --locked --all-targets -- -W dead_code -W unused`, exit 0, no Rust warnings; dev check finished in 24.71 s including build-directory lock wait. This does **not** prove all public APIs or feature-gated code are used. No cargo-udeps installation, native model inference, real AI CLI, protected-port connection/bind, process-name kill, dependency installation, repository edit, or PR operation was performed. Tests used fake CLIs/engines and ephemeral listeners. Reserved-port numbers occur in validation/assertion data only. Existing ignored `target/` build artifacts were reused; temporary fixtures clean themselves up. No audit artifacts were put in `/tmp`.

Logs: `/home/brasillero/scratch/cleanup/cargo-test.log` and `clippy.log`. Python packaging/install suites, feature-enabled Opus/CPU variants, Windows/macOS behavior and PR #112 execution are **UNVERIFIED** here. PR #112 (`p14.1-windows-vulkan`) is not on this audited main; its diff overlaps engine/build/workflow/docs. Reconcile transcription cleanup with it after merge; do not delete feature gates based on today's default Linux build.

## Inventory

Notation: **F** fast/pure or small filesystem tests; **P** subprocesses/fake CLIs; **N** loopback HTTP; **S** deliberate waits, large audio/bodies, or timeout tests. All counts are approximate source cases unless a measured Linux count is stated; macro expansion and cfg alter totals. Platform exceptions are stated; otherwise tests are portable. No real paid CLI/model work belongs to these Rust targets.

### Workspace root

Paths in the first table are relative to `src/orchestrator/`; every listed module has its own test module.

| File/module | Count | Coverage | Class/gates |
| --- | ---: | --- | --- |
| `cancel.rs` | 2 | First cancellation reason, dropped handles | F |
| `cli.rs` | 3 | run arguments, help, invalid/duplicate options | F |
| `config.rs` | 22 | strict schema, refs, defaults, paths, budgets, serve/model/port rules | F; platform path branches |
| `error.rs` | 2 | located errors, distinct cancellation reasons | F |
| `events.rs` | 1 | operation ID form/uniqueness | F |
| `flow.rs` | 4 | formatter stage, spent/late deadlines, error mapping | F/S |
| `result.rs` | 1 | lossy non-UTF-8 metadata paths | F; Unix only |
| `serve/child.rs` | 6 | child args, lifecycle ordering, strict handshake, safe error codes | F |
| `serve/cli.rs` | 2 | serve args, safe argument errors | F |
| `serve/dictation.rs` | 6 | multipart limits/errors/order, safe messages, panic envelope, admission | F |
| `serve/http.rs` | 8 | Host/Origin/fetch guards, route deadlines/shapes, readiness | F; in-process HTTP handling |
| `serve/output.rs` | 1 | panic metadata excludes payload | F |
| `serve/supervisor.rs` | 3 | policy constants, generation/recycle rules, reap confirmation | F |
| `serve/warm.rs` | 4 | multipart byte preservation/boundary, detailed result/error validation | F |

Root library: **65 passed / 0.38 s**. `src/main.rs` has zero tests; dispatch is tested at the process boundary below. Other orchestrator files have no separate inline test modules.

| Integration file | Linux count / elapsed | Coverage | Class/gates |
| --- | --- | --- | --- |
| `tests/flow_seam.rs` | 11 / 0.50 s | reusable stage runner, literal/raw text, formatter, errors, deadlines, cancellation/events, Send | F/P/S |
| `tests/pumice_run.rs` | 39 / 1.25 s | full one-shot flow, configs, output envelopes, sibling trust, caps, failures, signals, cleanup | P/S; symlink/signals/pipe checks Unix; one non-UTF-8 config case excludes macOS |
| `tests/pumice_serve.rs` | 13 / 2.54 s | actual executable, routes/guards, startup/recovery, port conflicts, shutdown | P/N/S; actual Unix signals/parent-death cases Unix |
| `tests/pumice_smoke.rs` | 20 / 1.27 s (21 source cases) | overview, relay/formatter dispatch, sibling args/exits/help/trust, copy fallback | P/S; Windows full exit status; Unix non-UTF-8/signals/copy stress |
| `tests/serve_dictation_faults.rs` | 2 / 0.03 s | panic and unconfirmed-cleanup fail-stop | P/N; panic directory trick Unix |
| `tests/serve_dictations.rs` | 14 / 2.83 s | real multipart flow, raw/final split, errors, admission, disconnects, deadlines, cleanup | P/N/S; signals/reset/PID/permission cases Unix |
| `tests/serve_supervisor.rs` | 22 / 1.62 s | lifecycle/protocol checks, bounded recovery, probes, recycle, reap, panics/shutdown | P/N/S; actual PID assertions platform-dependent |

### Relay integration tests

Paths relative to `crates/relay/tests/`. `adapter_contract.rs` expands **12 cases × 5 adapters = 60**: Claude, Codex, Kimi, Kiro, archived Antigravity. `support/adapter_contract.rs` contains the macro and generic functions, not an additional independently run test binary.

| File | Linux count / elapsed | Coverage | Class/gates |
| --- | --- | --- | --- |
| `adapter_contract.rs` | 60 / 1.08 s | final text, restrictions, empty/private cwd, literal transport, prompts, errors, deadlines, privacy, terminal failure | P/S; Kimi argv-limit variant; archive protocol case |
| `antigravity.rs` | 12 / 0.05 s | archived protocol/parser/transport and refusal/listing | F/P; never real agy |
| `api.rs` | 47 / 2.11 s | Handy HTTP, validation/SSE, models, inspect/passthrough, errors, logging, startup | P/N/S |
| `claude.rs` | 15 / 0.05 s | flags, system file/stdin, envelope parsing/error classification, empty prompt | P |
| `cleanup.rs` | 9 / 0.00 s | only leading closed think blocks, trim, empty rejection; preserve other wording | F |
| `cli.rs` | 13 / 0.42 s | **standalone pumice-relay** default/serve, help/options, startup/config failures | P/N; non-Unicode argument Unix |
| `codex.rs` | 20 / 0.05 s | restrictions, system file/stdin, event parser, error/retry/tool records | P |
| `config.rs` | 53 / 0.01 s (54 source) | strict YAML, explicit providers/models, removed-key rejection, locations/paths/CLI/example | F/P; per-user lookup cases Unix/Windows |
| `discovery.rs` | 9 / 0.21 s | fake version probes, missing/slow probes, registry/archive metadata | F/P/S |
| `doctor.rs` | 12 / 0.42 s | command report, safe fake login-check, no other provider, argument/config errors | P |
| `fake_cli.rs` | 14 / 0.80 s | fake's reporting/output/stalling/grandchild modes and fixture replay | P/S |
| `generic.rs` | 28 / 1.02 s | archived loopback endpoint validation, exact request, framing/status/errors/caps/cancellation | F/N/S; includes IPv6 loopback test |
| `isolation.rs` | 2 / 0.01 s (4 source) | exact Claude/Codex restrictions, safe Windows npm shims | P; two Windows-only cases |
| `kimi.rs` | 31 / 0.04 s | argv/agent/environment restrictions, parser/retries/tools/errors, config/model bounds | F/P |
| `kiro.rs` | 21 / 0.08 s | agent-only cwd exception, prompts/stdin/env, parser/errors and config | F/P |
| `monitor.rs` | 12 / 0.15 s | event order, debug consent, caps, safe diagnostics, disconnects | P/N/S |
| `pipeline.rs` | 38 / 2.03 s | selection, single provider, cleanup, limits/deadlines, FIFO/cancellation, progress | F/P/S; some paused Tokio time; platform PID helpers |
| `privacy.rs` | 6 / 0.02 s | no log without consent, inspect/log redaction, private records, startup failure/warning | P/N |
| `process.rs` | 21 / 0.52 s | input/env/argv, caps/deadlines, process trees, private temp/control files and drain | P/S; permissions Unix; PID checks Unix/Windows |
| `prompts.rs` | 6 / 0.00 s | incoming prompt/message reconstruction and separation, no added instructions | F |
| `request.rs` | 15 / 0.00 s | Handy extraction/reconstruction, text parts, supported subset, malformed envelopes, emptiness | F |
| `tui.rs` | 28 / 0.26 s | event reducer, navigation, filters, scroll limits, rendering, text consent | F/S; synthetic clock/terminal, no GUI |

Relay inline modules, paths relative to `crates/relay/src/`; library total **111 / 0.06 s**:

| File/module | Count | Coverage | Class/gates |
| --- | ---: | --- | --- |
| `api/handlers.rs` | 2 | client title and credential header masking | F |
| `api/log_entry.rs` | 9 | safe request/failure/status/diagnostic rendering | F |
| `config/error.rs` | 3 | located/bare error presentation | F |
| `config/mod.rs` | 6 | per-user environment path selection, unknown locations | F; injected platform branches |
| `doctor.rs` | 8 | report counts/categories, archive endpoint presentation | F |
| `logging.rs` | 7 | consent, append/header policy, collision avoidance, permissions | F; Unix permissions |
| `monitor.rs` | 2 | UTF-8-safe caps | F |
| `process/invocation.rs` | 4 | TOML escaping/round-trip for control paths | F |
| `process/mod.rs` | 1 | scoped-root deferred cleanup is registered and drained | F/P; Unix only |
| `process/resolve.rs` | 16 | safe PATH resolution, PATHEXT, npm shim allow/refuse gates | F; actual platform entry gate Windows-specific branch |
| `process/tree.rs` | 3 | failed reap stays owned, cancelled drain, temp cleanup failure | P; two Unix-only cases |
| `providers/antigravity.rs` | 7 | archive refusal, parser, merged stdin | F |
| `providers/claude.rs` | 1 | safe failure classification | F |
| `providers/cli.rs` | 1 | invalid UTF-8 output rejected | F; Unix only |
| `providers/codex.rs` | 6 | user config/MCP disabling, fail-closed reads, pinned effort/service settings | F |
| `providers/diagnostic.rs` | 4 | error classification/detail scrubbing and caps | F |
| `providers/discovery.rs` | 4 | bounded version parsing and safe startup line | F |
| `providers/kimi.rs` | 6 | retry classification, trust override removal | F |
| `providers/kiro.rs` | 2 | final text validation, safe error payload handling | F |
| `providers/mod.rs` | 3 | registry/build/default model/no-op validation metadata | F |
| `request.rs` | 8 | content deserialization, raw inspect selection, unsupported shapes | F |
| `time.rs` | 2 | timestamps | F |
| `tui/view.rs` | 6 | Unicode fitting/truncation/safe one-line text | F |

`cleanup.rs`, `prompts.rs`, `pipeline.rs` and the other production files without inline modules are covered by integration targets. `providers/generic.rs` has its tests in `tests/generic.rs`.

### Formatter

Relative to `crates/formatter/`. Library **55 / 0.04 s**; integration **26 / 0.03 s**.

| File/module | Count | Coverage | Class/gates |
| --- | ---: | --- | --- |
| `src/builtins.rs` | 1 | builtin definition validity | F |
| `src/cli.rs` | 6 | editable YAML rendering/round-trip, whitespace and CR preservation | F |
| `src/config/mod.rs` | 11 | definitions, defaults, references, errors and per-user paths | F; Unix/Windows path tests |
| `src/definition.rs` | 11 | strict schema/id/system-size/file errors | F |
| `src/transform.rs` | 26 | raw JSON preservation, conflicts/append, shapes/duplicate keys, Handy round-trip | F/S |
| `tests/cli.rs` | 26 | actual list/show/apply/check-config/JSON/error exit contracts and relay round-trip | P; no provider invocation |

### Transcription

Relative to `crates/transcription/`. Default library **192 / 3.01 s**. These use fake inference; decoding audio fixtures is real codec work, not model execution.

| File/module | Source count (default Linux if different) | Coverage | Class/gates |
| --- | ---: | --- | --- |
| `src/audio.rs` | 59 (58) | PCM widths/extensible WAV, bounds, downmix/resample, corrupt/truncated codecs, deterministic mutation, fixtures | F/S; Opus detection vs rejection cfg alternatives |
| `src/catalog.rs` | 4 | ID/hash form and lookup | F |
| `src/cli.rs` | 13 (12) | output writers, shell quoting, download/progress envelopes, list construction | F; quoting Unix/Windows alternatives |
| `src/config/error.rs` | 3 | located errors | F |
| `src/config/mod.rs` | 27 (26) | model paths/catalog refs, defaults, schema/errors, per-user config | F; Unix/Windows path alternatives |
| `src/download.rs` | 28 (27) | pinned size/hash, redirects, deadlines, locks, atomic install, progress, permissions/symlinks | N/S; Unix permission/attack cases; Windows post-install symlink case |
| `src/engine.rs::cpp::maps_tests` | 6 (0) | Linux maps backend-label parsing | F; `engine` + Linux |
| `src/engine.rs::cpp::dynamic_backends::tests` | 19 (0) | OS-aware feature tables, compatible fallback, staged layout and read-only selection | F; `cpu-variants` + Linux |
| `src/managed.rs` | 5 | lifecycle shape/order, instance/model bounds, safe error records | F |
| `src/models_dir.rs` | 7 | platform data directories with injected environments | F |
| `src/opus/mod.rs` | 11 (0) | Opus headers/packets, trim/gain, cancellation/limits | F/S; `opus` |
| `src/opus/ogg.rs` | 21 (0) | Ogg page/sequence/granule/truncation/stream validation | F/S; `opus` |
| `src/opus/webm.rs` | 37 (0) | bounded EBML/WebM validation, live chunks, lacing, skips/padding/durations | F/S; `opus` |
| `src/server/tests.rs` | 42 (40) | OpenAI multipart/options, exact text/silence, admission/cancel/deadline, managed route guards/shapes | F/N/S; Opus alternatives; wildcard-listener case may early-return |
| `src/service.rs` | 10 | language/thread normalization, bounded reader | F |
| `tests/cli.rs` | 63 / 0.38 s | fake-engine CLI, files/limits/config/errors/models, formats, stdout failures | F/P/S; FIFO and `/dev/full` Linux; symlink Unix; Opus-rejection cfg alternatives |
| `tests/managed.rs` | 11 on Linux; no duration | real CLI + fake engine, startup/EOF/watchdog/inference cancellation, safe records | P/N/S; custom `harness=false`; thread-limit Linux; SIGTERM Unix |
| `tests/opus.rs` | 13 (0 default) | genuine Opus/WebM fixtures, duration/trim/negative inputs | F/S; file-level `opus` gate |

`src/riff.rs` has no separate test module; strict WAV behavior is extensively exercised in `audio.rs` and the CLI target. No inline tests in `build.rs` or thin main/lib glue.

### Test infrastructure, fixtures, scripts and workflows

| Path | Inventory / purpose | Execution/platform notes |
| --- | --- | --- |
| `tests/support/transcribe_stub.rs` | one portable binary; dispatcher args/exits/PIDs, one-shot scenarios, managed HTTP generations and fault scripts | test-helpers feature; Unix signal/raw-arg pieces; deliberate stalls are test inputs |
| `crates/relay/tests/support/fake_cli.rs` | one source compiled by two packages; argv/stdin/cwd/env/file/permission reporting, output/errors/grandchildren/counters | test-helpers only, never release; all scenario fields have test consumers |
| `crates/relay/tests/support/mod.rs` | per-scenario FakeCli, fixture access, re-export support modules | broad dead-code allow because each test binary imports a subset |
| `support/adapter_contract.rs` | generic contract functions and 12-case macro | keep security cases |
| `support/install.rs` | shared hard-link/cp-child executable installation | already reused by root via `#[path]`; Unix cp avoids ETXTBSY |
| `support/fake_http.rs` | async scripted loopback server, raw framing, stalls/counters | mainly archived generic coverage; supports IPv6 |
| `support/test_provider.rs` | Ready/Fail/Sleep/Barrier provider double and invocation tracking | deterministic pipeline tests |
| `crates/relay/tests/fixtures/` | Handy request; Claude 2, Codex 2, Kimi 2, Kiro 4, Antigravity 3 protocol fixtures; 4 Windows shim fixtures | all protocol files used; `windows-shims/codex.ps1` unused; see findings |
| `crates/transcription/tests/fixtures/` | WAV/MP3/FLAC/Vorbis/Opus/WebM samples, live Chrome/stereo/48k variants, duration/channel negatives, Opus provenance | every audio fixture has a code/CI consumer; keep provenance, even without include_str consumer |
| `scripts/test_package_release.py` | 8 cases: archives, modes/notices/config membership, targets/version/failures | Python 3.11+; F; not executed here |
| `scripts/test_package_npm.py` | 13 cases: fake packing, manifests/native selectors/checksums, unsafe members | F/P with fake npm; not executed here |
| `scripts/test_npm_launcher.py` | 8 cases: Node launcher I/O/args/exits/signals and trusted native lookup/errors | P/S; skips without Node; fake native is POSIX shell; Linux CI unit lane |
| `scripts/test_npm_install.py` | 5 cases: npm local/global/npx, pnpm, Bun via loopback registry | P/N/S; skips unavailable tools; execution assertions skip Windows; explicit isolated caches |
| `scripts/npm_registry_fixture.py` | local npm registry helper, no standalone test target | bounded thread join; used in install tests and release workflow |
| `scripts/package_release.py`, `package_npm.py`, `npm/bin/pumice.js` | implementations tested above | packaging and install smoke are distinct contracts |
| `crates/transcription/scripts/bench.sh` | manual real-model benchmark | deliberately NOT run, not a fake test |
| `crates/transcription/scripts/build-cpu-variants.sh`, `build.rs` | native build/staging glue | checked in feature CI; no local execution |
| `tools/transcribe-test/serve.py` and its static page | manual browser recording/test helper, not an automated test suite | origin/body/upstream validation is useful; no automated helper tests found |
| `.github/workflows/ci.yml` | fmt Linux; default workspace clippy/tests on Linux/Windows/macOS; product/relay builds, Windows core CRT check | current modules; no removed-provider invocation |
| `.github/workflows/transcription.yml` | Linux engine clippy, Opus tests, engine build; separate CPU-variant clippy/tests/build/staging | pinned small-model smoke is REAL inference and was not run; CPU variants also tested from read-only stage |
| `.github/workflows/release.yml` | Python tests; four static-core target builds/version/link/archive checks; fake registry plus actual packaged npm/pnpm/Bun installs | current core packaging, not transcription bundle packaging; no removed-provider step |

No root `tests/fixtures/` directory remains: real Handy fixture lives under the relay crate. Existing notes with old paths are historical, not an unused second fixture tree. The custom transcription runner ignores most libtest-style switches and reports no timing; do not infer its cases from `#[test]` grep alone.

## Stale-test triage: evidence and disposition

| Candidate and evidence | Current authority / actual behavior | Recommendation |
| --- | --- | --- |
| `pipeline.rs::raw_fallback_preserves_the_dictation_exactly`, `crates/relay/tests/pipeline.rs:808`; raw-return names at :182, :211, :258; comments at :829, :886 | `phase1-plan/D12`, `s4.4-failure-http-error/D1`: one provider, HTTP errors. Internal `OutcomeKind::Raw` is intentionally a failure carrier (`crates/relay/src/pipeline.rs:1`); HTTP maps it to an error (`api/handlers.rs:397`). Tests do not assert HTTP 200. | **Rewrite names/comments**, keep terminal-failure and exact-text assertions. Name it failed-outcome text retention. Do not resurrect fallback or delete the outcome simply because of its name. |
| `generic.rs::failure_returns_raw_text_and_never_runs_another_provider`, `crates/relay/tests/generic.rs:881`, `chain_config` :950 | Same single-provider failure carrier, direct test-only construction of archived adapter | Keep archive protocol/single-call intent; rename `chain_config` to pipeline config, remove unused `_endpoint` helper arg at :943. No old fallback chain is executed. |
| `bare_pumice_starts_the_service_with_the_per_user_config`, `crates/relay/tests/cli.rs:282`; `bare_pumice_with_config_flag_starts_the_service` :316; `serve_alias_still_starts_the_service` :342 | `p10-pumice-entry/D1`: root overview and `pumice relay` default-to-serve. This test's BIN is `CARGO_BIN_EXE_pumice-relay`, not root pumice. Root opposite behavior is tested at `tests/pumice_smoke.rs:90`. | **Rename** to `bare_pumice_relay_...`; clarify standalone default/explicit serve language. Keep actual standalone behavior. |
| `render_reports_an_enabled_generic_as_a_local_endpoint`, `crates/relay/src/doctor.rs:353`; archived detection tests `crates/relay/tests/discovery.rs:186`, :212 | `s2.13-archive-providers/D1` retains adapter-level tests; registry at `providers/mod.rs:120` has only four active plugins. Doctor constructs synthetic statuses; it does not register generic. | Keep archive/metadata coverage; document synthetic-only doctor case. Do not construe this as permission to enable a generic config. |
| Antigravity parser rejects tool records, `crates/relay/src/providers/antigravity.rs:337`, `crates/relay/tests/antigravity.rs:159`, contract implementation in `tests/adapter_contract.rs` | `s3.7-no-tool-checks/D1` explicitly changed **Codex and Kimi**, not the dormant Antigravity protocol; archive retains safe refusal. Kiro active parser also ignores records. | **Keep**, including no-tools and archive refusal coverage. Removing it as superseded would conflate providers. |
| Removed-default/fallback/prompts/env/options tests `crates/relay/tests/config.rs:374`, :391, :417, :910, :956, :981; removed OpenCode :971 | `s6.5-provider-list/D1`, `s6.6-remove-default/D1`, `s6.8-remove-yaml-prompts/D1`, `s6.9-remove-env-options/D1`, `phase2-plan/D7` explicitly require rejection/removal hints. D12 removes acceptance/compatibility windows, not rejection. | **Keep** strict rejection, including null values and source lines. Consolidate repeated tables only. These are not backward-compatibility shims. |
| `legacy_pumice_yaml_in_the_config_dir_is_ignored`, `tests/pumice_smoke.rs:337` | `arch-modules/D12`, `p10-pumice-entry/D3`: old default filename must not be read | **Keep**. Old filename is intentional negative input; arbitrary explicit `--config` filenames remain valid. |
| Cleanup tests keeping preambles/quotes/fences, `crates/relay/tests/cleanup.rs:84`; unfinished think tag :34 | `s3.6-minimal-cleanup/D1`, `s3.9-cleanup-reply-only/D1` replaced old heuristics | **Keep**; these prevent reintroducing superseded cleanup. No stale fence/preamble stripping assertion found. |
| Prompt tests reconstructing Handy envelope, `crates/relay/tests/prompts.rs:51`, :68, :112; neutral fallback tests `codex.rs:465`, `kimi.rs:554` | `s3.8-pure-passthrough/D1–D2`: preserve incoming message, no new envelope/instructions; neutral prompt only for required CLI transport | **Keep**, including separation/security assertions. Incoming tags and the passthrough route's transcript extraction are not relay-generated formatting. |
| Formatter `builtin:clean` instruction tests and transcription `default_model` tests (`crates/transcription/tests/cli.rs`, `default_model_is_used_when_no_flag_is_given`) | `arch-modules/D3`, `p11-formatter/D1–D5`: explicit formatter owns instructions. Transcription model selection is separate from relay provider defaults. | **Keep**. Relay no-prompt/no-default decisions do not prohibit these module-specific features. |

No accepted `PUMICE_*` legacy user configuration option, old prompt template, OpenCode provider module, old Kiro adapter, or migration alias was found. `PUMICE_TEST_*`, `PUMICE_MANAGED_TEST_CHILD`, `PUMICE_STUB_PID_FILE`, and Python tool-discovery overrides are test infrastructure. The stub's “Legacy dispatcher contract” comment (`tests/support/transcribe_stub.rs:91`) refers to its still-used fake protocol; change the word to “Dispatcher” rather than deleting the mode.

## Redundancy and consolidation

1. **Config validation copied across provider suites.** `crates/relay/tests/kimi.rs:413–497` and `kiro.rs:424–477` duplicate enabled/disabled/model/flag-like model/env/options checks already in `tests/config.rs:354–450`, :928–968. Centralize common cases in a provider-ID table in config.rs; preserve Kimi's genuinely provider-specific validation wording/behavior and all located/null-value cases. `kimi.rs:413` and :458 differ only by a freely supplied model string; combine these as parameterized explicit model cases. Approximate saving: 80–150 lines. Do not fold invocation/agent/restriction tests into this change.
2. **Same API model ordering assertions.** `models_follow_the_list_order` (`crates/relay/tests/api.rs:1025`) covers the reversed order with no-call assertions; `models_list_the_enabled_providers_in_order` (:1049) covers natural order without extra metadata checks. Parameterize both orders in the stronger case. Keep `models_lists_enabled_providers` (:971), which checks response shape, disabled exclusion, and no CLI work. `model_less_request_is_an_http_400_and_runs_nothing` (:1345) overlaps :1123, but has detection of a missing binary: merge as an availability dimension only if that distinction remains covered. Approximate saving: 25–60 lines.
3. **Process test harnesses have repeatable unsafe-to-maintain parts.** `Running::record/finish/stop` and stdout/stderr pumping are near-copies in `tests/pumice_serve.rs:169–285` and `tests/serve_dictations.rs:202–359`. A small shared *test-only* bounded child-output owner would fix both consistently. Keep flow-specific setup/assertions local. Also consider sharing bounded PID disappearance helpers repeated in relay `process.rs:443`, `pipeline.rs:1033`, `support/adapter_contract.rs:617`, root `pumice_run.rs:1426`, and dictation tests. Preserve platform semantics; do not replace those security/lifecycle tests with one test.
4. **Already consolidated: keep.** Executable installation is one shared `support/install.rs`, fixed in `27059cb` after `025f406`; root tests import it. Fake CLI is the same source, not near-identical copies. Do not undo the cp-child fallback or replace it with in-process writes. `pumice-test-transcribe` and the transcription managed fake engine test different boundaries and need different fault modes.
5. **Multipart/HTTP helpers:** root dictation, warm-child, transcription server, and managed-child tests build similar forms, but one has `flow` and the other has OpenAI `model`/options; some deliberately emit malformed raw bytes. Keep independent protocol expectations to avoid implementation/test coupling. Share only byte plumbing where the resulting diff actually shrinks. Avoid creating a universal test framework.
6. **Layered tests worth retaining:** formatter raw-value units vs standalone CLI vs root dispatcher; relay cleanup units vs pipeline vs HTTP error envelope; provider restriction/transport/empty-dir/privacy checks vs runner primitives; supervisor seam vs real process/socket dictation tests; fake archive packer vs real target archive/install/link checks. These protect different failure boundaries. All security coverage is **KEEP**, even where exact argv/temp-dir/no-content assertions overlap.

## Weaknesses and flake/hang risks

### W1 — High: Windows hang candidate and unbounded pipe shutdown

Evidence: `an_unready_service_refuses_before_reading_the_body`, `tests/serve_dictations.rs:1003`; recorded CI incident in `chore-small-fixes/backlog.md#1`, commit `dd49beb` (incident at `1bc5c5e`). First stub crashes after 300 ms, second delays ready 60 s. Test waits for generation 2, sends only 64 bytes of a large upload, expects 503, then stops service.

- `read_response` (:431) installs a 20 s **per-read idle timeout**, but waits for EOF/reset rather than Content-Length completion. It is not an absolute exchange deadline. Connects/writes at :467–474 and :507–516 have no explicit timeout.
- `Running::finish` (:327–343) bounds `try_wait` with WAIT, **then unconditionally joins the stderr thread**. That thread (:219–223) reads until EOF. The deadline does not cover join. Any inherited/retained writer can hold it indefinitely after parent exit. This is a verified hang path; whether Windows handle retention actually occurred is **UNVERIFIED**.
- Windows `stop` (:347–352) kills the core directly; Unix signals it and exercises cooperative teardown. Thus Windows's test shutdown path differs precisely around managed-child ownership/pipe closure. Inspect handle inheritance/job closure during reproduction; do not assume it is a model-load bug.
- `assert_children_gone` (:258–267) has no wait/join: it reads a log and only Unix checks live PIDs. It cannot itself explain a 50-minute Windows wait. The 60 s ready delay also cannot by itself explain 50 minutes: stub's separate EOF thread can stop it during the delay (`tests/support/transcribe_stub.rs:470–498`).

Recommendation: preserve the early-503 test; use one monotonic deadline for connect/write/framed response/process exit/output collection; move output collection to a bounded channel or cancellable I/O; retain a kill/reap guard on assertion failure; add stage breadcrumbs to identify where Windows stalls. A timeout around a `JoinHandle::join()` on the same thread is ineffective. A per-target/job watchdog is the final guard, not the only fix. Windows cause/reproduction is **UNVERIFIED**; local Linux case passed this run.

### W2 — High/medium: other waits can defeat local test deadlines

- Same final stderr join: `tests/pumice_serve.rs:251–268`. Root one-shot signal tests use unbounded `wait_with_output` at `tests/pumice_run.rs:1850`, :1883, :1912, :1947; :1905 first-event `read_line` is also unbounded. `tests/pumice_smoke.rs:557` uses unbounded wait after signalling the stub.
- `crates/relay/tests/cli.rs::startup_stderr_line` (:651) calls blocking `read_line`; its elapsed check runs only **after** a line arrives. A child that never emits/closes can hang. Its short-command helper :182 and several plain `.output()` tests have no child deadline.
- `crates/relay/tests/monitor.rs::raw_http` (:106) has no exchange timeout. API raw helper (:315) has a 30 s bound on read/write but connect is outside it; privacy helper (:115) bounds reading only, not its prior write.
- `scripts/test_npm_launcher.py:134` does `process.stdout.readline()` before its timed `wait`; no readiness bound. Its signal shell starts `sleep 30 &` (:48) and does not reap/kill that sleep in the trap, leaving a temporary descendant on the successful signal path. Stop that explicitly owned descendant in the fake and keep signal-forwarding assertions.
- CI test steps have no explicit shorter timeout (`.github/workflows/ci.yml`), so a missed test deadline can occupy most of the Actions job limit. Add a generous finite test-step ceiling alongside local fixes. Do not use process-name cleanup.

Do not add process machinery to every pure 1 ms serialization test. Prioritize waits around intentional stall/fault scenarios and shared process helpers.

### W3 — Medium: ordering sleeps / missing prerequisite assertions

- `queued_requests_run_in_arrival_order`, `crates/relay/tests/pipeline.rs:710`, sleeps 10 ms at :739 to assume request two entered the queue. Use a `Progress::queued` notification before starting request three. `a_cancelled_queued_request_frees_its_place` (:748) similarly sleeps at :775; although paused time is used there, explicitly observe queue admission before aborting. Otherwise it can merely prove an unpolled task never ran.
- `a_client_disconnect_cancels_the_transcription`, `tests/flow_seam.rs:480`, cancels after 50 ms. Under slow scheduling this can cancel during preflight instead of transcription; notify from the fake when transcribe starts.
- `concurrent_download_fails_fast`, `crates/transcription/src/download.rs:1212`, gives first download 300 ms at :1239 to acquire its lock, then assumes it owns the lock. Notify when the fake server receives the first transfer (which is after lock acquisition). This would also avoid a 3 s sleep as the test's dominant cost; use an explicit release gate with a bounded safety timeout.
- `client_disconnect_writes_one_dropped_log_entry`, `crates/relay/tests/api.rs:731`, and `client_disconnect_while_cli_runs_emits_dropped`, `monitor.rs:645`, poll for a fake report until deadline but **never assert it appeared** before dropping the socket. Both can pass on an earlier disconnect, failing to test cancellation of a running CLI. Assert/start notification first; keep both security/lifecycle assertions.
- Several monitor tests sleep 50 ms before snapshotting events (`monitor.rs:166`, :238, :294, :478, :529, :574, :619, :759). Poll the expected terminal event/count with a deadline instead. Other loops already do this; retain their bounded backoff rather than removing every sleep indiscriminately.

### W4 — Medium: tight wall-clock timing and negative observation windows

`api.rs::health_answers_quickly_while_a_dictation_runs` (:920) sleeps 300 ms, then demands response under 300 ms (:953); inspect while busy demands <500 ms (:1930). `server/tests.rs::total_deadline_counts_from_arrival_including_the_upload` (:801) sleeps 300 ms within a 400 ms total and requires <650 ms (:820). The latter can fail or hit upload timeout when descheduled; its margin over the incorrect 700 ms implementation is narrow.

Use explicit stage/permit gates and virtual time for in-process timer arithmetic where possible; retain a smaller number of generously bounded real-time end-to-end checks for actual process cancellation. Do not enlarge a 650 ms threshold past 700 ms without replacing its distinguishing assertion. Keep real contractual shutdown bounds (5 s) meaningful; add an outer test watchdog separately. Supervisor sleeps used to prove “no later restart” are negative observation windows, not automatically obsolete tests.

### W5 — Medium: fake download server lifetime contradicts its comment

`crates/transcription/src/download.rs:950–980` says `serve` runs “until dropped” but returns only `(String, Arc<AtomicUsize>)`. A detached thread owns `listener.incoming()` forever; neither returned value can stop it. Every caller leaves listener/thread resources until the whole test executable exits. Handler threads also perform blocking `read_line` without explicit I/O timeouts (:983 onward), and background handler panics are not joined.

Return a small owned server guard with shutdown/wake and bounded collection; set read/write limits, propagate handler errors to the owning test. Do not reuse the async archived-generic server wholesale: synchronous downloader tests and raw trickle handlers have different needs. No persistent process survives the test binary here; this is test resource ownership, not a daemon leak.

### W6 — Low: apparent passes and refactor friction

- `support/adapter_contract.rs::tool_activity` (:488) returns immediately for adapters whose protocol fixture is `None`; the source explicitly documents this. Report these as not-applicable rather than evidence of parser tool coverage. **Keep** actual no-tools invocation tests; a protocol with no tool event cannot be meaningfully forced into one.
- `crates/transcription/src/download.rs:1722` skips a Windows symlink assertion when privilege is missing; `server/tests.rs:1399` skips wildcard rejection if binding is unavailable; managed thread-limit case at `tests/managed.rs:570` and permission-cleanup dictation case at `tests/serve_dictations.rs:1504` skip as root. These returns show “ok”, not libtest “ignored”. Surface skip reasons in CI; keep the security tests and ensure at least one suitable runner exercises them.
- `crates/transcription/tests/cli.rs::non_regular_file_returns_invalid_input` (:329) uses `/dev/null` without a Unix gate. On Windows that missing path can produce the same InvalidInput result, so it does not establish device refusal there. Use a portable temp-directory case, plus Unix-specific device/FIFO checks. Keep the bounds/security intent.
- `generic.rs:922–938`, second half of the no-fallback case, asserts one attempt and alpha calls, but never asserts the generic HTTP server accepted zero connections. Add the server counter assertion if retaining that redundant archived pipeline wrapper.
- `providers/mod.rs::claude_and_codex_have_no_new_capabilities` (:220) is named for a former refactor, while actually checking that validators allow an explicit arbitrary model. Rename around that behavior. Avoid tests promising that an internal function must remain “a no-op” when its externally useful rule is acceptance of valid settings.

## Dead/deprecated code, fixtures and comments

**No verified removable production function/module was found from this pass.** Stable default Clippy is clean. Public archive APIs and feature-gated native code are not evidence of dead code just because the release CLI never reaches them or default tests do not compile them.

- **Verified unused fixture:** `crates/relay/tests/fixtures/windows-shims/codex.ps1`. No tracked source/script/workflow/doc consumes this file. `process/resolve.rs:482–483` creates a `.ps1` file using the `.cmd` constant and rejects it by extension before reading. Delete the unused fixture, or use it in that existing refusal test; retain the actual shim-refusal test. All other protocol/audio fixtures have consumers. `OPUS-PROVENANCE.md` is documentation, not dead fixture data.
- **Unnecessary suppressions:** `crates/transcription/src/config/error.rs:40` suppresses `general`, but production `config/mod.rs:136` uses it. `crates/transcription/src/riff.rs:21` suppresses the whole `WavFormatInfo`; all fields are used by validation/canonical reconstruction or decoder frame checks (`riff.rs:154–205`, `audio.rs:142–149`). Remove those allows and verify the supported build matrix; do not delete the constructor/fields. Other allows are justified: shared test support subset imports; non-Unix cancel reason; non-Windows shim reader; ignored child error `message: IgnoredAny` (`src/orchestrator/serve/child.rs:212`) that must be parsed without retention. Narrow `support/mod.rs`'s blanket allow only if split imports improve clarity; it is not proof all helpers are unused.
- **Test seams are live:** `ProcessTree::fail_reap`, cancelled cleanup/drain hooks, fake transcriber modes, `run_with_engine_factory`, `validate_text_with_descriptors`, and fake-provider progress hooks all have consumers. Keep failure-injection seams that make cleanup/security failures reproducible.
- **Binary collision (observed):** root `Cargo.toml:73–80` and `crates/relay/Cargo.toml:17–22` define `pumice-test-cli` from the same source. Cargo warns for the executable and `.dwp`. Give the root helper a unique binary target name and update its `CARGO_BIN_EXE_*` references, retaining one shared source. It is a packaging/build hazard, not two independent fake implementations. Keep `required-features = ["test-helpers"]`, `test=false`, `bench=false`, and release exclusions from `25f244c`.
- **Misleading comments:** `crates/relay/tests/isolation.rs:5–9` still says “fallback participation” and “stdin-only transport”; shared Kimi uses argv and failure is terminal. Rewrite to match the actual contract. `tests/pumice_run.rs:1457–1459` says a zombie “is reaped by definition once its parent exits”; a zombie is exited but still unreaped until a parent/reaper collects it. Correct the explanation; do not loosen ownership assertions. `generic.rs:876–879`, `api.rs:40`, and the pipeline names above use raw-fallback terminology for internal failures; clarify scope.
- **Stale current documentation:** `crates/transcription/README.md:28` says runtime CPU selection is planned, but `cpu-variants` and staging/feature CI exist (`p7-transcribe-cpu-variants/D2`). Replace with a link to current `docs/transcription.md` CPU-variant instructions. Its Windows section is accurate for this main and overlaps PR #112; reconcile after that PR rather than claiming Vulkan support now. Root README, module examples, install/npm instructions use current relay.yml/commands and archive status; no obsolete live example config was found.
- **Historical design notes:** `docs/research/architecture-plan.md:40–84` proposes aliases, old-name fallback and migration; round-one `architecture-modules.md:253–255` promises compatibility. Both identify themselves as proposals/historical evidence. `arch-modules/D12`, `p10/D1,D3`, `p12/D6`, and `p13.1/D2` supersede those parts. A concise dated status notice linking current decisions is preferable to rewriting old research or implementing its obsolete plan. Never edit old append-only decisions/released changelog sections to erase history.
- **Untracked `spikes/`:** `git status` was `?? spikes/` before and after; `git ls-files spikes` is empty. It occupies ~110 MB, and visible content is leftover `spikes/stt/target` build output. No Cargo workspace member, production code, test, or CI workflow references that local tree. Historical `docs/research/architecture-modules.md:187,:235,:298–304` and task reports mention `spikes/stt`/untracked spikes; the `S:` citations are to retained branch `s10.1-transcription` at `e644902`, not a dependency on this local directory. `arch-modules/D15` preserves that remote evidence. Local artifact removal would not be a PR change; leave it to the supervising agent/owner after confirming it contains nothing they need. It was not modified here.
- **CI:** no OpenCode/Antigravity real execution or old config-name packaging step found. Transcription feature lanes should remain. Release workflow builds standalone relay but packages/checks native closure primarily for core `pumice`; this is a coverage gap, not proof the relay build is obsolete. Do not delete it.

## Clear gaps (brief)

1. Missing Windows hang-stage identification/absolute pipe bounds and Windows PID/child-reap assertions described in W1. Manual Windows/macOS teardown is already retained in `p12-pumice-run#2`; do not claim Linux fake tests discharge it.
2. No dedicated automated tests found for manual browser helper `tools/transcribe-test/serve.py` origin/body/upstream forwarding guards. If that helper is maintained as part of the product test workflow, add a few fake-upstream tests; do not start real models or use its default ports in tests.
3. Only one captured Handy request, no captured OpenWhispr request or 10–20 sample dictation corpus. Already deferred in `s8.2-fake-cli#1`, with OpenWhispr work paused. Preserve Handy/audio fixtures; track missing fixtures without treating synthetic input tests as real client evidence. Do not reopen client/CORS behavior in the cleanup PR.
4. `.github/workflows/release.yml` builds and version-smokes `pumice-relay` but native-closure assertions target `pumice`. A relay-only closure check would directly protect the independent executable guarantee; this can follow distribution work, not block small cleanup. PR #112 native-bundle tests may add transcription-specific checks; those do not replace core/relay checks.

## Proposed cleanup sequence (three reviewable PRs)

**A — Bound fault-test lifetimes and remove scheduling assumptions.** Estimated 300–600 changed lines across ~7–10 test/helper files plus CI timeout; medium risk (Windows needs verification). Share only bounded process-output ownership for root serve suites; fix W1/W2 hang paths first, frame HTTP completion without waiting for EOF, add prerequisite assertions/notifications for W3, make download fake lifetime owned. Add regression scenarios for a child retaining a pipe and queued tasks that have not yet been polled. Keep real shutdown/security assertions and stage-specific errors. Validation: workspace tests once per change set on Linux; targeted Windows CI for the previously hung test and affected lifecycle targets; no inference. Split helper lifetime work into another commit if it obscures Windows diagnosis.

**B — Consolidate duplicate assertions and fix helper build identity.** Estimated 150–300 lines removed/reworked across config/provider/API tests and manifests; low/medium risk. Parameterize common config cases (including null/located removal rejection), combine both model-order cases, merge the redundant two Kimi explicit-model checks into data cases, assign a unique root fake CLI target name. Retain independent provider-specific, real fake-process, wire, privacy and archive tests. Validation: workspace clippy/tests; verify collision warnings disappear and release builds still exclude helpers. Do not pull native engine or GPU changes into this PR.

**C — Correct stale language and trim verified leftovers.** Estimated 40–100 lines across comments/docs and one fixture; low risk. Rename misleading standalone-relay/raw-fallback tests; fix isolation/zombie/dispatcher comments; remove unused `.ps1` fixture or connect it to the existing refusal test; remove the two unnecessary dead-code suppressions; refresh transcription CPU-status README and optionally add a supersession notice to historical architecture proposals. Use current decisions as references; no new owner decisions. Reconcile files touched by PR #112. Validation: workspace clippy/tests only if source/test attributes change; existing config/packaging checks for changed packaged docs. No new tests merely mirroring comment edits.

**Excluded from all three:** changing AGENTS.md, docs/spec.md or docs/product.pt-BR.md; deleting archive adapter code/tests contrary to `s2.13/D1`; removing security tests; bringing old prompts/fallback/compatibility aliases back; adding a new test framework; deleting the owner's untracked files; modifying live services; spending paid quota; treating unverified Windows/GPU behavior as established.
