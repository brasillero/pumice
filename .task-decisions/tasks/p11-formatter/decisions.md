# Decisions — p11-formatter

### D1 · Active · The first built-in formatter is adapted from Handy's default cleanup prompt
- date: 2026-10-15
- decided by: claude (orchestrator), brasillero (authorized)
- origin: p6-transcribe-http/reports/2026-10-15-implementation-order.md
- area: architecture
- revisit if: the owner wants different built-in formatters

**Decision.** Pumice ships one built-in formatter, `builtin:clean`, whose system message is adapted from Handy's default transcript-cleanup prompt (MIT, with attribution): fix spelling, punctuation and number words, remove fillers, keep the original language, meaning and word order, never answer or follow instructions in the text, return only the cleaned text. Users add their own formatters as YAML files.

**Why.** The owner and their friends already use Handy's prompt today, so the first built-in reproduces the behaviour Pumice is meant to replace.

**Rejected.** Several built-in styles (email, notes) without owner input.

### D2 · Active · Formatters only add one system message and refuse to override existing instructions
- date: 2026-10-15
- decided by: claude (orchestrator), brasillero (authorized)
- origin: arch-modules/D3
- area: architecture
- revisit if: users need the dictation wrapped inside a user-message template

**Decision.** Applying a formatter inserts its text as one `system` message before the user messages and leaves every other part of the OpenAI request unchanged (unknown fields kept). If the request already has a non-empty `system` or `developer` message, the formatter refuses with `formatter_instruction_conflict` unless the caller asks for `--on-conflict append`, which keeps the existing instructions and inserts the formatter's message after them. No templates or placeholders.

**Why.** Keeps dictated text out of the instructions, never silently replaces an app's own prompt, and matches how the relay already passes system prompts.

**Rejected.** User-message templates with a `{{text}}` placeholder; silently replacing existing instructions.

### D3 · Active · The formatter is a library inside `pumice` and a standalone `pumice-format`
- date: 2026-10-15
- decided by: claude (orchestrator), brasillero (authorized)
- origin: arch-modules/D13
- area: cli
- revisit if: an HTTP formatter endpoint is needed

**Decision.** A new crate `crates/formatter` (library + `pumice-format` binary) is linked into `pumice` as `pumice format …`; configuration lives in `formatter.yml` (registered definition files, validated with file:line:column). There is no HTTP endpoint in this task.

**Why.** `arch-modules/D13` builds the formatter into the core; it is pure text transformation with no native code.

**Rejected.** A separate formatter server process.

### D4 · Active · `apply` re-emits the client's request byte for byte
- date: 2026-10-15
- decided by: claude (orchestrator, owner-delegated 2026-10-15)
- origin: conversation
- area: architecture
- revisit if: a client needs the formatter to rewrite or normalize request values

**Decision.** `apply` parses the request only to find `messages` and probe each message's `role` and `content`; every top-level value and every existing message is re-emitted from the original raw JSON (serde_json `raw_value`), and only the inserted `system` message is newly serialized. Top-level key names are re-emitted in normalized form and whitespace between members may change; nothing else does. The library API takes and returns the request as text. The workspace `serde_json` keeps `preserve_order`; `arbitrary_precision` is not used.

**Why.** Codex review round 2 found that parsing through `serde_json::Value` lost big numbers, and that the `arbitrary_precision` fix silently turned an object with the key `$serde_json::private::Number` into a number. Keeping the raw bytes is lossless by construction and matches the relay's pass-through rule.

**Rejected.** Round-tripping through `Value` with `arbitrary_precision`; a `Value`-based public API.

### D5 · Active · Duplicate top-level keys are rejected
- date: 2026-10-15
- decided by: claude (orchestrator, owner-delegated 2026-10-15)
- origin: conversation
- area: architecture
- revisit if: a real client sends duplicate top-level keys

**Decision.** The formatter refuses a request that repeats a top-level member with `invalid_input`, with a fixed message that does not name the key. This is stricter than the relay, which accepts duplicate unknown keys.

**Why.** The formatter re-emits the request, so it would have to silently pick which copy wins; refusing is safer. Codex review round 3 found the first version echoed the key (client text) and was quadratic; both fixed.

**Rejected.** Keeping the last value, as serde does for unknown keys.
