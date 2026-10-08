# Backlog

Things we decided to revisit, defer or keep an eye on. Each item says where it came from. When an item becomes work, it gets a story ID, a branch and a PR, and moves out of here. The owner decides priorities.

Last updated: 2026-10-07.

## To discuss (owner decision needed)

| Item | Context | Source |
| --- | --- | --- |
| **Fixed prompt (`ADAPTER_INSTRUCTION`)** | Pumice prepends a fixed instruction to every call (`src/prompts.rs`), before the client's own system prompt (Handy's Wispr prompt). It may be redundant, or may conflict ("light corrections only") with a richer client prompt. It is also the only guard for clients that send bare text (AGENTS.md rule 4). Keep, shorten or make configurable? | Owner, 2026-10-07 |
| **Tool-activity checks** | Adapters reject a reply when the CLI's output shows a tool call (Kimi, Kiro, Codex, generic). The owner's direction: ignore tool activity completely and assume no tool is used, relying on the per-CLI lockdown flags. Review together with AGENTS.md rule 2 (CLIs run without tools). | Owner, 2026-10-07 |
| **Cleanup: is any left?** | Cleanup is now minimal (#63): drop a leading `<think>` block, trim, reject an empty reply. Discuss whether even these stay. Note: translation use (dictating in Portuguese, getting English) is why heading and quote heuristics were removed. Spec S3.4 still describes preamble and code-fence stripping; this is a recorded deviation. | Owner, 2026-10-07 |
| **B3: literal transcript tags** | Dictation containing literal `<transcript>…</transcript>` tags loses the surrounding words (the extraction accepts an envelope anywhere in the message). The owner will check whether it happens in practice and whether the prompt can handle it. | Audit 2026-10-07 |
| **AGENTS.md exceptions** | Two documented deviations need an owner edit to AGENTS.md: the generic adapter's loopback HTTP call (outbound-call rule) and the Kiro agent file Pumice writes into the call's temporary directory (empty-directory rule). | Audit 2026-10-07 |

## Deferred work

| Item | Context | Source |
| --- | --- | --- |
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
| Kimi argv transport | The dictation travels as a command-line argument (visible to other local processes while the call runs), capped at 24 KiB and at the Windows command-line limit. |
| Low-severity audit items | Left after #67: removed-key errors point at the value's column rather than the key's (same line; needs a key-position visitor); Antigravity timeout/NDJSON details (adapter is dormant); process-group re-signal after a failed reap; log column alignment from request #1000. Not pursued: a Claude `subtype` check (more processing of the reply), the generic self-recursion guard for `[::1]` (Pumice only listens on IPv4, so it cannot be reached), and echoing the client's own model name in the response (only the client sees it). |
