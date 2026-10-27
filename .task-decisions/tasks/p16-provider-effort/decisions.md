# Decisions — p16-provider-effort

### D1 · Active · Kimi and Antigravity providers are archived
- date: 2026-10-20
- decided by: brasillero
- origin: conversation
- area: providers
- revisit if: the owner wants Kimi or Antigravity for dictation again

**Decision.** The `kimi` and `antigravity` providers are archived: they are not maintained or offered for use.

**Why.** The owner does not use them and judges them not mature enough.

**Rejected.** Keeping them maintained alongside Claude, Codex and Kiro: extra work for providers nobody uses.

**Consequences.** What "archived" means in the code (removed, kept but rejected in config, or moved aside) is still to be chosen; tracked in #2.

### D2 · Active · Reasoning effort is configured on one common scale: low, medium, high
- date: 2026-10-20
- decided by: brasillero
- origin: conversation
- area: config
- revisit if: —

**Decision.** Pumice's YAML offers one effort setting with three values, `low`, `medium` and `high`. Each provider plugin maps them to its own CLI setting (for example Claude `--effort`, Codex `model_reasoning_effort`, Kiro `--effort`).

**Why.** One way to configure every provider, instead of each CLI's own levels and names. A test on 2026-10-20 showed Claude Haiku at the fixed lowest settings ignored the list rules of the owner's `clean-en` prompt in 3 of 4 runs, while raising effort or allowing thinking produced the lists.

**Rejected.** Passing each CLI's own levels through verbatim (`xhigh`, `max`, `minimal`…): no common meaning across providers.

**Consequences.** Values a provider cannot honour are refused at startup with the file and line.

### D3 · Active · The owner sets provider effort defaults case by case
- date: 2026-10-20
- decided by: brasillero
- origin: conversation
- area: process
- revisit if: —

**Decision.** The AGENTS.md rule that every plugin always uses the cheapest CLI settings is dropped; the owner defines the defaults on demand.

**Why.** Effort becomes a user setting (D2); a fixed "always cheapest" rule contradicts it.

**Rejected.** —

**Consequences.** The owner authorized removing that sentence from AGENTS.md in the implementing pull request.

### D4 · Active · Effort is set per provider in `relay.yml`
- date: 2026-10-23
- decided by: brasillero
- origin: #1
- area: config
- revisit if: —

**Decision.** Each provider entry in `relay.yml` has an `effort` key (`low`, `medium` or `high`, D2). It applies to every call that provider makes, through the relay on 7567 and through the orchestrator alike. Orchestrator flows and `pumice serve` combinations do not override it.

**Why.** The owner chose one place to configure it.

**Rejected.** Per flow in `orchestrator.yml`; both, with the flow overriding the provider.

**Consequences.** Like every setting, `effort` must be written (`p17-serve-model-names/D3`), so there is no built-in default.

### D5 · Active · Claude thinking has its own switch
- date: 2026-10-23
- decided by: brasillero
- origin: #1
- area: config
- revisit if: —

**Decision.** The `claude` provider entry has a `thinking` key (`true` or `false`) next to `effort`. Other providers have no such key and reject it, because their CLIs reason through effort alone.

**Why.** The 2026-10-20 test showed Claude Haiku at low effort with thinking allowed followed the list rules, so the two settings are worth controlling separately.

**Rejected.** Deriving thinking from effort (`low` = off, `medium`/`high` = on).

### D6 · Active · Archived providers are deleted from the code
- date: 2026-10-23
- decided by: brasillero
- origin: #2
- area: providers
- revisit if: the owner wants Kimi or Antigravity again (git history keeps them)

**Decision.** The `kimi` and `antigravity` plugins, their tests, fixtures and docs are removed. A `relay.yml` that still lists either provider fails at startup with a clear "provider removed" error and its line.

**Why.** D1 archives them; keeping unmaintained code costs work.

**Rejected.** Keeping the code but refusing the providers in config.
