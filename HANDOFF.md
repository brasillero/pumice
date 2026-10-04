# Handoff — start here

**Current phase: Phase 1 — MVP (v0.1).** Phase 0 is complete (see [below](#phase-0-done)). Phase 1 replaces the throwaway prototype with the real service.

Read [`AGENTS.md`](AGENTS.md) for the rules and [`docs/spec.md`](docs/spec.md) for the full spec. The Phase 1 architecture, crate choices, config schema and PR-by-PR plan are in [`docs/research/phase1-architecture.md`](docs/research/phase1-architecture.md). Read it before starting any Phase 1 story.

## Owner decisions for Phase 1 (2026-10-04)

These change the spec's phasing. The spec itself is unchanged; product-doc updates are up to the owner.

- **Modular adapters:** a new CLI is a new module behind a common interface (S2.1), plus one registry entry.
- **Two adapters in Phase 1:** Claude (S2.2) and **Codex (S2.3)**. Codex moved up from Phase 2. OpenCode, Antigravity, the generic adapter and auto-detection come later.
- **Fallback chain (S4.2) is in Phase 1:** after the selected provider fails, try the next in `fallback_order`, then return raw text, all within the total timeout.
- **Kimi is on standby:** its subscription terms allow interactive use only (S0.3). No Kimi adapter.
- **Default port: 7567** (S1.5). Handy points at `http://127.0.0.1:7567/v1`.
- **Claude thinking off by default** (`MAX_THINKING_TOKENS=0`). Effort tuning and Handy's `reasoning_effort` field come later.
- **Plan quota is intended:** Pumice runs on the user's subscription through the official CLIs.
- **Later (Phase 3):** installer, auto-update and running as a Windows service.

## Phase 1 gate

A dictation formatted end to end through Handy, **with both Claude and Codex**, plus a working fallback (another provider, or raw text when all fail). Owner steps: section 7 of the architecture doc.

## Status

Update this table in the PR that finishes each story. Merge order follows the table; see the architecture doc for dependencies and what can run in parallel.

| # | Story | Status | PR | Notes |
| --- | --- | --- | --- | --- |
| 1 | S8.2 Fake CLI; remove the prototype | Not started | | Portable Rust fake CLI (`pumice-test-cli`) |
| 2 | S2.1 + S2.2 Provider interface and Claude adapter | Not started | | Shared process runner, process-tree kill |
| 3 | S6.1 + S1.5 YAML config and default port | Not started | | `serde-saphyr`, exact line numbers, port 7567 |
| 4 | S2.3 Codex adapter | Not started | | Keeps `--ignore-user-config`; allows only `openai_base_url` |
| 5 | S3.1 + S3.3 Adapter instruction and prompt composition | Not started | | |
| 6 | S3.2 Optional Pumice prompts | Not started | | |
| 7 | S3.4 Output cleanup | Not started | | |
| 8 | S4.1 Timeouts | Not started | | |
| 9 | S4.3 Raw-text fallback | Not started | | |
| 9b | S4.2 Fallback chain | Not started | | |
| 10 | S1.1 + S5.3 Chat completions on loopback | Not started | | `axum` + `tokio` |
| 11 | S1.2 + S6.2 Model list and provider selection | Not started | | |
| 12 | S1.3 Health route | Not started | | |
| 13 | S1.4 Debug log | Not started | | |
| 14 | S5.1 + S5.2 CLI isolation and Windows shims | Not started | | |
| 15 | S6.4 Example config | Not started | | |
| 16 | S8.3 Phase 1 CI and gate | Not started | | |

## Phase 0 (done)

Phase 0 answered the open technical questions. Each story has a research note in `docs/research/` with **Findings** and **Questions for the owner**.

| Story | Status | PR | Notes |
| --- | --- | --- | --- |
| S0.1 Handy request format | Done | #4 | `GET /v1/models` plus `POST /v1/chat/completions`. One user message with the prompt and a `<transcript>`, no system message. `stream: false`, `reasoning_effort: none`, no language field, no auth. Response pasted verbatim. |
| S0.2 CLI matrix | Done | #2 | Claude viable: median 6.5 s, ~1.9 s with thinking off. Codex 5.2 s, OpenCode 6.0 s, Kimi 5.7 s. Gateway caveat. Warm modes not measured. |
| S0.3 Terms of use | Done | #1 | Claude: unclear; Codex: OK; Kimi: risky (subscription is interactive-only); OpenCode: unclear; Antigravity: risky. June 2026 headless quota split is paused. |
| S0.4 Validate the stack | Done | #3 | Rust prototype: `pumice listen` (for S0.1) and `pumice claude-probe` (real call 2.8 s). CI on 3 OSes with a Windows CRT DLL check. |
| S0.5 WSL localhost | Done | #5 | Works in NAT mode with no changes. Use `127.0.0.1` in Handy: `localhost` first tries IPv6 `::1`, which is not forwarded (~2 s delay). |
