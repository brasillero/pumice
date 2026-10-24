# P16 — Configurable effort per provider and Claude thinking

Task: `p16-provider-effort` (owner decisions D2, D4, D5). Checked on
2026-10-23 on Linux (WSL), against the installed CLIs only:

| CLI | Installed version | How it was checked |
| --- | --- | --- |
| Claude Code | `2.1.306 (Claude Code)`, native binary built 2026-10-21 (the task named 2.1.304; the machine had updated) | `claude --version`, `claude --help`, and read-only string inspection of the installed binary (`~/.local/share/claude/versions/2.1.306`) |
| Codex | `codex-cli 0.163.0` (standalone musl build) | `codex --version`, `codex --help`, `codex exec --help`, `codex app-server --help`, and read-only string inspection of the installed binary |
| Kiro | `kiro-cli 2.29.0` | `kiro-cli --version`, `kiro-cli chat --help` |

No AI call was made: no call spent quota, and nothing was logged in or
configured. Everything below about how a call behaves at run time is read
from the CLIs' help and code, not measured, unless it says otherwise.

## The mapping

`relay.yml` writes `effort: low | medium | high` on every provider entry, and
`thinking: true | false` on the `claude` entry only. Each plugin maps them:

| Pumice | Claude Code 2.1.306 | Codex 0.163.0 | Kiro 2.29.0 |
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
| Claude | `claude-haiku-4-5`, `claude-sonnet-4-5`, `claude-sonnet-4-0`, `claude-opus-4-1`, `claude-opus-4-0` and every `claude-3-…` model; each also with a `-` or `@` suffix (dated ids such as `claude-haiku-4-5-20251001`), case-insensitive, with or without `[1m]` | Claude Code 2.1.306 binary: the effort-support check returns false for exactly these names (`r.includes("claude-3-")\|\|r==="claude-opus-4-0"\|\|r==="claude-opus-4-1"\|\|r==="claude-sonnet-4-0"\|\|r==="claude-sonnet-4-5"\|\|r==="claude-haiku-4-5"`), after overrides that only apply to custom provider setups; the request builder then deletes `effort` (`if(!JS(h)){delete n.effort;return}`) without a warning. Its bundled model catalog agrees for Haiku 4.5 (`claude-haiku-4-5-20251001`: `thinking:{type:"none"}`, no effort levels). |
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
  `claude-haiku-4-5` (which does not) on Bedrock, Vertex, Foundry and
  gateway setups, and a second table maps it to
  `claude-haiku-4-5-20251001`. When it resolves to Haiku 4.5, the
  configured `effort` is silently dropped. Which one the owner's `haiku`
  entry reaches through the gateway was not checked (it needs a call).
- Spellings the table does not match (Bedrock ARNs, `us.anthropic.…`
  inference profiles) are accepted.

A model added to a CLI later is accepted until its limitation is recorded
here and added to the plugin's table.

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
allows it. A model that cannot turn thinking off keeps it (the CLI's
`Thinking can't be turned off for …`); that was true before too.

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
4. Does your Claude `haiku` entry reach Haiku 4.5 through your gateway? If
   so, its `effort` has no effect (Claude Code drops it without a warning);
   naming a model with effort, such as `claude-haiku-5-5`, would make it
   apply. One call with `--debug` (or the gateway's request log) would show
   the resolved model.
5. Kiro's `claude-haiku-4.5`, the model S2.14 tested, can no longer be
   configured, because it ignores every effort level. The shipped example
   now suggests `glm-5`, your live Kiro model.
