# Backlog

Things we decided to revisit, defer or keep an eye on. Each item says where it came from. When an item becomes work, it gets a story ID, a branch and a PR, and moves out of here. The owner decides priorities.

Last updated: 2026-10-08.

## To discuss (owner decision needed)

| Item | Context | Source |
| --- | --- | --- |
| **Cleanup: is any left?** | Cleanup is now minimal (#63, S3.9): drop closed `<think>`/`<thinking>` blocks at the start, trim, reject an empty reply. It looks only at the reply; an unclosed tag is kept as text. Still open: whether to strip only one exact `<think>` block, and whether even these rules stay. Note: translation use (dictating in Portuguese, getting English) is why heading and quote heuristics were removed. Spec S3.4 still describes preamble and code-fence stripping; this is a recorded deviation. | Owner, 2026-10-07 |
| **B3: literal transcript tags** | Since S4.4 failures are HTTP errors, Pumice no longer extracts the transcript to answer a failure. Dictation containing literal `<transcript>…</transcript>` tags can make `passthrough` return only the inner words, and can affect normal requests (see **Transcript tags in normal requests**). | Audit 2026-10-07 |
| **Plugin contract standard** | Write down what every plugin may receive and must return, independent of its internals, so anyone can build a plugin against it and swap parts freely. Today the shared types are `FormatInput` in and `Result<String, ProviderError>` out (`src/providers/interface.rs`). | Owner, 2026-10-08 |
| **Decouple plugins from the Handy format** | Pumice splits Handy's user message around `<transcript>` into `before_text`/`text`/`after_text`, and each plugin (Claude, Codex, Kimi) glues the three back together. Plugins should receive just the system prompt and the whole user message, with no knowledge of transcript tags. Fine while Handy is the only client; revisit when Pumice goes generic. | Owner, 2026-10-08 |
| **Transcript tags in normal requests** | The `<transcript>` extraction still drives normal requests, not only `passthrough`: an empty envelope returns an empty answer without calling the CLI, and a malformed one returns 400. Fine while Handy is the only client; make generic later (also covers B3). | Architecture review 2026-10-08 |
| **AGENTS.md exceptions** | Two documented deviations need an owner edit to AGENTS.md: the generic adapter's loopback HTTP call (outbound-call rule) and the Kiro agent file Pumice writes into the call's temporary directory (empty-directory rule). | Audit 2026-10-07 |

## Deferred work

| Item | Context | Source |
| --- | --- | --- |
| Plugin metrics in the log | Optional per-plugin export of tokens used, tokens per second, context size and cost hints, for cost debugging and user transparency. Each plugin may report them or not, through the plugin contract. | Owner, 2026-10-08 |
| Deadline ownership | Plugins (through the shared process runner) enforce the call deadline today, and the pipeline also caps it. Revisit whether an orchestrator should own it instead; possibly overengineering for now. | Owner, 2026-10-08 |
| OpenWhispr support | OpenWhispr's dictation window is a browser context: it likely needs a restricted CORS/preflight policy in Pumice before its requests succeed, and it retries 408/429/5xx up to 3 times. It first tries `/v1/responses` and falls back to chat completions on 404. Needs a check in the real desktop app. See `~/pumice-research/client-protocols-2026-10-08.md`. | Owner, 2026-10-08 (deferred) |
| Release 0.2.0 | After local testing by the owner. CHANGELOG `[Unreleased]` is ready; the config format change makes it 0.2.0. Skip the unpublished npm 0.1.1. | Audit fix plan item 9 |
| Setup preparation | Per-adapter metadata the wizard needs: option specs, model sources, verification status. | Audit fix plan item 10 |
| `pumice setup` wizard | List installed CLIs, pick which to enable, pick a model each. "Dumb easy, frictionless, robust." | Owner, audit item 11 |
| Wispr prompt | The owner's v3 Wispr prompt lives only in Handy. Decide where an example belongs (a `docs/prompts/` file, the example config, or both). | Owner, 2026-10-07 |
| Dictation samples (S8.1) | 10–20 real dictation samples as fixtures for prompt and cleanup tests. | Spec S8.1 |
| Contract suite coverage | Kimi and generic are tested outside the shared `adapter_contract!` macro (Kimi's argv transport differs). | Audit 2026-10-07 |

## Known limitations

| Item | Context |
| --- | --- |
| Unverified adapters | Kiro and Antigravity have never run against the real CLI. Kiro has no quota or rate-limit classification yet. Antigravity cannot be enabled. |
| Partial-stdin check | A CLI that exits cleanly without reading all of its stdin is treated as a failure (#62), but the OS pipe buffer (about 64 KB) absorbs normal-size dictations, so in practice only very long dictations are caught. |
| Kimi thinking off | Pumice forces `KIMI_MODEL_THINKING_EFFORT=off` (S2.14). Kimi 2.1.1 sends it even for models that must think; such a backend could reject every call (an HTTP error, logged; the app keeps its transcript). The owner's models (k2.7-code-highspeed, k2.8) offer thinking off. If a thinking-required model is ever used, pin its lowest effort instead. Routes through non-Kimi providers ignore the override. |
| Kimi argv transport | The dictation travels as a command-line argument (visible to other local processes while the call runs), capped at 24 KiB and at the Windows command-line limit. |
| Low-severity audit items | Left after #67: removed-key errors point at the value's column rather than the key's (same line; needs a key-position visitor); Antigravity timeout/NDJSON details (adapter is dormant); process-group re-signal after a failed reap; log column alignment from request #1000. Not pursued: a Claude `subtype` check (more processing of the reply), the generic self-recursion guard for `[::1]` (Pumice only listens on IPv4, so it cannot be reached), and echoing the client's own model name in the response (only the client sees it). |
