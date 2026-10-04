# Handoff — start here

**Current phase: Phase 2 — remaining providers (v0.2).** Phase 1 is complete and its gate passed (agent-verified; see [Phase 1](#phase-1-done)). Phase 0 is [below](#phase-0-done).

Read [`AGENTS.md`](AGENTS.md) for the rules and [`docs/spec.md`](docs/spec.md) for the full spec. The Phase 2 design, the orchestrator's decisions and the PR-by-PR plan are in [`docs/research/phase2-architecture.md`](docs/research/phase2-architecture.md). The Phase 1 architecture they build on is in [`docs/research/phase1-architecture.md`](docs/research/phase1-architecture.md). Read both before starting a Phase 2 story.

## Phase 2 decisions (2026-10-04)

The owner was away and authorized the orchestrator to set specifications and proceed, reporting afterwards. These are the decisions taken (full list in the architecture doc). The spec is unchanged.

- **Scope:** OpenCode (S2.6), Antigravity (S2.5, dormant), the loopback-only generic adapter (S2.7), auto-detection and `pumice doctor` (S2.8), and a shared adapter contract suite. **Kimi (S2.4) stays on standby.**
- **New providers are off by default.** Only `enabled: true` turns one on; detection never does.
- **OpenCode:** an explicit `provider/model` is required when enabled; there is no hosted default.
- **Antigravity:** the protocol is built against the fake CLI only, and turning it on is refused until a safe per-launch tool policy exists. `agy` is never run.
- **Generic adapter:** conflicts with the AGENTS.md outbound-call rule, so its PR is opened but **not merged** until the owner approves a loopback-only exception.
- **Gate:** the contract suite passes on 3 OSes for every implemented adapter. Antigravity is reported as protocol coverage plus safe refusal.

## Phase 2 gate

All adapters pass the same criteria: the shared contract suite (no tools, empty workspace, transport, system-prompt separation, error classification, timeout, invalid output, cleanup, fallback participation, privacy, disabled behavior), on Windows, Linux and macOS. Unresolved conditions (Antigravity execution, the generic adapter's policy exception) are recorded, not relabeled as passes.

## Status

Update this table in the PR that finishes each story. Order follows the plan in `docs/research/phase2-architecture.md` §7 (its rows 1–6 were finished in Phase 1).

| # | Story | Status | PR | Notes |
| --- | --- | --- | --- | --- |
| 1 | S8.2 follow-up: shared adapter contract suite | Done (pending review) | | Suite in `tests/support/adapter_contract.rs` + `tests/adapter_contract.rs`; claude and codex run every applicable case; isolation.rs keeps exact argv and Windows shim tests |
| 2 | S2.1 follow-up: provider capabilities | Not started | | Default-off flag, risk warnings, probe metadata, full-settings validation |
| 3 | S2.8 part 1: provider detection | Not started | | Cached version probes (2 s), `/v1/models` lists available providers |
| 4 | S2.6 OpenCode adapter | Not started | | Off by default, explicit `provider/model`, deny-all inline agent |
| 5 | S2.5 Antigravity adapter | Not started | | Dormant: fake-only protocol, enabling refused, never runs `agy` |
| 6 | S2.7 Generic loopback adapter | Not started | | PR to stay open until the owner approves the outbound exception |
| 7 | S2.8 part 2: `pumice doctor` | Not started | | `--login-check` is the only quota-bearing path |
| 8 | S6.4 follow-up: Phase 2 config examples | Not started | | |
| 9 | S8.3 follow-up: Phase 2 gate | Not started | | |

## Phase 1 (done)

### Owner decisions for Phase 1 (2026-10-04)

These change the spec's phasing. The spec itself is unchanged; product-doc updates are up to the owner.

- **Modular adapters:** a new CLI is a new module behind a common interface (S2.1), plus one registry entry.
- **Two adapters in Phase 1:** Claude (S2.2) and **Codex (S2.3)**. Codex moved up from Phase 2. OpenCode, Antigravity, the generic adapter and auto-detection come later.
- **Fallback chain (S4.2) is in Phase 1:** after the selected provider fails, try the next in `fallback_order`, then return raw text, all within the total timeout.
- **Kimi is on standby:** its subscription terms allow interactive use only (S0.3). No Kimi adapter.
- **Default port: 7567** (S1.5). Handy points at `http://127.0.0.1:7567/v1`.
- **Claude thinking off by default** (`MAX_THINKING_TOKENS=0`). Effort tuning and Handy's `reasoning_effort` field come later.
- **Plan quota is intended:** Pumice runs on the user's subscription through the official CLIs.
- **Later (Phase 3):** installer, auto-update and running as a Windows service.
- **Codex residual tool risk accepted** (S5.1): "make it work first, refine its behavior later". Codex runs in its most restricted documented mode, and any run that shows tool activity is rejected.
- **Default models confirmed:** Claude `haiku`, Codex `gpt-6.1-sol`.
- **Unattended gate (2026-10-04):** the owner was away and asked to skip manual steps, so the gate below was verified by the orchestrator over HTTP with the real CLIs. The Handy GUI check is deferred to the owner.

### Phase 1 gate

A dictation formatted end to end, **with both Claude and Codex**, plus a working fallback: another provider, or raw text when all fail.

**Status: passed (agent-verified, 2026-10-04).** The owner asked for manual steps to be skipped, so the orchestrator ran `pumice serve` (release build of `main`) in WSL and sent the exact request shape Handy sends (`tests/fixtures/handy-request.json`, from S0.1) with a synthetic Portuguese dictation containing a spoken list:

| Check | Result |
| --- | --- |
| `claude` | Formatted in 2.2–2.4 s; language kept, spoken list punctuated, "escreva um email pro João" kept as text (not acted on) |
| `codex` | Formatted in 4.7 s (07:07 UTC). Later runs hit a real upstream `429 Too Many Requests`, which Pumice classified as `RateLimited` |
| Fallback chain | Claude binary missing → Codex (rate-limited at the time) → exact raw text, within the budget. Missing-then-success is covered by fake-CLI tests |
| Raw fallback | All providers missing → exact raw dictation in 19 ms |
| `/health`, `/v1/models` | `ok` + version; `claude`, `codex` listed |
| Logs | Metadata only, no dictated text |

**Owner checks still open** (not blocking Phase 2):
- the same dictation through the Handy GUI on Windows (`http://127.0.0.1:7567/v1`, model `claude`, then `codex`);
- the Windows npm-shim layout on a real install;
- the Handy version (S0.1).

### Phase 1 status

| # | Story | Status | PR | Notes |
| --- | --- | --- | --- | --- |
| 1 | S8.2 Fake CLI; remove the prototype | Done | #7 | Portable Rust fake CLI (`pumice-test-cli`) driven by `<exe>.scenario.json`; prototype removed |
| 2 | S2.1 + S2.2 Provider interface and Claude adapter | Done | #8 | `Provider` trait and registry, shared `ProcessRunner` (process group / Job Object, bounded pipes, 250 ms exit grace), restricted Claude adapter |
| 3 | S6.1 + S1.5 YAML config and default port | Done | #11 | `serde-saphyr` with `file:line:column` errors, per-user lookup, `pumice check-config`; port 7567 |
| 4 | S2.3 Codex adapter | Done | #14 | Restricted `exec` invocation with TOML-quoted control file; only `openai_base_url` option, no env overrides |
| 5 | S3.1 + S3.3 Adapter instruction and prompt composition | Done | #10 | Fixed instruction, transcript extraction, lossless Handy reconstruction, plain-text envelope escaping; pure functions + tests |
| 6 | S3.2 Optional Pumice prompts | Done | #12 | `compose_with_settings` wires `PromptSettings` into composition; block-scalar, whitespace-only and fake-CLI order tests |
| 7 | S3.4 Output cleanup | Done | #9 | `cleanup` pure function + tests; preservation guard on every rule, idempotence over positive fixtures |
| 8 | S4.1 Timeouts | Done | #13 | `pipeline.rs`: total budget with response reserve, provider cap + hard stop, fake-CLI deadline tests |
| 9 | S4.3 Raw-text fallback | Done | #13 | `pipeline.rs` outcomes: exact raw text for unknown/disabled/busy/budget/provider/cleanup failures |
| 9b | S4.2 Fallback chain | Done | #15 | `pipeline.rs` walks `fallback_order` within the total budget; skips duplicates/disabled/unbuilt; `FormatOutcome.attempts`; raw fallback carries the last failure |
| 10 | S1.1 + S5.3 Chat completions on loopback | Done | #17 | axum + tokio; exact routes, 10 MiB/5 s body bounds, SSE, loopback bind, port-taken exit 1 |
| 11 | S1.2 + S6.2 Model list and provider selection | Done | #19 | `Pipeline::model_ids` (default first) and `Pipeline::select` (trimmed, case-insensitive) are the single source of truth for listing, selection and the response `model`; covered end to end over HTTP with two fakes |
| 12 | S1.3 Health route | Done | #20 | `GET /health` returns ok+version without touching the pipeline; a slow-fake test proves it answers in <300 ms during a dictation |
| 13 | S1.4 Debug log | Done | #20 | `src/logging.rs` JSONL sink behind `debug_log.enabled`; records body+headers (only user-agent/content-type), raw_text, outcome, response; mode 0600 on Unix; `tests/privacy.rs` covers redaction and the disabled default |
| 14 | S5.1 + S5.2 CLI isolation and Windows shims | Done | #16 | npm `.cmd` shim translation to direct node launch + all-adapter isolation contract tests; Codex residual risk accepted by the owner (2026-10-04) |
| 15 | S6.4 Example config | Done | #18 | Commented `pumice.example.yaml` (all defaults, load-tested both as shipped and with documented overrides uncommented); README "run from source" section |
| 16 | S8.3 Phase 1 CI and gate | Done | #21 | CI builds and tests with `--locked` on 3 OSes, reports the binary size, keeps the static-CRT check; gate evidence below |

## Phase 0 (done)

Phase 0 answered the open technical questions. Each story has a research note in `docs/research/` with **Findings** and **Questions for the owner**.

| Story | Status | PR | Notes |
| --- | --- | --- | --- |
| S0.1 Handy request format | Done | #4 | `GET /v1/models` plus `POST /v1/chat/completions`. One user message with the prompt and a `<transcript>`, no system message. `stream: false`, `reasoning_effort: none`, no language field, no auth. Response pasted verbatim. |
| S0.2 CLI matrix | Done | #2 | Claude viable: median 6.5 s, ~1.9 s with thinking off. Codex 5.2 s, OpenCode 6.0 s, Kimi 5.7 s. Gateway caveat. Warm modes not measured. |
| S0.3 Terms of use | Done | #1 | Claude: unclear; Codex: OK; Kimi: risky (subscription is interactive-only); OpenCode: unclear; Antigravity: risky. June 2026 headless quota split is paused. |
| S0.4 Validate the stack | Done | #3 | Rust prototype: `pumice listen` (for S0.1) and `pumice claude-probe` (real call 2.8 s). CI on 3 OSes with a Windows CRT DLL check. |
| S0.5 WSL localhost | Done | #5 | Works in NAT mode with no changes. Use `127.0.0.1` in Handy: `localhost` first tries IPv6 `::1`, which is not forwarded (~2 s delay). |
