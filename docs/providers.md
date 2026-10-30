# AI provider plugins

The relay hands each request to the official CLI of a subscription the user
is logged into. This page records how each supported plugin (`claude`,
`codex`, `kiro`) calls its CLI, why, and what is known about each CLI's
limits, with the CLI version and the date the evidence was gathered. The
plugin contract itself (input unchanged, output as text or a safe error, the
cheapest settings, no fallback) is in [`AGENTS.md`](../AGENTS.md); how to
configure the providers is in [install.md](install.md#configuration).

**Evidence levels.** *Verified* means a real call observed it. *Read* means
it comes from the installed CLI's `--help` or from read-only inspection of
its binary, with no call made. *Documented* means a public source describes
it. Anything else is marked **unverified**. A CLI update can change any of
this; the version and the date say what the evidence covers.

**Gateway caveat.** On the owner's machines every CLI reaches its models
through a CLIProxyAPI gateway configured in the CLI itself (Claude Code's
`ANTHROPIC_BASE_URL` in its settings `env`, Codex's `openai_base_url`), not
straight through the provider. Real calls and timings below include that hop.

## Common to every plugin

- **Spawning.** The CLI is started directly with an argument array, never
  through a shell. On Windows, a recognised npm `.cmd` shim is translated
  into a direct `node <package entrypoint>` launch; any other shim is
  refused (`UnsupportedShim`).
- **Working directory.** Each call gets a private temporary root (0700 on
  Unix), removed afterwards. The CLI runs in its empty `workspace/`; control
  files such as the system prompt sit in a sibling `control/` directory.
  The only file a plugin writes into the workspace is Kiro's agent file
  ([Kiro](#kiro)).
- **Input.** The user message goes on stdin, where shell metacharacters are
  data, and is never placed in argv. Windows limits a whole command line to
  32,767 characters ([`CreateProcessW`](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessw)),
  so stdin also removes any input size limit from argv.
- **User configuration.** Each CLI keeps its own login, gateway and
  routing; Pumice inherits them and never reads credentials. Pumice adds
  only the per-call settings listed for each plugin.
- **Output.** Each plugin decodes its CLI's structured output and returns
  the answer text. Tool use is not policed in the output (owner decision,
  2026-10-08): tools are switched off in the invocation instead. stderr and
  event transcripts are never written to the ordinary log, since they can
  hold dictated text (AGENTS.md security rule 5).
- **Failures.** A missing binary, a nonzero exit, an error envelope, a
  timeout or an empty answer is an HTTP error with a safe reason; the app
  keeps its own text and no other provider is tried.

### Effort and thinking

`relay.yml` writes `effort: low | medium | high` on every provider entry
and `thinking: true | false` on the `claude` entry only
(`p16-provider-effort/D2`, `D4`, `D5`). Checked on 2026-10-23 (Claude Code
re-checked on 2026-10-26), read from the installed CLIs; no AI call was made:

| Pumice | Claude Code 2.1.306 and 2.1.310 | Codex 0.163.0 | Kiro 2.29.0 |
| --- | --- | --- | --- |
| `effort: low` | `--effort low` and `CLAUDE_CODE_EFFORT_LEVEL=low` (child env and the `--settings` layer's `env`) | `-c model_reasoning_effort="low"` | `--effort low` |
| `effort: medium` | `--effort medium` and `CLAUDE_CODE_EFFORT_LEVEL=medium` | `-c model_reasoning_effort="medium"` | `--effort medium` |
| `effort: high` | `--effort high` and `CLAUDE_CODE_EFFORT_LEVEL=high` | `-c model_reasoning_effort="high"` | `--effort high` |
| `thinking: false` | `MAX_THINKING_TOKENS=0` (child env and settings layer) | — (key rejected) | — (key rejected) |
| `thinking: true` | `MAX_THINKING_TOKENS=` (empty) and `"alwaysThinkingEnabled": true` in the settings layer | — (key rejected) | — (key rejected) |

All three CLIs use the same three words, so every level maps to itself. Each
plugin still has its own `cli_effort` function, so a CLI that renames a level
changes one plugin. The owner skipped a live check of `medium`, `high` and
`thinking: true` (2026-10-29), so those combinations are **unverified** at
run time.

**Configuration rules.**

- `effort` is required on every entry, enabled or not
  (`p17-serve-model-names/D3`). A missing key is reported at the entry's
  first key; `null` and any other value (`max`, `xhigh`, `minimal`, `Low`…)
  fail at the value with the three levels listed.
- A disabled entry may not write `effort: null`. Only `model` can be
  `null`, and only while disabled: no model is valid for every account,
  while `low` is valid for every provider.
- `thinking` is required on `claude` (also when disabled) and must be
  `true` or `false`. On any other entry the key fails at its line: those
  CLIs reason through effort alone (`p16-provider-effort/D5`).
- The shipped [`relay.example.yml`](../relay.example.yml), also the
  first-start `relay.yml`, writes `effort: low` on every entry and
  `thinking: false` on `claude`.

### Models that cannot honour effort

A value a provider cannot honour is refused at startup with the file and
line (`p16-provider-effort/D2`). Within `low`, `medium` and `high`, the
installed CLIs show no model that takes some levels but not others; what they
show is models that take no effort at all. Each plugin keeps a small table
of them and refuses the entry's `effort` at its line (on a disabled entry
too), with the model, the level and the evidence in the message:

```text
relay.yml:10:13: providers entry "kiro".effort: model "claude-haiku-4.5" has no reasoning effort (kiro-cli 2.29.0 ignores --effort for it: "model does not support additional fields"), so "high" cannot be honoured, nor can any other level; choose a model with reasoning effort
```

Since `effort` is required, such a model cannot be configured for that
provider at all.

| Provider | Models refused | Evidence |
| --- | --- | --- |
| Claude | Every spelling Claude Code canonicalizes to `claude-haiku-4-5`, `claude-sonnet-4-5`, `claude-sonnet-4-0`, `claude-opus-4-1`, `claude-opus-4-0` or a `claude-3-…` name: `claude-sonnet-4`, `claude-sonnet-4-20250514`, `claude-opus-4`, `claude-opus-4@20250514`, `claude-haiku-4-5-20251001`, `us.anthropic.claude-haiku-4-5-20251001-v1:0`, `Claude-Sonnet-4-5[1m]`, … ([Claude Code's model names](#claude-codes-model-names)), except the four legacy Opus spellings Claude Code remaps at run time | Read from the Claude Code 2.1.306 binary: the effort check (`JS`) canonicalizes the model (`r=qe(e)`) and returns false for exactly these names (`r.includes("claude-3-")\|\|r==="claude-opus-4-0"\|\|r==="claude-opus-4-1"\|\|r==="claude-sonnet-4-0"\|\|r==="claude-sonnet-4-5"\|\|r==="claude-haiku-4-5"`), after overrides that only apply to custom setups; the request builder then deletes `effort` (`if(!JS(h)){delete n.effort;return}`) without a warning. Its bundled model catalog agrees: none of these entries has the `effort` capability. `claude-opus-4-5` lacks that capability too but is not in this list, so it is accepted: its effort depends on the API provider (below). Same in 2.1.310 |
| Kiro | `claude-haiku-4.5` (verbatim, as `--list-models` prints it) | Verified on 2026-10-10 with kiro-cli 2.29.0 ([Kiro](#kiro), live calls 1 and 3): `[warn] failed to set effort 'low': model does not support additional fields`; the recorded run (`crates/relay/tests/fixtures/kiro/success.jsonl`, live call 1) says `"reasoning":{"support":"unavailable","effortLevels":[]}` in every `metadata` event |
| Codex | none | Read: `codex debug models --bundled` (0.163.0, no network) lists `low`, `medium` and `high` in `supported_reasoning_levels` for every model in the bundled catalog (`gpt-6-astra`, `gpt-6.1-sol`, `gpt-6-sol`, `gpt-6-luna`, `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna`, `gpt-5.5` and the hidden ones) |

**Every other model is accepted**, because nothing recorded says it cannot
honour the level:

- Kiro's levels come from its service, per model, and only show up in a
  run's `metadata` event; `kiro-cli chat --list-models --format json` lists
  ids and credit rates without effort, and the 2.29.0 binary holds no
  per-model table. Models such as `glm-5` are unknown, not unsupported.
- Claude Code resolves aliases (`haiku`, `sonnet`, `opus`, …) at run time,
  from `ANTHROPIC_DEFAULT_*_MODEL`, the account and the API provider, so
  Pumice cannot tell at startup which model an alias means. The 2.1.306
  alias tables map `haiku` to `claude-haiku-5-5` (which takes effort) by
  default, but to `claude-haiku-4-5` (which does not) on Bedrock, Vertex,
  Foundry, Mantle, Claude Platform on AWS and on Google Cloud and in Claude
  Code's gateway mode; a second table maps it to `claude-haiku-4-5-20251001`.
  When it resolves to Haiku 4.5, the configured `effort` is silently
  dropped. A proxy set only through `ANTHROPIC_BASE_URL` is not gateway mode
  ([Which setups count as a gateway](#which-setups-count-as-a-gateway)); the
  owner confirmed on 2026-10-29 that the live `haiku` entry reaches Haiku 5.5
  through CLIProxyAPI.
- `claude-opus-4-5` (and its spellings, such as `claude-opus-4-5-20251101`)
  is a known model whose effort depends on the API provider. Its catalog
  entry has no `effort` capability, but the effort check does not list it,
  so the answer comes from the last step, `AK(vd(e))` (`eq(Od(e))` in
  2.1.310): effort is sent on first-party, Claude Platform on AWS and on
  Google Cloud, Foundry and Mantle setups, and dropped without a warning on
  Bedrock, Vertex and in Claude Code's gateway mode. It is the only catalog
  model that reaches that step; names the tables do not know (such as
  `claude-opus-4-10`) reach it too. Pumice cannot tell the provider at
  startup, so it accepts the model.
- `claude-opus-4-0`, `claude-opus-4-1`, `claude-opus-4-20250514` and
  `claude-opus-4-1-20250805` (exactly these, any case, with or without
  `[1m]`) are accepted for the same reason: Claude Code's `--model`
  resolution replaces them with the default Opus on first-party, gateway
  and Anthropic cloud setups (`if(Iu()&&c7e(h)&&P1t())return … uh()`, with
  `WU=["claude-opus-4-20250514","claude-opus-4-1-20250805","claude-opus-4-0","claude-opus-4-1"]`),
  unless `CLAUDE_CODE_DISABLE_LEGACY_MODEL_REMAP` is set; elsewhere (Bedrock,
  Vertex, Foundry, Mantle) they keep their model and lose `effort`. Other
  spellings of the same models (`claude-opus-4`, `claude-opus-4-1@20250805`,
  …) are not remapped and are refused. Thinking follows the model they
  reach ([Models that cannot turn thinking off](#models-that-cannot-turn-thinking-off)).

A model added to a CLI later is accepted until its limitation is recorded
here and added to the plugin's table.

### Settings that stay pinned

Only effort and Claude thinking are configurable. Every call pins the
cheapest settings and never a fast or priority mode:

- Claude: `--settings` `fastMode: false`, `CLAUDE_CODE_DISABLE_FAST_MODE=1`,
  `ANTHROPIC_BEDROCK_SERVICE_TIER=default` (the fast-mode switch does not
  cover an inherited Bedrock `priority` tier), `CLAUDE_CODE_DISABLE_THINKING`
  cleared.
- Codex: `service_tier="default"`, `features.fast_mode=false`,
  `model_reasoning_summary="none"`.
- Kiro: no further setting exists beyond `--effort`.

## Claude Code

Plugin: `crates/relay/src/providers/claude.rs`. First verified with Claude
Code 2.1.288 on 2026-10-03; effort, thinking and model names read from 2.1.306
(2026-10-23/24) and re-checked against 2.1.310 (2026-10-26).

```text
claude -p --safe-mode --tools "" --strict-mcp-config --disable-slash-commands \
  --no-chrome --no-session-persistence --model <model> --effort <level> \
  --settings '<per-call layer>' --output-format json \
  --system-prompt-file <control>/system.txt
```

with the user message on stdin and the [pinned variables](#settings-that-stay-pinned)
in the child environment. The `--settings` layer is
`{"fastMode":false,["alwaysThinkingEnabled":true,]"env":{…}}`, with the same
pins as the child environment: a user's settings `env` block would otherwise
overwrite the child's.

| Piece | Why |
| --- | --- |
| `-p` | Print mode: one prompt in, the run ends after the answer; reads piped stdin |
| `--tools ""` | Removes every built-in tool (`claude --help`; verified 2026-10-03: `num_turns: 1`, no `server_tool_use`) |
| `--safe-mode` | Disables instruction discovery (CLAUDE.md), skills, plugins, hooks and MCP servers while keeping normal authentication (`--help`) |
| `--strict-mcp-config`, `--disable-slash-commands` | Ignore MCP configs other than an explicit one (none is given); disable skills |
| `--no-session-persistence` | No resumable session is saved in print mode (`--help`); whether every other state write is suppressed was not inspected |
| `--system-prompt-file` | Replaces the default system prompt with the client's, from a control file outside the workspace |
| `--output-format json` | One envelope whose `result` holds the text ([programmatic use](https://code.claude.com/docs/en/headless)) |

Flags deliberately not used:

- **`--bare`**: `--help` says bare mode does not read OAuth or keychain
  authentication, so it cannot use the subscription login.
- **`--restricted`**: it ignores the user, project and local settings files,
  including the `env` block of `~/.claude/settings.json` where a gateway's
  `ANTHROPIC_BASE_URL` and token usually live. Verified on 2026-10-06 with
  real calls, the proxy variables removed from the process environment and
  one flag changed at a time: the settings `env` applied with no flags,
  `--safe-mode`, `--tools ""` and both together, but **not** with
  `--restricted`. Under it Claude skipped the gateway, used the
  subscription login and failed with `API status 429` "You've hit your
  weekly limit". `--tools ""` and `--safe-mode` already cover what it was
  meant for. The adapter contract test fails if the flag returns.

Managed policy still applies under safe mode.

**Output and errors.** A nonzero exit or `is_error: true` is a failure.
Verified on 2026-10-03 with an empty `HOME`: "not logged in" exits 1 with
`{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}`
on stdout, so `subtype` cannot be trusted; this maps to `NotLoggedIn`. On an
error envelope only, "weekly limit" or "usage limit" maps to
`QuotaExceeded` (the weekly-limit text verified on 2026-10-06) and "rate
limit" to `RateLimited`; anything else is `NonzeroExit`. A `result` from a
successful envelope is never classified, so dictated text that mentions
limits cannot be misread as an error.

### Effort

`claude --help`: `--effort <level>  Effort level for the current session
(low, medium, high, xhigh, max)`. The flag parser warns and falls back to the
default effort on an unknown value (`Unknown --effort value '…' — ignoring
it and using the default effort`), so a typo would not fail a call; the YAML
check keeps that from happening. The binary also reads
`CLAUDE_CODE_EFFORT_LEVEL`, which wins over `--effort` (`CLAUDE_CODE_EFFORT_LEVEL
overrides effort for this session`); that is why the plugin pins the same
level in the variable, in the child environment and in the settings layer.

**Effort caps** (read on 2026-10-24 from the 2.1.306 settings schema and
effort resolution code; `--help` does not mention them). A level can be
lowered after Pumice chose it:

- **`maxEffortLevel`** in the user's settings. The schema says: "Anything
  above it (an /effort or /model pick, --effort, CLAUDE_CODE_EFFORT_LEVEL, a
  model default) is clamped to it, on every provider including Bedrock,
  Vertex and Foundry … across settings files the lowest value wins". It is
  also a policy-restrictive setting, so managed settings can set it.
- **`modelSettings.<model>.maxEffortLevel`**, the same cap for one model:
  "Within one settings file it replaces the top-level maxEffortLevel for
  the model ("max" exempts it); across settings files the lowest applicable
  value wins". Keys match the canonical model name and its dated, `[1m]`,
  Bedrock and Vertex spellings.
- **An organization's per-model cap**: on the first-party API and through a
  gateway, the account's model list can carry a `max_effort_level` per
  model; the CLI takes the lower of it and the settings cap.

The code resolves a call's effort in this order: `CLAUDE_CODE_EFFORT_LEVEL`
(unless `unset` or `auto`), then the per-turn and session levels
(`--effort`), then the model's default, and only then clamps the result to
the cap, silently. Pumice's settings layer writes no `maxEffortLevel`, and
since the lowest value wins across files it could not lift a cap anyway.
With a cap below the configured `effort`, Claude runs at the cap, and Pumice
cannot see it at startup, because it does not read the user's Claude
settings or account. An effort written into an inherited
`CLAUDE_CODE_EXTRA_BODY` is not clamped and replaces the configured level;
Pumice does not set that variable. None of this was run.

### Claude Code's model names

Claude Code compares a model with its tables only after canonicalizing its
name, so `claude-opus-4` and `claude-opus-4-20250514` both count as
`claude-opus-4-0`. Pumice reproduces that step
(`crates/relay/src/providers/claude/models.rs`).

**Where it comes from.** The 2.1.306 binary (`sha256 520bc6c1…71cfe`) is a
Bun executable whose JavaScript is stored as code-split chunks, each
starting with `// @bun @bytecode`. They were read in place, not run: the
effort check `JS` imports `qe` from the chunk at byte offset 240567143,
where `qe(e)` → `Iv(e)` applies the user's `modelOverrides` setting, then
calls `sw(e)`, which lowercases the name and runs `NU`. `NU` and its helpers
(`Ac`, `u`, `_2o`, `xL`, `E2e`, `QSt` in the chunk at 238260561; `LU`,
`Mv`, `qv` next to `NU`; the baked catalog `lGr` in the chunk at
238661436), in order:

1. `pye(e)`: an exact match of the catalog's provider ids (first-party,
   Bedrock, Vertex, Foundry, gateway… ids of every model), and the same
   with a Bedrock region prefix replaced by `us.`. Pumice leaves this step
   out: every one of the 210 provider ids and region variants gives the
   same name without it.
2. `Ac(e)` parses the name: trimmed, lowercased; `null` when empty or
   containing whitespace; a trailing `[1m]` or `[2m]` dropped; only the part
   after the last `/` kept; a `[<region>.]anthropic.` prefix dropped
   (`null` for a region outside `us`, `eu`, `apac`, `jp`, `au`, `us-gov`,
   `global`). The rest must start with
   `/^claude-([a-z]+)-(\d{1,2})(?!\d)(?:-(\d{1,2})(?!\d))?/` (family,
   major, minor) or the legacy `/^claude-(\d{1,2})(?!\d)(?:-(\d{1,2})(?!\d))?-([a-z]+)/`,
   and what follows must be empty or start with `-` or `@`. When it is a
   known suffix (`_2o`:
   `/^(?:-fast|-latest)?(?:-v\d{1,3}@\d{8}|[-@]\d{8})?(?:-v\d{1,3}(?::\d{1,3})?)?$/`,
   so dates, `-v1:0`, `-v2@20241022`, `-latest`, `-fast`), the result is
   the first catalog id with the same family, layout and version, a missing
   minor counting as `0` (`LU`), else the parsed base (`claude-opus-4-10`).
3. Otherwise (no parse, or another suffix such as `-foo` or `x`), the first
   of these the lowercased name contains: `claude-fable-5-1`,
   `claude-fable-5`, `claude-mythos-5-1`, `claude-mythos-5`,
   `claude-opus-5-5`, `claude-opus-5`, `claude-opus-4-8`, `-4-7`, `-4-6`,
   `-4-5`, `-4-1`, then `/claude-opus-4(?!-\d(?!\d))/` → `claude-opus-4-0`,
   `claude-sonnet-5-5`, `claude-sonnet-5`, `claude-sonnet-4-6`, `-4-5`,
   `/claude-sonnet-4(?!-\d(?!\d))/` → `claude-sonnet-4-0`,
   `claude-haiku-5-5`, `claude-haiku-4-5`, `claude-3-7-sonnet`,
   `claude-3-5-sonnet`, `claude-3-5-haiku`, `claude-3-opus`,
   `claude-3-sonnet`, `claude-3-haiku`.
4. Otherwise the name without a trailing `-YYYYMMDD` (`Mv`).

Excerpts, verbatim from the binary:

```js
function sw(e){e=e.toLowerCase();…let s=NU(e);…return s}
function NU(e){let n=pye(e);if(n!==void 0)return n;…let r=Ac(e);if(xL(r))return LU(r)??Mv(r.base);if(e.includes("claude-fable-5-1"))return"claude-fable-5-1";…if(/claude-opus-4(?!-\d(?!\d))/.test(e))return"claude-opus-4-0";…return Mv(e)}
function Mv(e){return e.replace(/-\d{8}$/,"")}
function LU(e){let n=[...KDe().models.map((r)=>r.id),...qv()];for(let r of n){let s=Ac(r);if(xL(s)&&QSt(s,e))return r}return}
function QSt(e,n){return e.family===n.family&&e.legacyVersionFirst===n.legacyVersionFirst&&E2e(e,n)===0}
function E2e(e,n){return e.major-n.major||(e.minor??0)-(n.minor??0)}
```

**How the port was checked** (2026-10-24). These functions and the catalog
were extracted verbatim from the chunks and run under node 24.21.0 in a `vm`
context (nothing else from the binary, no network, `claude` never started).
Their output for 94,263 generated spellings (5 families plus a dummy, majors
3/4/5/10/41/100, minors none/0/1/5/6/7/8/10/50/100, 21 suffixes, 10
prefixes, both layouts, aliases, odd cases and every catalog provider id in
upper and lower case) was compared with the Rust port's: no difference. The
unit test `canonicalizes_like_claude_code` keeps 43 of those cases, for
example:

| Written | Canonical (2.1.306) |
| --- | --- |
| `claude-sonnet-4`, `claude-sonnet-4-20250514`, `claude-sonnet-4@20250514`, `claude-sonnet-4-latest`, `eu.anthropic.claude-sonnet-4-20250514-v1:0` | `claude-sonnet-4-0` |
| `claude-opus-4`, `claude-opus-4-20250514`, `claude-opus-4x`, `my-claude-opus-4`, `xx.anthropic.claude-opus-4` | `claude-opus-4-0` |
| `claude-opus-4-10-foo`, `claude-opus-4-1-foo` | `claude-opus-4-1` |
| `claude-opus-4-10`, `claude-haiku-4-50`, `claude-sonnet-4-1`, `claude-haiku-4` | unchanged (versions of their own) |
| `claude-fable-5-0`, `claude-sonnet-5-0` | `claude-fable-5`, `claude-sonnet-5` |
| `haiku`, `opusplan`, `glm-5`, `claude-haiku-4.5` | unchanged |

`claude-opus-4-10` and `claude-haiku-4-50` stay accepted: `Ac` parses both
as versions of their own (minor 10 and 50), no catalog entry matches, and
`NU` returns the parsed base before the `includes` chain is reached. Only
with a suffix `_2o` does not know (`claude-opus-4-10-foo`) does the chain
apply.

**What Pumice cannot see.** Claude Code applies, before these tables,
things that only exist at run time; Pumice uses the baked rules alone:

- the `--model` resolution (`Dt`): aliases and the legacy Opus remap;
- the user's `modelOverrides` setting and the account's model knowledge,
  which can map a name before canonicalization;
- `CLAUDE_CODE_MODEL_CAPABILITIES`, and on Bedrock, Vertex, Foundry and
  Mantle the `ANTHROPIC_DEFAULT_*_MODEL` / `…_SUPPORTED_CAPABILITIES`
  pairs, which override the capability answer;
- per-model effort levels the account's model knowledge serves, and a latch
  set after the API refuses effort for a model;
- for `claude-opus-4-5` and for a model the tables do not know, whether the
  setup sends effort (`AK(vd(e))`).

### Which setups count as a gateway

Claude Code's provider (`xe()` in 2.1.306, `Ae()` in 2.1.310) is
`"gateway"` when any of three run-time slots is set, before any
`CLAUDE_CODE_USE_<cloud>` variable is looked at:

```js
function xe(){if(Wr()||knt()||Ant())return"gateway";return a.CLAUDE_CODE_USE_BEDROCK?"bedrock":…:"firstParty"}
```

- `Wr()`, the gateway credential: set when `CLAUDE_CODE_USE_GATEWAY` is set
  together with `ANTHROPIC_BASE_URL` and a token (`ANTHROPIC_AUTH_TOKEN` or a
  gateway descriptor), or from a stored `enterpriseGateway` login. With
  `CLAUDE_CODE_USE_GATEWAY` but no base URL or token it warns and ignores
  the variable.
- `knt()` (`Rit()` in 2.1.310): the process is `claude gateway --config
  <file>`, Claude Code's own gateway server.
- `Ant()` (`Pit()` in 2.1.310): managed settings require a gateway login
  (`forceLoginMethod: "gateway"` or `forceLoginGatewayUrl`), no
  `CLAUDE_CODE_USE_<cloud>` variable is set and the provider is not
  managed by a host (`CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST`).

`ANTHROPIC_BASE_URL` alone does not make a gateway: the provider stays
`"firstParty"`. The binary's gateway model discovery
(`CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY`) runs only when the provider
is `"firstParty"` and `ANTHROPIC_BASE_URL` is set, and otherwise logs
`skipped: provider is <provider>`. The alias tables pick their
`per_provider` entry from the same provider. So a CLIProxyAPI setup through
`ANTHROPIC_BASE_URL` counts as first-party: `haiku`, `sonnet` and `opus`
mean Haiku 5.5, Sonnet 5.5 and Opus 5.5, and `claude-opus-4-5` gets its
effort.

### Thinking

How Claude Code 2.1.306 picks the session's thinking mode, read from the
binary, first match wins:

1. The hidden `--thinking <mode>` flag (`enabled`/`adaptive` or `disabled`).
   Pumice does not use hidden flags.
2. `MAX_THINKING_TOKENS` when it is set and not empty (else the hidden,
   deprecated `--max-thinking-tokens`): `0` is off, a positive number is a
   fixed budget.
3. Otherwise thinking is on (`adaptive`), unless the settings say
   `"alwaysThinkingEnabled": false`.

At request time, an inherited non-empty `CLAUDE_CODE_DISABLE_THINKING` drops
the thinking parameter from the request altogether, whatever the mode, so the
plugin clears it in both cases.

- **`thinking: false`** sets `MAX_THINKING_TOKENS=0`, so the request asks
  for thinking off where the model allows it. A model that cannot turn
  thinking off is refused (below).
- **`thinking: true`** sets `MAX_THINKING_TOKENS` to an empty value
  (clearing a budget the user's environment or settings might set; the CLI
  treats empty as unset) and adds `"alwaysThinkingEnabled": true` to the
  settings layer. Pumice sets no budget. On models with adaptive thinking
  (the binary lists `claude-3-*`, `claude-opus-4-0`/`4-1`/`4-5`,
  `claude-sonnet-4-0`/`4-5` and `claude-haiku-4-5` as models without it),
  the request carries `thinking: {type: "adaptive"}` with no budget. On the
  others the request carries `thinking: {type: "enabled", budget_tokens: N}`,
  where `N` is the model's output upper limit minus one, clamped to at least
  1024 and below the request's maximum output tokens: in practice the
  request's whole output allowance, as a cap. The owner chose to keep the
  CLI's own budget (2026-10-29).

The binary has an error for `effort` levels a model accepts only with
thinking on (`Effort '…' isn't available with thinking turned off on this
model · … lower effort to High, or turn thinking back on`). Its hint offers
`high` as the fallback, so `low`, `medium` and `high` with `thinking: false`
should not hit it; this was not run.

### Models that cannot turn thinking off

With `MAX_THINKING_TOKENS=0` the session asks for `{type:"disabled"}`
thinking, but on some models Claude Code sends adaptive thinking anyway. In
the 2.1.306 binary (chunks at 240567143 and 246787320):

```js
function u7e(e){return sb(qe(e),"rejects_disabled_thinking",e)===!0&&NHe(e)}
… thinking:{supported:H6s(e),alwaysOn:u7e(e),rejectsDisabled:NHe(e),capsEffortWhenDisabled:Y0r(e)} …
function D6s(e,n){if(!n||e.type!=="disabled"||e.mechanical===!0)return e;return{type:"adaptive",…}}
… nn=D6s(r,ft.thinking.alwaysOn||…) …
```

`sb` reads the capability from `CLAUDE_CODE_MODEL_CAPABILITIES`, the
account's served capabilities, or the baked catalog entry of the canonical
name; `NHe` is false for the older models it lists and otherwise follows the
same capability. The baked catalog gives `rejects_disabled_thinking` to
exactly six models: `claude-haiku-5-5`, `claude-sonnet-5-5`,
`claude-opus-5-5`, `claude-fable-5`, `claude-fable-5-1`,
`claude-mythos-5-1`.

Pumice refuses `thinking: false` for every spelling that canonicalizes to
one of them, on a disabled entry too, at the `thinking` key:

```text
relay.yml:90:15: providers entry "claude".thinking: model "claude-haiku-5-5" cannot turn thinking off (Claude Code 2.1.310 sends adaptive thinking for it even with thinking off), so "false" cannot be honoured; write "thinking: true" or choose a model that can turn thinking off
```

An effort refusal is reported first when both apply (no model is in both
lists). Aliases and the four remapped legacy Opus spellings are accepted, as
for effort, although on a first-party setup all of them reach a model that
keeps thinking on. From the catalog's alias tables, unless
`ANTHROPIC_DEFAULT_*_MODEL` or the account's served alias targets say
otherwise:

| Written | First-party | Gateway mode | Cloud providers |
| --- | --- | --- | --- |
| `haiku` | `claude-haiku-5-5`: thinking always on | `claude-haiku-4-5`: no effort | `claude-haiku-4-5`: no effort |
| `sonnet` | `claude-sonnet-5-5`: thinking always on | `claude-sonnet-4-6` | `claude-sonnet-4-5` (no effort) on Bedrock, Vertex, Foundry, Mantle; `claude-sonnet-4-6` on Claude Platform on AWS; `claude-sonnet-5-5` (thinking always on) on Claude Platform on Google Cloud |
| `opus` | `claude-opus-5-5`: thinking always on | `claude-opus-4-7` | `claude-opus-5-5` (thinking always on) except Foundry (`claude-opus-4-6`) |
| `claude-opus-4-0`, `claude-opus-4-1`, `claude-opus-4-20250514`, `claude-opus-4-1-20250805` | remapped to the default Opus, `claude-opus-5-5`: thinking always on | remapped to `claude-opus-4-7` | remapped to `claude-opus-5-5` (thinking always on) on Claude Platform on AWS and on Google Cloud; elsewhere not remapped: no effort |

So the shipped example's `model: haiku` with `thinking: false` keeps
thinking on through a first-party setup (the owner's live entry reaches
Haiku 5.5, 2026-10-29); `relay.example.yml` says so in a comment. The
owner kept the example as it is (`p16-provider-effort/D3`).

### Re-check on Claude Code 2.1.310

On 2026-10-26 the installed Claude Code was `2.1.310 (Claude Code)`
(`sha256 b7367643…bb47df56`). It was checked the same way as 2.1.306: its
2,552 `// @bun @bytecode` chunks were read in place, the relevant functions
extracted verbatim and run under node 24.21.0 in a `vm` context (no `claude`
process, no network). Everything above holds for 2.1.310; the minifier
renamed most functions:

| 2.1.306 | 2.1.310 | Result |
| --- | --- | --- |
| baked catalog `lGr` | `G5r` | byte-identical (`sha256 b46409c9…1123c95` for both extracts) |
| `qe` → `Iv` → `sw` → `NU` | `Xe` → `mk` → `_b` → `XB` | same code modulo names |
| `Ac`, `u`, `_2o`, `xL`, `E2e`, `QSt`, `LU`, `Mv`, `qv`, `pye` | `Tc`, `u`, `W9o`, `H0`, `jqe`, `aTt`, `JB`, `pk`, `Pk`, `F9` | same code modulo names; the 94,263 spellings give the same canonical name under 2.1.310's functions (0 differences), so the Rust port still matches |
| effort check `JS` | `Tw` | same code modulo names, same five names and `claude-3-`; the request builder still runs `if(!Tw(h)){delete n.effort;return}` |
| `AK` / `vd` / `xe` / `Iu` | `eq` / `Od` / `Ae` / `bu` | same code modulo names |
| thinking checks `u7e` / `NHe` | `pnt` / `C0e` | same code modulo names, same list of older models; `alwaysOn:pnt(e)` still feeds the disabled-to-adaptive substitution (`Fni`, was `D6s`) |
| legacy Opus remap list `WU` | `oH` | the same four ids |
| `--model` resolution `Dt` | `$t` | same result: `h=s?Mt(r).trim():r` became `h=Ot(r).trim()`, where `Ot` is `Mt` renamed (`e.replace(/\[1m\]$/i,"")`) |

`claude --help` still lists `--effort <level>` with the same five levels,
and the binary still carries the strings quoted above. Pumice's messages and
tests name 2.1.310; the excerpts keep 2.1.306's names, where they were read.

## Codex

Plugin: `crates/relay/src/providers/codex.rs`. First verified with
`codex-cli 0.160.0` on 2026-10-03; user-config inheritance verified on
2026-10-06; effort and catalog read from 0.163.0 on 2026-10-23.

```text
codex --no-daemon exec [--ignore-user-config] --ignore-rules --ephemeral \
  --skip-git-repo-check --sandbox read-only --color never --json -m <model> \
  -c 'web_search="disabled"' -c features.shell_tool=false \
  -c features.unified_exec=false -c features.multi_agent=false \
  -c features.hooks=false -c features.view_image=false \
  -c features.remote_plugin=false -c features.plugins=false \
  -c features.apps=false -c 'notify=[]' -c 'service_tier="default"' \
  -c features.fast_mode=false -c 'model_reasoning_summary="none"' \
  [-c mcp_servers.<name>.enabled=false …] \
  -c 'model_instructions_file="<control>/system.txt"' \
  -c 'model_reasoning_effort="<level>"' -
```

with the user message on stdin (`-`).

- **System prompt.** `model_instructions_file` replaces Codex's built-in
  instructions ([configuration reference](https://developers.openai.com/codex/config-reference)).
  Codex rejects an empty instructions file, so when the client sends no
  system prompt the plugin writes the neutral `Follow the user's message.`
  (AGENTS.md, Provider plugins).
- **No complete "no tools" switch.** The configuration reference documents
  individual switches (shell, unified exec, multi-agent, hooks, web search,
  plugins); no single tool-removal switch was found. `--sandbox read-only`
  is a sandbox for model-generated commands (`codex exec --help`), not tool
  removal. `view_image` is off because it can read files and leaves no trace
  in the JSONL stream; plugins and apps can bring their own tools; `notify`
  runs a program after the turn. The owner accepted this residual risk on
  2026-10-04: Codex runs in its most restricted documented mode (AGENTS.md
  security rule 2). In the 2026-10-03 injection check (a dictation asking it
  to list files, run `ls` and search the web) Codex returned the sentence
  formatted with no command or tool items in its JSONL, but the input was
  about 5.3k tokens for a 100-word prompt, which suggests built-in tool
  definitions or instructions are still sent.
- **User configuration is inherited** (owner decision, 2026-10-05; verified
  2026-10-06). Codex reads its own `$CODEX_HOME/config.toml`, else
  `~/.codex/config.toml`, so a gateway (`openai_base_url`), provider or
  profile there applies without repeating it in Pumice. `--ignore-user-config`
  is not passed normally: on 2026-10-03 it was found to drop the user's
  `openai_base_url` too, after which Codex sent the environment's API key to
  `api.openai.com` and failed with `401 Incorrect API key`. What the user's
  file could add to the tool surface is switched off per call:
  - `-c mcp_servers={}` does **not** clear user servers, because `-c`
    merges tables (checked with `codex mcp list`, 2026-10-06). Instead
    Pumice reads only the table names under `[mcp_servers]` and passes
    `-c mcp_servers.<name>.enabled=false` for each; it never reads values.
  - `features.plugins=false`, `features.apps=false` and `notify=[]` are
    always passed.
  - **Fail closed:** if the file exists but cannot be read or parsed, if
    `mcp_servers` is not a table, or if a server name is not a bare TOML
    key, the call adds `--ignore-user-config`. A missing file is inherited
    as empty. The file is read again for every call.
  - Verified on 2026-10-06: one call through Pumice with no gateway option
    failed with the gateway's own signature (WebSocket resets, then
    `429 Too Many Requests`), the same as with an explicit option.
- **Persistence.** `--ephemeral` prevents session-file persistence
  (`--help`); suppression of every cache, log or database write is
  **unverified**.
- **Output.** `--json` emits JSONL events on stdout. Success needs a
  `turn.completed` event and exit 0; the answer is the text of the last
  completed `agent_message` item. Reasoning, warnings and any other item
  carry no answer text and are ignored ([non-interactive mode](https://developers.openai.com/codex/noninteractive)).
- **Errors.** Failure events (`error`, `turn.failed`) carry CLI
  diagnostics, never dictation, so they are the only text classified: `401`
  with "missing bearer" is `NotLoggedIn`, `401` with "incorrect api key" is
  `AuthenticationRejected`, quota wording ("usage limit", "exceeded your
  current quota", `insufficient_quota`, …) is `QuotaExceeded`, and any other
  429 or "rate limit" is `RateLimited`. Verified on 2026-10-03 with an empty `HOME`:
  "not logged in" took about 17 s (five reconnect attempts) before exit 1
  with `turn.failed` `401 Unauthorized: Missing bearer or basic
  authentication`.
- **Effort.** `codex exec --help` documents `-c key=value` overrides; the
  0.163.0 config schema contains `model_reasoning_effort`, and its effort
  names include `minimal`, `low`, `medium`, `high`, `xhigh`, `max` and
  `ultra`. Which levels a model accepts comes from the model catalog; see
  [Models that cannot honour effort](#models-that-cannot-honour-effort).
  The value stays the last `-c` before the stdin marker.
- **Warm mode.** `codex app-server --help` advertises an experimental server
  (stdio, Unix sockets, WebSockets); `mcp-server` is not in the top-level
  help. Not used and not measured.

## Kiro

Plugin: `crates/relay/src/providers/kiro.rs`, provider id `kiro`, written
from scratch on owner request (2026-10-10). Checked against
`kiro-cli 2.29.0` (WSL, Linux x86-64, logged in with IAM Identity Center) on
2026-10-10, with `--help`, the official docs ([headless mode][headless],
[agent configuration reference][agent-ref], [exit codes][exit-codes],
[reasoning effort][effort]) and the [live checks](#kiro-live-checks); effort
from `relay.yml` since 2026-10-23.

```text
kiro-cli chat --agent-engine v2 --no-interactive --output-format stream-json \
  --trust-tools= --agent pumice --model <model> --effort <level>
```

run in the call's workspace with the user message on stdin and one
workspace file, `.kiro/agents/pumice.json` (below). The child also gets
`KIRO_DISABLE_TELEMETRY=1` and `KIRO_NO_AUTO_UPDATE=1`.

| Piece | Why |
| --- | --- |
| `chat --no-interactive` | Headless mode ([headless]): one instruction in, the run ends after the turn |
| `--agent-engine v2` | Pinned, so a change of the CLI's default engine cannot change behaviour. V2 is the 2.x default ("the pre-3.0 default" in `--help`). See [engine choice](#kiro-engine-choice) |
| `--output-format stream-json` | The run's ACP events as JSON Lines on stdout; logs and warnings go to stderr. Works only on V2/V3 |
| `--trust-tools=` | "trust no tools" (`chat --help`). In non-interactive mode an untrusted tool call is denied, not asked |
| `--agent pumice` | The one-off agent file Pumice writes into the workspace |
| `--model <model>` | From `relay.yml`; V2 overrides the agent's and the saved model with it. `kiro-cli chat --list-models --format json` (no chat call) lists `auto` and the model ids with a credit multiplier; Pumice ships no default |
| `--effort <level>` | From `relay.yml` ([effort and thinking](#effort-and-thinking)). Levels depend on the model ([effort]); a model without reasoning prints `[warn] failed to set effort '<level>': model does not support additional fields` on stderr and runs (verified for `low` with `claude-haiku-4.5`; expected but not run for `medium` and `high`), so such a model is refused at startup. The S2.14 check (kiro-cli 2.29.0, 2026-10-11) found GPT-5.6 Terra/Sol/Luna also offer `none`, cheaper than `low`; Pumice's common scale has no `none`, so `low` is the cheapest Kiro level it sends |
| stdin | "When stdin is piped and there is no positional argument, Kiro reads the full stream as the instruction" ([headless]) |
| `KIRO_DISABLE_TELEMETRY=1` | Present in the 2.29.0 binary next to the legacy `Q_DISABLE_TELEMETRY` (AGENTS.md: no telemetry). Its effect was not measured |
| `KIRO_NO_AUTO_UPDATE=1` | Present in the binary; no update checks from runs Pumice starts. Its effect was not measured |

Nothing else is set or removed: the user's own login, settings
(`~/.kiro/settings/cli.json`), API key (`KIRO_API_KEY`) and `KIRO_HOME` are
inherited. Pumice never reads or writes `~/.kiro`.

### Kiro agent file

Kiro has no `--system-prompt` flag. The documented way to give a system
prompt is a custom agent's `prompt` field ([agent-ref]), and `kiro-cli agent
--help` says "local agents are only discovered if the command is invoked at a
directory that contains them". So, as an owner-approved exception to
AGENTS.md security rule 3 (2026-10-10), Pumice writes exactly one file into
the call's workspace (directories 0700, file 0600, removed with the
temporary root):

```json
{
  "name": "pumice",
  "description": "Answers one message without tools.",
  "prompt": "<the client's system prompt, verbatim; empty when it sent none>",
  "tools": [],
  "allowedTools": [],
  "mcpServers": {},
  "includeMcpJson": false,
  "resources": [],
  "hooks": {}
}
```

- `tools: []` gives the agent no tools; `allowedTools: []` pre-approves
  none; `mcpServers: {}` and `includeMcpJson: false` add no MCP servers and
  do not merge `mcp.json`; `resources: []` loads no files; `hooks: {}` runs
  no hook commands.
- An empty `prompt` is accepted (live call 3), so the client's system prompt
  is sent as is, including an empty one (Handy sends none). Kiro's own
  built-in instructions stay underneath it, about 3.6 % of a 200k context
  before the first message, which Pumice cannot remove.
- **`file://` prompts are refused.** The `prompt` field also accepts a
  `file://` URI, which Kiro reads from disk ([agent-ref]). A client system
  prompt starting with `file:` (ignoring leading whitespace and case) is
  refused with `UnsupportedSystemPrompt` instead of written (security
  rule 4).
- **A workspace agent wins.** With `KIRO_HOME` pointed at a scratch
  directory holding a global `pumice` agent, `kiro-cli agent list` in a
  workspace with a local one printed `WARNING: Agent conflict for pumice.
  Using workspace version.` So a user's own global `pumice` agent cannot
  replace the no-tools one.
- `kiro-cli agent validate` exits 0 for this file but also for unknown
  fields, so it proves nothing about field support; the live calls do.

### Kiro output and errors

V2 `stream-json` writes one JSON object per line, `{"type": …, "data": …}`:

| `type` | Meaning | Plugin |
| --- | --- | --- |
| `runStarted` | `data.payloadSchema: "acp"`, `data.engine: "v2"` | Required first, exactly once |
| `metadata` | Session id, context usage, `reasoning.effortLevels`, credits used | Ignored |
| `sessionUpdate` | ACP `session/update`: `data.update.sessionUpdate` is `agent_message_chunk` (text in `content.text`), or `agent_thought_chunk`, `tool_call`, `tool_call_update`, … | Text chunks concatenate into the answer; every other update is ignored |
| `runFinished` | `data.status: "success"`, `stopReason`, `finalText`, `finalTextTruncated` | Required last, exactly once, with `status: "success"` |
| `runError` | `data.stage`, `data.message` (free text), exit code 1 | Failure |

The answer is the concatenation of the `agent_message_chunk` texts, which
equalled `runFinished.finalText` in every live call; `finalText` itself is
not used because it can be truncated. Unknown event types inside the run are
ignored; any event before the start or after the finish is invalid output.

Exit codes ([exit-codes]): 0 success, 1 general failure ("auth error,
invalid args, operation failed"), 3 MCP startup failure (only with
`--require-mcp-startup`, not used), 4 agent not found. Quota, rate-limit and
model errors have no code of their own, and `runError` has no structured
cause, so every `runError` and nonzero exit is `NonzeroExit` (HTTP 502),
without matching text.

**Not logged in hangs.** With an empty scratch `HOME` (no login, no model
call), `--no-interactive` still starts a device-code login: it prints a blank
line, then `Confirm the following code in the browser`, a code and a URL on
stdout, and waits (stopped by `timeout` after 40 s). In Pumice this ends as
`Timeout` (HTTP 504) when the provider timeout runs out. If the CLI ever
exits after printing that prompt, the plugin reports `NotLoggedIn`; the
prompt is matched only on a line that is not a JSON event, so model output
cannot trigger it.

### Kiro engine choice

Live calls 1 and 2 ran the same request on both engines:

| Engine | Time | Stream |
| --- | --- | --- |
| V2 | 3.1 s | `runStarted`, `metadata`, text chunks, `runFinished` |
| V3 | 5.1 s | Also mode/command/config updates, a `fetch_cloud_config` tool call, remote-session and content-collection logs on stderr, a KAS (Node) server start; the context breakdown still listed ~5.9k tokens of built-in tool definitions |

V2 is faster, simpler and quieter, and is the 2.x default, so it is pinned.
**Limitation:** if CLI 3.0 drops V2, the plugin will need a V3 recipe (V3
also reads hooks from the workspace, turns knowledge and code intelligence on
in headless mode, and treats `--trust-tools` as exact internal tool ids).

### Kiro live checks

On 2026-10-10, four calls straight to the CLI and one through Pumice (0.006
to 0.014 credits each):

1. **V2, French system prompt, `claude-haiku-4.5`, `--effort low`:** exit 0,
   3.05 s; the answer followed the system prompt; stderr held only the
   effort warning.
2. **V3, same request:** exit 0, 5.08 s ([engine choice](#kiro-engine-choice)).
3. **Tool lockdown, V2, empty system prompt:** the user message asked it to
   run `touch pwned.txt` and `ls -la` and read `/etc/hostname`. Exit 0, no
   `tool_call` event, the reply said it has no shell or file tools, and the
   workspace afterwards held only `.kiro/`.
4. **Unknown model `no-such-model-xyz`:** exit 1, last line
   `{"type":"runError","data":{"sessionId":"…","stage":"prompt","message":"Internal error (code -32603): Encountered an error in the response stream: The model 'no-such-model-xyz' is not available. …"}}`.
5. **Through Pumice** with only `kiro` enabled (`claude-haiku-4.5`) and the
   Handy fixture carrying a Portuguese spoken list: an OpenAI
   `chat.completion` in 4.7 s. The plumbing was correct, but the reply
   answered the dictation like a request instead of formatting it: Kiro's
   built-in instructions sit under Handy's prompt, and with that model they
   won. Whether Kiro suits a client depends on the model and the app's
   prompt; Pumice adds no instructions by design.

`claude-haiku-4.5` has since become unconfigurable, because it ignores every
effort level.

### Kiro residual risks

- **Session history:** V2 saves every run under the user's own
  `~/.kiro/sessions/cli/<session id>.json(l)`, including the dictated text.
  There is no per-run switch, and Pumice does not touch `~/.kiro`.
- **Hooks and MCP from the user's profile:** the agent sets none, and
  `tools: []` leaves the model no tool to call. Whether V2 still starts
  servers from a user `mcp.json` despite `includeMcpJson: false` was not
  observed (the test account had none).
- **No error classes:** login, quota and rate-limit failures are all HTTP
  502, or 504 for the login hang.
- **Engine pin** and **Kiro's own base instructions**: see above.

## Subscription terms evidence (2026-10-03)

**Historical findings, not legal conclusions or legal advice.** Live web
research on 2026-10-03 (Anthropic quota notice re-checked on 2026-10-04)
assessed Pumice's personal use: a local program spawning an official CLI
non-interactively to format its owner's dictation, without tools, in a fresh
empty temporary directory, without reading or reusing credentials. Only
Anthropic and OpenAI below apply to the current providers; **Kiro's terms
were not reviewed**. Terms change; re-read the sources before relying on
this.

### Anthropic: Claude Code with Pro or Max

Rating at the time: **unclear**. Sources, all accessed 2026-10-03:
[Consumer Terms](https://www.anthropic.com/legal/consumer-terms) (displayed
effective 2025-10-08), [Usage Policy](https://www.anthropic.com/legal/aup)
(displayed effective 2025-09-15),
[Claude Code legal and compliance](https://code.claude.com/docs/en/legal-and-compliance),
[run Claude Code programmatically](https://code.claude.com/docs/en/headless),
[Agent SDK overview](https://code.claude.com/docs/en/agent-sdk/overview),
[use the Agent SDK with your Claude plan](https://support.claude.com/en/articles/15036540-use-the-claude-agent-sdk-with-your-claude-plan),
[Claude Code with Pro/Max](https://support.claude.com/en/articles/11145838-use-claude-code-with-your-pro-or-max-plan),
[Pro](https://support.claude.com/en/articles/8325606-what-is-the-pro-plan),
[Max](https://support.claude.com/en/articles/11049741-what-is-the-max-plan).

- Consumer Terms §3(7) prohibit automated or non-human access except
  through an API key or where Anthropic explicitly permits it. The Usage
  Policy applies to submitted dictation too.
- Anthropic documents `claude -p` for scripts, pipelines and CI/CD, and its
  subscription help page says `claude -p` and Agent SDK use consume
  subscription limits. This supports personal headless use through the
  official CLI.
- The legal documentation directs developers building products or services
  to API keys and prohibits routing requests through consumer credentials on
  behalf of users; the SDK overview requires prior approval for third-party
  products offering subscription rate limits. Whether a distributed local
  wrapper that only spawns the already-authenticated official CLI falls
  within that was **unconfirmed**. No Anthropic clarification was sought.
- **Quota.** A separate headless quota announced for June 2026 was paused:
  the help article's update said `claude -p`, SDK and third-party app use
  still consume subscription limits, and its separate monthly credits were
  not taking effect. Re-checked on 2026-10-04: still paused, no separate
  credit, and Anthropic said any future change would be announced first
  (also reported by [The New Stack](https://thenewstack.io/anthropic-pauses-claude-agent-sdk-subscription-change/)
  and [Zed](https://zed.dev/blog/anthropic-subscription-changes)). Claude and
  Claude Code share the plan's five-hour and weekly limits; there is no fixed
  "dictations per day" allowance.
- **Owner decision (2026-10-04):** Pumice is meant to run on the
  subscription plan quota; `claude -p` using the normal limits is what the
  owner wants.

### OpenAI: Codex CLI with ChatGPT Plus or Pro

Rating at the time: **OK** (an interpretation of the documented workflow).
Sources, all accessed 2026-10-03:
[Terms of Use outside the EEA, Switzerland and UK](https://openai.com/policies/row-terms-of-use/),
[European Terms](https://openai.com/policies/terms-of-use/),
[Service Terms](https://openai.com/policies/service-terms/),
[non-interactive mode](https://learn.chatgpt.com/docs/non-interactive-mode),
[authentication](https://learn.chatgpt.com/docs/auth),
[pricing and usage limits](https://learn.chatgpt.com/docs/pricing).

- OpenAI documents `codex exec` for scripts, pipelines and output consumed by
  other tools, including automation with a Codex user account when
  ChatGPT/Codex limits are needed (API keys are the recommended default for
  CI). CLI authentication supports ChatGPT subscription access.
- The consumer terms prohibit programmatic extraction of data or output,
  account sharing and bypassing rate limits, and require following product
  documentation. Interpretation at the time: the documented `codex exec`
  workflow supports ordinary personal CLI use; it is not permission to
  scrape ChatGPT, share account access or evade limits. No Pumice-specific
  approval was found.
- The documented account-authenticated CI workflow includes credential
  handling that Pumice must not do; Pumice only uses the existing local
  login.
- **Quota.** Plus and Pro include Codex CLI access; local and cloud work
  share plan allowances that vary by model, context, reasoning and tools.
  The pricing page then estimated 15–160 GPT-6.1 Sol local messages per five
  hours on Plus (estimates, not guarantees) and no five-hour limit on Pro,
  with weekly limits possible.

## Historical latency (2026-10-03)

Cold starts, one process per call, a 55-word English dictation, wall time
from spawn to exit, Claude Code 2.1.288 and Codex 0.160.0, with an S0.2-era
prompt and flags (not the current invocations). **Every call went through
the owner's CLIProxyAPI gateway**, so the numbers include that hop and may
differ from direct use.

| CLI (model, effort) | Runs (ms) | Median | Max |
| --- | --- | --- | --- |
| Claude (Haiku 4.5, `--effort low`) | 3329, 5136, 6533, 10840, 28544 | 6.5 s | 28.5 s |
| Claude (Haiku 4.5, thinking off: `MAX_THINKING_TOKENS=0`) | 1854, 1973 (2 runs) | ~1.9 s | 2.0 s |
| Codex (gpt-6.1-sol, `model_reasoning_effort=low`) | 4792, 5125, 5179, 5331, 6644 | 5.2 s | 6.6 s |

Claude's variance came from thinking, not startup: thinking tokens across the
five runs were 311, 599, 694, 1198 and 3390 (the 28.5 s run), and process
startup plus API overhead was under 1 s. This is why thinking was off by
default (owner decision, 2026-10-04). Warm or server modes were not
measured.

[headless]: https://kiro.dev/docs/cli/headless/
[agent-ref]: https://kiro.dev/docs/custom-agents/configuration-reference/
[exit-codes]: https://kiro.dev/docs/reference/exit-codes/
[effort]: https://kiro.dev/docs/models/effort/
