# P16 — Configurable effort per provider and Claude thinking

Task: `p16-provider-effort` (owner decisions D2, D4, D5). Checked on
2026-10-23 on Linux (WSL), against the installed CLIs only:

| CLI | Installed version | How it was checked |
| --- | --- | --- |
| Claude Code | `2.1.306 (Claude Code)`, native binary built 2026-10-21 (the task named 2.1.304; the machine had updated); re-checked on 2026-10-26 against `2.1.310 (Claude Code)`, the installed version since then | `claude --version`, `claude --help`, read-only string inspection of the installed binary (`~/.local/share/claude/versions/2.1.306`), and on 2026-10-24 its model-name functions extracted and run under node (no `claude` process; [Claude Code's model names](#claude-codes-model-names)); the same for 2.1.310 ([Re-check on Claude Code 2.1.310](#re-check-on-claude-code-21310)) |
| Codex | `codex-cli 0.163.0` (standalone musl build) | `codex --version`, `codex --help`, `codex exec --help`, `codex app-server --help`, and read-only string inspection of the installed binary |
| Kiro | `kiro-cli 2.29.0` | `kiro-cli --version`, `kiro-cli chat --help` |

No AI call was made: no call spent quota, and nothing was logged in or
configured. Everything below about how a call behaves at run time is read
from the CLIs' help and code, not measured, unless it says otherwise.

## The mapping

`relay.yml` writes `effort: low | medium | high` on every provider entry, and
`thinking: true | false` on the `claude` entry only. Each plugin maps them:

| Pumice | Claude Code 2.1.306 and 2.1.310 | Codex 0.163.0 | Kiro 2.29.0 |
| --- | --- | --- | --- |
| `effort: low` | `--effort low` and `CLAUDE_CODE_EFFORT_LEVEL=low` (child env and the `--settings` layer's `env`) | `-c model_reasoning_effort="low"` | `--effort low` |
| `effort: medium` | `--effort medium` and `CLAUDE_CODE_EFFORT_LEVEL=medium` | `-c model_reasoning_effort="medium"` | `--effort medium` |
| `effort: high` | `--effort high` and `CLAUDE_CODE_EFFORT_LEVEL=high` | `-c model_reasoning_effort="high"` | `--effort high` |
| `thinking: false` | `MAX_THINKING_TOKENS=0` (child env and settings layer), as before | — (key rejected) | — (key rejected) |
| `thinking: true` | `MAX_THINKING_TOKENS=` (empty) and `"alwaysThinkingEnabled": true` in the settings layer | — (key rejected) | — (key rejected) |

All three CLIs use the same three words, so every level maps to itself. Each
plugin still has its own `cli_effort` function, so a CLI that renames a level
changes one plugin.

### Evidence

**Claude Code.** `claude --help`: `--effort <level>  Effort level for the
current session (low, medium, high, xhigh, max)`. The binary's flag parser
warns and falls back to the default effort on an unknown value (`Unknown
--effort value '…' — ignoring it and using the default effort`), so a typo
there would not fail a call; Pumice's YAML check keeps that from happening.
The binary also reads `CLAUDE_CODE_EFFORT_LEVEL`, and that variable wins over
`--effort` (`CLAUDE_CODE_EFFORT_LEVEL overrides effort for this session`).
That is why the plugin pins the same level in the variable, in the child
environment and in the per-call `--settings` `env` block (a user's settings
`env` block would otherwise overwrite the child environment). This is the
same arrangement the plugin used for `low` before.

**Claude Code effort caps.** Checked on 2026-10-24 in the same 2.1.306
binary (its settings schema descriptions and the effort resolution code;
`claude --help` does not mention caps). A level can be lowered after
Pumice chose it, in three ways:

- **`maxEffortLevel`** in the user's settings (one of `low`, `medium`,
  `high`, `xhigh`, `max`). The schema says: "Anything above it (an /effort
  or /model pick, --effort, CLAUDE_CODE_EFFORT_LEVEL, a model default) is
  clamped to it, on every provider including Bedrock, Vertex and Foundry
  … across settings files the lowest value wins". It is also on the CLI's
  list of policy-restrictive settings, so managed settings can set it.
- **`modelSettings.<model>.maxEffortLevel`**, the same cap for one model:
  "Within one settings file it replaces the top-level maxEffortLevel for
  the model ("max" exempts it); across settings files the lowest applicable
  value wins". Keys match the canonical model name and its dated, `[1m]`,
  Bedrock and Vertex spellings.
- **An organization's per-model cap**: on the first-party API and through a
  gateway, the account's model list can carry a `max_effort_level` per
  model; the CLI takes the lower of it and the settings cap (`max` means no
  cap).

The code resolves a call's effort in this order: `CLAUDE_CODE_EFFORT_LEVEL`
(unless `unset` or `auto`), then the per-turn and session levels
(`--effort`), then the model's default (where an organization default
also applies), and only then clamps the result to the cap. The clamp is
silent: no warning, the call simply runs at the lower level. So both of
Pumice's pins (`--effort` and `CLAUDE_CODE_EFFORT_LEVEL`) are subject to the
cap. Pumice's per-call `--settings` layer writes no `maxEffortLevel`, and
since the lowest value wins across settings files it could not lift a cap
anyway. That is consistent with AGENTS.md (each CLI keeps its own user
configuration; Pumice inherits it): with a cap below the configured
`effort`, Claude runs at the cap, and Pumice cannot see it at startup,
because it does not read the user's Claude settings or account. The schema
also notes that an effort written into `CLAUDE_CODE_EXTRA_BODY` is not
clamped, and the request builder keeps an `effort` already present there
instead of its own; Pumice does not set that variable, so an inherited one
would replace the configured level. None of this was run.

