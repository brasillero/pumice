# Backlog — plugin-contract

## Active

### #1 · To do · Stop splitting the message around <transcript> tags
- type: Tech debt
- area: pipeline
- origin: conversation
- ticket: —
- created: 2026-10-07

**Context.** `src/request.rs` still splits Handy's user message around `<transcript>` into `before_text`/`text`/`after_text`, and every plugin glues them back. An empty envelope returns an empty answer without calling the CLI, a malformed one returns 400, and dictation containing literal `<transcript>` tags can make `passthrough` return only the inner words (audit B3, parked by the owner on 2026-10-07). Was the BACKLOG item "Transcript tags (B3)".

**To do.**
- [ ] Plugins receive the system prompt and the whole user message.
- [ ] Decide what `passthrough` returns.

**Where in the code.**
- `src/request.rs`, `src/providers/*.rs`

### #2 · To do · Write down the plugin contract
- type: Process
- area: providers
- origin: conversation
- ticket: —
- created: 2026-10-08

**Context.** Write down what every plugin may receive and must return, independent of its internals, so anyone can build a plugin against it and swap parts freely. Today the shared types are `FormatInput` in and `Result<String, ProviderError>` out. Goes with #1.

**To do.**
- [ ] Document the contract next to the interface.

**Where in the code.**
- `src/providers/interface.rs`

### #3 · To do · Use typed results and errors everywhere
- type: Tech debt
- area: pipeline
- origin: conversation
- ticket: —
- created: 2026-10-08

**Context.** Owner rule (2026-10-08): modules are replaceable and return typed enum results and errors, never strings. Plugins already do (`ProviderError`); audit the pipeline outcome, request, cleanup, config and API reasons. Some names still reflect the old "return the original text" design (`OutcomeKind::Raw`, `RawReason`) and should say failure.

**To do.**
- [ ] Audit the modules and close the gaps.
- [ ] Rename `Raw`/`RawReason`.

## Deferred

### #5 · Deferred · Let plugins report usage metrics
- type: Improvement
- area: providers
- origin: conversation
- ticket: —
- created: 2026-10-08
- reason: after the contract is written down (#2)

**Context.** Optional per-plugin export of tokens used, tokens per second, context size and cost hints, for cost debugging and user transparency, through the plugin contract (#2).

**To do.**
- [ ] Define the optional fields in the contract.

### #6 · Deferred · Decide who owns the call deadline
- type: Alignment
- area: pipeline
- origin: conversation
- ticket: —
- created: 2026-10-08
- reason: waiting for the owner's decision

**Context.** Plugins (through the shared process runner) enforce the call deadline, and the pipeline also caps it. Revisit whether an orchestrator should own it instead; possibly overengineering.

**To do.**
- [ ] Owner decides.

## Closed

### #4 · Discarded · Run Kimi through the shared contract suite
- type: Tech debt
- area: providers
- origin: conversation
- ticket: —
- created: 2026-10-07
- closed: 2026-10-29
- outcome: Kimi provider deleted (p16-provider-effort/D6)

**Context.** Claude, Codex, Kiro and Antigravity run the shared `adapter_contract!` suite; Kimi is tested separately in `tests/kimi.rs` because its argv transport differs. From the 2026-10-07 audit.

**To do.**
- [ ] Make the suite cover an argv transport, then move Kimi into it.
