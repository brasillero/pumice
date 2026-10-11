# Backlog — setup-wizard

## Active

### #1 · To do · Build the `pumice setup` wizard
- type: Improvement
- area: cli
- origin: conversation
- ticket: —
- created: 2026-10-07

**Context.** List installed CLIs, pick which to enable, pick a model each. "Dumb easy, frictionless, robust." Needs per-provider metadata: how to list models, whether the CLI was verified, the login check. From the 2026-10-07 audit plan (items 10–11).

**To do.**
- [ ] Design the provider metadata it needs.
- [ ] Implement and test with fake CLIs.

### #2 · To do · Publish which model IDs each provider accepts
- type: Improvement
- area: providers
- origin: conversation
- ticket: —
- created: 2026-10-11

**Context.** Users need to know what to put in `model:` (Claude `haiku`, Kiro `claude-haiku-4.5`, …). Either a table maintained by us, or not needed once the wizard lists models from each CLI (`kiro-cli chat --list-models`, `kimi provider list`). Could carry the per-model effort levels for #3.

**To do.**
- [ ] Decide between a maintained table and listing from the CLIs.

### #3 · To do · Send each model its lowest effort
- type: Improvement
- area: providers
- origin: conversation
- ticket: —
- created: 2026-10-11

**Context.** Every plugin should send each model the lowest effort it offers, and `none` when the model supports none, instead of one fixed `low` (Kiro and Codex send `low` today; GPT-5.6 models in Kiro also accept `none`). Needs a source of per-model effort levels (#2). The client's own `reasoning_effort` (Handy sends `"none"`) is ignored; the plugins' pins decide.

**To do.**
- [ ] Find or build the per-model effort source.
- [ ] Use it in every plugin.

## Deferred

## Closed