**Codex.** `codex exec --help` documents `-c key=value` overrides of
`~/.codex/config.toml`. The binary's config schema contains
`model_reasoning_effort` (next to `plan_mode_reasoning_effort` and
`model_reasoning_summary`), and its effort names include `minimal`, `low`,
`medium`, `high`, `xhigh`, `max` and `ultra`. Which levels a model accepts
comes from the model catalog, not from the CLI; `low`, `medium` and `high`
are the common ones. The plugin keeps the value as the last `-c` before the
stdin marker `-`, as before.

**Kiro.** `kiro-cli chat --help`: `--effort <EFFORT>  Initial effort level
(e.g. low, medium, high, xhigh, max)`. Levels depend on the model (S2.14,
[Effort](S2.14-kiro-plugin.md#effort)): a model without reasoning, such as
`claude-haiku-4.5`, prints `[warn] failed to set effort '<level>': model does
not support additional fields` on stderr and runs. That was measured for
`low` in S2.14; the same path is expected for `medium` and `high` but was not
run. Since 2026-10-24 Pumice refuses such a model at startup (see
[Models that cannot honour effort](#models-that-cannot-honour-effort)).

## Models that cannot honour effort

D2 says a value a provider cannot honour is refused at startup with the
file and line. Within `low`, `medium` and `high`, the installed CLIs show no
model that takes some levels but not others; what they do show is models
that take no effort at all. For those, every level is ignored, so each
plugin keeps a small table of them and refuses the entry's `effort` at its
line (on a disabled entry too, like every other check), with the model, the
level and the evidence in the message:

```text
relay.yml:10:13: providers entry "kiro".effort: model "claude-haiku-4.5" has no reasoning effort (kiro-cli 2.29.0 ignores --effort for it: "model does not support additional fields"), so "high" cannot be honoured, nor can any other level; choose a model with reasoning effort
```

Since `effort` is required, such a model cannot be configured for that
provider at all.

| Provider | Models refused | Evidence |
| --- | --- | --- |
| Claude | Every spelling Claude Code canonicalizes to `claude-haiku-4-5`, `claude-sonnet-4-5`, `claude-sonnet-4-0`, `claude-opus-4-1`, `claude-opus-4-0` or a `claude-3-…` name: `claude-sonnet-4`, `claude-sonnet-4-20250514`, `claude-opus-4`, `claude-opus-4@20250514`, `claude-haiku-4-5-20251001`, `us.anthropic.claude-haiku-4-5-20251001-v1:0`, `Claude-Sonnet-4-5[1m]`, … (see [Claude Code's model names](#claude-codes-model-names)), except the four legacy Opus spellings Claude Code remaps at run time | Claude Code 2.1.306 binary: the effort check (`JS`) canonicalizes the model (`r=qe(e)`) and returns false for exactly these names (`r.includes("claude-3-")\|\|r==="claude-opus-4-0"\|\|r==="claude-opus-4-1"\|\|r==="claude-sonnet-4-0"\|\|r==="claude-sonnet-4-5"\|\|r==="claude-haiku-4-5"`), after overrides that only apply to custom setups (see below); the request builder then deletes `effort` (`if(!JS(h)){delete n.effort;return}`) without a warning. Its bundled model catalog agrees: none of these entries has the `effort` capability. `claude-opus-4-5` lacks that capability too but is not in this list, so it is accepted: its effort depends on the API provider (see below). |
| Kiro | `claude-haiku-4.5` (verbatim, as `--list-models` prints it) | S2.14 live calls 1 and 3 ([Effort](S2.14-kiro-plugin.md#effort)): `[warn] failed to set effort 'low': model does not support additional fields`; the recorded run (`crates/relay/tests/fixtures/kiro/success.jsonl`, which is live call 1: same answer and the same 0.0059 credits) says `"reasoning":{"support":"unavailable","effortLevels":[]}` in every `metadata` event (line 2 onwards). |
| Codex | none | `codex debug models --bundled` (0.163.0, no network): every model in the bundled catalog (`gpt-6-astra`, `gpt-6.1-sol`, `gpt-6-sol`, `gpt-6-luna`, `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna`, `gpt-5.5` and the hidden ones) lists `low`, `medium` and `high` in `supported_reasoning_levels`. |

**Every other model is accepted**, because nothing recorded says it cannot
honour the level:

- Kiro's levels come from its service, per model, and only show up in a
  run's `metadata` event; `kiro-cli chat --list-models --format json` lists
  ids and credit rates without effort, and the 2.29.0 binary holds no
  per-model table. Models such as `glm-5` (the owner's live Kiro model) are
  unknown, not unsupported.
- Claude Code resolves aliases (`haiku`, `sonnet`, `opus`, …) at run time,
  from `ANTHROPIC_DEFAULT_*_MODEL`, the account and the API provider, so
  Pumice cannot tell at startup which model an alias means. **This matters
  for `haiku`:** the 2.1.306 binary's alias tables map it to
  `claude-haiku-5-5` (which takes effort) by default but to
  `claude-haiku-4-5` (which does not) on Bedrock, Vertex, Foundry, Mantle,
  Claude Platform on AWS and on Google Cloud and in Claude Code's gateway
  mode, and a second table maps it to `claude-haiku-4-5-20251001`. When it
  resolves to Haiku 4.5, the configured `effort` is silently dropped. A
  proxy set only through `ANTHROPIC_BASE_URL` is not gateway mode, so by
  these tables the owner's `haiku` entry reaches Haiku 5.5
  ([Which setups count as a gateway](#which-setups-count-as-a-gateway));
  this was read from the binary, not run.
- `claude-opus-4-5` (and its spellings, such as `claude-opus-4-5-20251101`)
  is a known model whose effort depends on the API provider. Its catalog
  entry has no `effort` capability, but the effort check does not list it,
  so the answer comes from the last step, `AK(vd(e))` (`eq(Od(e))` in
  2.1.310): effort is sent on first-party, Claude Platform on AWS and on
  Google Cloud, Foundry and Mantle setups, and dropped without a warning on
  Bedrock, Vertex and in Claude Code's gateway mode. It is the only catalog
  model that reaches that step; names the tables do not know (such as
  `claude-opus-4-10`) reach it too. Pumice cannot tell the provider at
  startup ([Which setups count as a gateway](#which-setups-count-as-a-gateway)),
  so it accepts the model.
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
  reach, as for `opus`: see
  [Models that cannot turn thinking off](#models-that-cannot-turn-thinking-off).

A model added to a CLI later is accepted until its limitation is recorded
here and added to the plugin's table.

### Claude Code's model names

Claude Code compares a model with its tables only after canonicalizing its
name, so `claude-opus-4` and `claude-opus-4-20250514` both count as
`claude-opus-4-0`. Pumice reproduces that step (`crates/relay/src/providers/claude/models.rs`);
the round 2 matcher, which only compared the canonical names with a `-` or
`@` suffix, accepted `claude-sonnet-4`, `claude-sonnet-4-20250514`,
`claude-opus-4` and `claude-opus-4-20250514` although Claude Code drops
their effort (Codex review round 2).

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

**How the port was checked.** These functions and the catalog were
extracted verbatim from the chunks and run under node 24.21.0 in a
`vm` context (nothing else from the binary, no network, `claude` never
started). Their output for 94,263 generated spellings (5 families plus a
dummy, majors 3/4/5/10/41/100, minors none/0/1/5/6/7/8/10/50/100, 21
suffixes, 10 prefixes, both layouts, aliases, odd cases and every catalog
provider id in upper and lower case) was compared with the Rust port's: no
difference. The unit test `canonicalizes_like_claude_code` keeps 43 of
those cases, for example:

| Written | Canonical (2.1.306) |
| --- | --- |
| `claude-sonnet-4`, `claude-sonnet-4-20250514`, `claude-sonnet-4@20250514`, `claude-sonnet-4-latest`, `eu.anthropic.claude-sonnet-4-20250514-v1:0` | `claude-sonnet-4-0` |
| `claude-opus-4`, `claude-opus-4-20250514`, `claude-opus-4x`, `my-claude-opus-4`, `xx.anthropic.claude-opus-4` | `claude-opus-4-0` |
| `claude-opus-4-10-foo`, `claude-opus-4-1-foo` | `claude-opus-4-1` |
| `claude-opus-4-10`, `claude-haiku-4-50`, `claude-sonnet-4-1`, `claude-haiku-4` | unchanged (versions of their own) |
| `claude-fable-5-0`, `claude-sonnet-5-0` | `claude-fable-5`, `claude-sonnet-5` |
| `haiku`, `opusplan`, `glm-5`, `claude-haiku-4.5` | unchanged |

The round 2 review said the binary canonicalizes `claude-opus-4-10` to
`claude-opus-4-1` and `claude-haiku-4-50` to `claude-haiku-4-5`. Running
the whole `NU` does not: `Ac` parses both as versions of their own (minor
10 and 50), no catalog entry matches, and `NU` returns the parsed base
before the `includes` chain is reached. Only with a suffix `_2o` does not
know (`claude-opus-4-10-foo`) does the chain apply, and then it gives
`claude-opus-4-1`. So both stay accepted, now with this reason.

**What Pumice cannot see.** Claude Code applies, before these tables, things
that only exist at run time; Pumice uses the baked rules alone:

- the `--model` resolution (`Dt`): aliases and the legacy Opus remap
  above;
- the user's `modelOverrides` setting and the account's model knowledge
  (`uv`), which can map a name before canonicalization;
- `CLAUDE_CODE_MODEL_CAPABILITIES`, and on Bedrock, Vertex, Foundry and
  Mantle the `ANTHROPIC_DEFAULT_*_MODEL` / `…_SUPPORTED_CAPABILITIES`
  pairs, which override the capability answer;
- per-model effort levels the account's model knowledge serves (`iBr`), and
  a latch set after the API refuses effort for a model;
- for `claude-opus-4-5` and for a model the tables do not know, effort is
  sent on first-party, Claude Platform on AWS and on Google Cloud, Foundry
  and Mantle setups and dropped elsewhere (Bedrock, Vertex, gateway mode;
  `AK(vd(e))`, where `vd` keeps Bedrock's provider unless Mantle is enabled
  and serves the model).

### Which setups count as a gateway

Claude Code's provider (`xe()` in 2.1.306, `Ae()` in 2.1.310) is
`"gateway"` when any of three run-time slots is set, before any
`CLAUDE_CODE_USE_<cloud>` variable is looked at:

```js
function xe(){if(Wr()||knt()||Ant())return"gateway";return a.CLAUDE_CODE_USE_BEDROCK?"bedrock":…:"firstParty"}
```

- `Wr()`, the gateway credential: set when `CLAUDE_CODE_USE_GATEWAY` is set
  together with `ANTHROPIC_BASE_URL` and a token (`ANTHROPIC_AUTH_TOKEN` or a
  gateway descriptor), or from a stored `enterpriseGateway` login (Claude
  Code's gateway sign-in). With `CLAUDE_CODE_USE_GATEWAY` but no base URL or
  token it warns and ignores the variable.
- `knt()` (`Rit()` in 2.1.310): the process is `claude gateway --config
  <file>`, Claude Code's own gateway server.
- `Ant()` (`Pit()` in 2.1.310): managed settings require a gateway login
  (`forceLoginMethod: "gateway"` or `forceLoginGatewayUrl`), no
  `CLAUDE_CODE_USE_<cloud>` variable is set and the provider is not
  managed by a host (`CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST`).

`ANTHROPIC_BASE_URL` alone does not make a gateway: the provider stays
`"firstParty"`. The binary says so itself: its gateway model discovery
(`CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY`) runs only when the provider
is `"firstParty"` and `ANTHROPIC_BASE_URL` is set, and otherwise logs
`skipped: provider is <provider>`. The alias tables pick their
`per_provider` entry from the same provider.

The owner routes Claude Code through CLIProxyAPI with
`ANTHROPIC_BASE_URL`. On this development machine `~/.claude/settings.json`
sets `ANTHROPIC_BASE_URL`, `ANTHROPIC_AUTH_TOKEN` and
`CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY` in its `env` block and not
`CLAUDE_CODE_USE_GATEWAY` (only the key names were read), and there is no
`/etc/claude-code/managed-settings.json`. A stored gateway login was not
checked, because it sits with the credentials. So by the binary's rules
such a setup counts as first-party: `haiku`, `sonnet` and `opus` mean
Haiku 5.5, Sonnet 5.5 and Opus 5.5, and `claude-opus-4-5` gets its effort.
The live services run on another host with their own environment, which
was not checked. None of this was run.

## Claude thinking

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
the thinking parameter from the request altogether, whatever the mode. The
plugin clears it (empty value) in both cases, as it did before.

**`thinking: false`** is the old behaviour, byte for byte:
`MAX_THINKING_TOKENS=0`, so the request asks for thinking off where the model
allows it. A model that cannot turn thinking off is refused (see
[Models that cannot turn thinking off](#models-that-cannot-turn-thinking-off)).

**`thinking: true`** sets `MAX_THINKING_TOKENS` to an empty value (clearing a
budget the user's environment or settings might set; the CLI treats empty as
unset) and adds `"alwaysThinkingEnabled": true` to the per-call settings
layer (overriding a user's `false`). Pumice sets no budget: the CLI does what
it does by default for the model.

- On models with adaptive thinking (the newer ones; the binary lists
  `claude-3-*`, `claude-opus-4-0`/`4-1`/`4-5`, `claude-sonnet-4-0`/`4-5` and
  `claude-haiku-4-5` as models without it), the request carries
  `thinking: {type: "adaptive"}` with no budget: the model decides how much
  to think, guided by the effort.
- On models without adaptive thinking, including `claude-haiku-4-5`, the
  request carries `thinking: {type: "enabled", budget_tokens: N}`, where `N`
  is the model's output upper limit minus one, clamped to at least 1024 and
  below the request's maximum output tokens. In practice the budget is the
  request's whole output allowance. It is a cap, not an amount the model
  must spend, but it is the largest one the CLI allows.

So no budget variable is needed to allow thinking, and its default on older
models is the maximum. Setting a smaller budget would need a new setting
(`MAX_THINKING_TOKENS=<n>` only changes models without adaptive thinking);
that is a question for the owner, not part of D5.

**Effort and thinking together.** The binary has an error for `effort` levels
a model accepts only with thinking on (`Effort '…' isn't available with
thinking turned off on this model · … lower effort to High, or turn thinking
back on`). Its hint offers `high` as the fallback, so `low`, `medium` and
`high` with `thinking: false` should not hit it; this was not run.

### Models that cannot turn thinking off

With `MAX_THINKING_TOKENS=0` the session asks for `{type:"disabled"}`
thinking, but on some models Claude Code sends adaptive thinking anyway, so
`thinking: false` cannot be honoured. In the 2.1.306 binary (chunks at
240567143 and 246787320):

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
exactly six models:

`claude-haiku-5-5`, `claude-sonnet-5-5`, `claude-opus-5-5`,
`claude-fable-5`, `claude-fable-5-1`, `claude-mythos-5-1`.

Pumice refuses `thinking: false` for every spelling that canonicalizes to
one of them (the same port as for effort), on a disabled entry too, at the
`thinking` key:

```text
relay.yml:90:15: providers entry "claude".thinking: model "claude-haiku-5-5" cannot turn thinking off (Claude Code 2.1.310 sends adaptive thinking for it even with thinking off), so "false" cannot be honoured; write "thinking: true" or choose a model that can turn thinking off
```

An effort refusal is reported first when both apply (no model is in both
lists today). Aliases and the four remapped legacy Opus spellings are
accepted, as for effort, although on a first-party setup (which includes
a proxy set only through `ANTHROPIC_BASE_URL`,
[Which setups count as a gateway](#which-setups-count-as-a-gateway)) all
of them reach a model that keeps thinking on. From the catalog's alias
tables, unless `ANTHROPIC_DEFAULT_*_MODEL` or the account's served alias
targets say otherwise:

| Written | First-party | Gateway mode | Cloud providers |
| --- | --- | --- | --- |
| `haiku` | `claude-haiku-5-5`: thinking always on | `claude-haiku-4-5`: no effort | `claude-haiku-4-5`: no effort |
| `sonnet` | `claude-sonnet-5-5`: thinking always on | `claude-sonnet-4-6` | `claude-sonnet-4-5` (no effort) on Bedrock, Vertex, Foundry, Mantle; `claude-sonnet-4-6` on Claude Platform on AWS; `claude-sonnet-5-5` (thinking always on) on Claude Platform on Google Cloud |
| `opus` | `claude-opus-5-5`: thinking always on | `claude-opus-4-7` | `claude-opus-5-5` (thinking always on) except Foundry (`claude-opus-4-6`) |
| `claude-opus-4-0`, `claude-opus-4-1`, `claude-opus-4-20250514`, `claude-opus-4-1-20250805` | remapped to the default Opus, `claude-opus-5-5`: thinking always on | remapped to `claude-opus-4-7` | remapped to `claude-opus-5-5` (thinking always on) on Claude Platform on AWS and on Google Cloud; elsewhere not remapped: no effort |

**This matters for the shipped example:** `relay.example.yml`, which is
also the first-start `relay.yml`, writes `model: haiku` with
`thinking: false`. On a first-party setup `haiku` resolves to
`claude-haiku-5-5`, so thinking stays on whatever the file says; in
gateway mode or on a cloud provider (`claude-haiku-4-5`) thinking is off
but `effort` is dropped. The example's values are the owner's to change
(question 6); for now it carries a comment saying this.

## Settings that stay pinned

Only effort and Claude thinking became configurable. Every call still pins:

- Claude: `--settings` `fastMode: false`, `CLAUDE_CODE_DISABLE_FAST_MODE=1`,
  `ANTHROPIC_BEDROCK_SERVICE_TIER=default`, `CLAUDE_CODE_DISABLE_THINKING`
  cleared.
- Codex: `service_tier="default"`, `features.fast_mode=false`,
  `model_reasoning_summary="none"`.
- Kiro: no further setting exists beyond `--effort`.

## Configuration rules

- `effort` is required on every entry, enabled or not
  (p17-serve-model-names/D3). A missing key is reported at the entry's first
  key; `null` and any other value (`max`, `xhigh`, `minimal`, `Low`…) fail at
  the value with the three levels listed.
- **A disabled entry may not write `effort: null`.** `model: null` exists
  because no model is valid for every account, so a disabled entry may have
  nothing sensible to write. Every provider accepts every level (only the
  models above are refused, whatever the level), so `low` is always a
  valid value, and keeping `null` out leaves one rule for the whole file:
  only `model` can be `null`, and only while disabled.
- `thinking` is required on `claude` (also disabled) and must be `true` or
  `false`. On any other entry the key, whatever its value, fails at its line:
  those CLIs reason through effort alone (D5).
- The shipped `relay.example.yml`, which is also the first-start `relay.yml`,
  writes `effort: low` on every entry and `thinking: false` on `claude`:
  the behaviour before this change.

## Re-check on Claude Code 2.1.310

On 2026-10-26 the installed Claude Code was `2.1.310 (Claude Code)`
(`~/.local/share/claude/versions/2.1.310`, `sha256 b7367643…bb47df56`).
It was checked the same way as 2.1.306: its 2,552 `// @bun @bytecode`
chunks were read in place, the relevant functions extracted verbatim and
run under node 24.21.0 in a `vm` context (no `claude` process, no
network). Everything above holds for 2.1.310; the minifier renamed most
functions:

| 2.1.306 | 2.1.310 | Result |
| --- | --- | --- |
| baked catalog `lGr` | `G5r` | byte-identical (`sha256 b46409c9…1123c95` for both extracts) |
| `qe` → `Iv` → `sw` → `NU` | `Xe` → `mk` → `_b` → `XB` | same code modulo names |
| `Ac`, `u`, `_2o`, `xL`, `E2e`, `QSt`, `LU`, `Mv`, `qv`, `pye` | `Tc`, `u`, `W9o`, `H0`, `jqe`, `aTt`, `JB`, `pk`, `Pk`, `F9` | same code modulo names; the 94,263 spellings of the 2.1.306 corpus give the same canonical name under 2.1.310's functions (0 differences), so the Rust port still matches |
| effort check `JS` | `Tw` | same code modulo names, same five names and `claude-3-`; the request builder still runs `if(!Tw(h)){delete n.effort;return}` |
| `AK` / `vd` / `xe` / `Iu` | `eq` / `Od` / `Ae` / `bu` | same code modulo names |
| thinking checks `u7e` / `NHe` | `pnt` / `C0e` | same code modulo names, same list of older models; `alwaysOn:pnt(e)` still feeds the disabled-to-adaptive substitution (`Fni`, was `D6s`) |
| legacy Opus remap list `WU` | `oH` | the same four ids |
| `--model` resolution `Dt` | `$t` | same, except the name it tests: `h=s?Mt(r).trim():r` became `h=Ot(r).trim()`. `Ot` is `Mt` renamed (`e.replace(/\[1m\]$/i,"")`), and without a `[1m]` suffix it returns the already trimmed, lowercased name, so the result is the same |

`claude --help` still lists `--effort <level>` with the same five levels,
and the binary still carries the strings quoted above (`Unknown --effort
value`, `CLAUDE_CODE_EFFORT_LEVEL overrides effort for this session`, the
effort-with-thinking-off error). Pumice's messages and tests now name
2.1.310; the excerpts above keep 2.1.306's names, where they were read.

## Questions for the owner

1. A live check of `medium`, `high` and `thinking: true` on your models
   (Claude `haiku`, Codex, Kiro) costs a few calls each; none was made here.
2. With `thinking: true` on a model without adaptive thinking (such as Haiku
   4.5), Claude Code allows thinking up to the request's whole output budget.
   Do you want a budget setting, or is the CLI's default fine?
3. AGENTS.md still names Kimi in two places this task was not authorized to
   change: the agent list in the first paragraph ("Claude Code, Codex,
   OpenCode, Kimi and others", about coding agents) and the Input bullet
   ("the CLI refuses an empty one (Codex, Kimi)"). Only Codex needs the
   neutral system prompt now.
4. Does your Claude `haiku` entry reach Haiku 4.5 or Haiku 5.5 through
   CLIProxyAPI? Read from the binary, a proxy set only through
   `ANTHROPIC_BASE_URL` counts as first-party, so `haiku` should be Haiku
   5.5 (effort applies, thinking cannot be turned off); Haiku 4.5 would
   mean the live host's setup is gateway mode or a cloud provider, and then
   `effort` has no effect. One call with `--debug` (or the proxy's request
   log) would show the resolved model.
5. Kiro's `claude-haiku-4.5`, the model S2.14 tested, can no longer be
   configured, because it ignores every effort level. The shipped example
   now suggests `glm-5`, your live Kiro model.
6. The shipped `relay.example.yml` (also the first-start `relay.yml`) writes
   `model: haiku` with `thinking: false`. On a first-party setup, which
   includes a proxy set only through `ANTHROPIC_BASE_URL`, `haiku` is Haiku
   5.5, which keeps thinking on regardless; in gateway mode or on a cloud
   provider it is Haiku 4.5, which drops `effort`. The same holds for
   `opus` and `sonnet` (Opus 5.5 and Sonnet 5.5 on first-party) and for the
   four legacy Opus ids Claude Code remaps to the default Opus
   (`claude-opus-4-0`, `claude-opus-4-1`, `claude-opus-4-20250514`,
   `claude-opus-4-1-20250805`): Pumice accepts all of them with
   `thinking: false`, and on first-party thinking stays on (see the table
   in [Models that cannot turn thinking off](#models-that-cannot-turn-thinking-off)).
   Keep `haiku` with `thinking: false`, or switch the example to
   `thinking: true` or to a pinned model? And should aliases with
   `thinking: false` stay accepted?
