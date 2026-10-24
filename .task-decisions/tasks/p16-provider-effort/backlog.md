# Backlog — p16-provider-effort

## Active

### #1 · In progress · Add the common effort setting to the YAML
- type: Improvement
- area: config
- origin: D2
- ticket: —
- created: 2026-10-20

**Context.** Each plugin today hard-codes the lowest effort (Claude also forces thinking off with `MAX_THINKING_TOKENS=0`). D2 adds `low`, `medium`, `high`, mapped per provider. Two questions are open for the owner.

**To do.**
- [x] Owner: where the setting lives (D4: per provider in `relay.yml`).
- [x] Owner: thinking switch (D5: separate `thinking` key on `claude`).
- [x] Owner: the default when omitted (none: it must be written, `p17-serve-model-names/D3`).
- [x] Map the levels per provider, checked against the installed CLIs (Claude Code 2.1.306, codex-cli 0.163.0, kiro-cli 2.29.0; `docs/research/P16-provider-effort.md`): implemented in `2ac62d5`.
- [x] Claude `thinking` (D5): `false` keeps `MAX_THINKING_TOKENS=0`, `true` lets Claude Code use its own thinking mode and budget (`2ac62d5`).
- [x] Remove the cheapest-settings sentence from AGENTS.md (D3): replaced with the owner's wording in `7b4c5e2`.
- [x] Refuse models the CLI is known to run without effort at the `effort` key (Codex review round 1): Claude Code's `claude-haiku-4-5`, `claude-sonnet-4-5`/`4-0`, `claude-opus-4-1`/`4-0`, `claude-3-…`; Kiro's `claude-haiku-4.5`; none for Codex. Aliases and unknown models are accepted (`d45280f`, evidence in `docs/research/P16-provider-effort.md`).
- [x] Document Claude Code's effort caps (`maxEffortLevel`, per-model and organization caps) and how they clamp the mapped level (`03f8b61`).

**Acceptance criteria.**
- Startup refuses an unknown or unsupported value with the file and line.
- Fake-CLI tests show the mapped arguments for each level.

**Where in the code.**
- `crates/relay/src/providers/claude.rs` (`CHEAPEST_ENV`), `codex.rs`, `kiro.rs`
- `crates/relay/src/config/raw.rs`

### #2 · In progress · Archive the Kimi and Antigravity providers
- type: Task
- area: providers
- origin: D1
- ticket: —
- created: 2026-10-20

**Context.** D1 archives both providers. The owner's live `relay.yml` already has `kimi` disabled; Antigravity is not configured there.

**To do.**
- [x] Owner: what archived means (D6: delete the plugins).
- [x] Delete the plugins, tests, fixtures and contract cases; a listed `kimi` or `antigravity` fails with "provider removed" at its line (`d34dd8f`).
- [x] Update docs and research notes that list them; the research notes stay as history with a removal note (`3379174`).

**Where in the code.**
- `crates/relay/src/providers/kimi.rs`, `antigravity.rs`

### #3 · To do · Owner: live check of the new effort levels and Claude thinking
- type: Task
- area: providers
- origin: #1
- ticket: —
- created: 2026-10-23

**Context.** The mapping was checked against the installed CLIs' help and code only; no AI call was made (AGENTS.md: real calls cost quota). `medium`, `high` and `thinking: true` have never run through Pumice. Claude Code's code warns that some efforts need thinking on, with `high` as its fallback, so `high` with `thinking: false` should work but is unverified (`docs/research/P16-provider-effort.md`).

**To do.**
- [ ] Owner: a few calls per provider with `medium` and `high`, and Claude Haiku with `thinking: true`, on a temporary config.
- [ ] Owner: check which model the `haiku` alias reaches through the gateway. Claude Code 2.1.306 maps it to `claude-haiku-5-5` by default but to `claude-haiku-4-5` on gateway and cloud providers, and it drops effort for Haiku 4.5 without a warning (read from the binary, not run).

**Acceptance criteria.**
- Each combination answers, or its limitation is recorded in the research note.

### #4 · To do · Owner: decide whether Claude thinking needs a budget setting
- type: Alignment
- area: config
- origin: D5
- ticket: —
- created: 2026-10-23

**Context.** With `thinking: true`, Pumice sets no budget. On models without adaptive thinking (Claude Code 2.1.306 lists `claude-haiku-4-5`, `claude-sonnet-4-5`, `claude-opus-4-5` and older), the CLI then allows thinking up to the request's whole output budget; newer models use adaptive thinking, with no budget. A smaller cap would be a new setting (`MAX_THINKING_TOKENS=<n>`, which only affects the older models).

**To do.**
- [ ] Owner: keep the CLI's default, or add a budget key to the `claude` entry.

**Where in the code.**
- `crates/relay/src/providers/claude.rs` (`pinned_env`, `settings_json`)

### #5 · To do · Owner: drop the remaining Kimi mentions from AGENTS.md
- type: Alignment
- area: docs
- origin: D6
- ticket: —
- created: 2026-10-23

**Context.** This task changed only the two AGENTS.md sentences the owner authorized. AGENTS.md still says "Claude Code, Codex, OpenCode, Kimi and others" (coding agents, first paragraph) and, in the Input bullet, "the CLI refuses an empty one (Codex, Kimi)"; since D6 only Codex needs the neutral system prompt.

**To do.**
- [ ] Owner: authorize "(Codex)" in the Input bullet, and say whether the coding-agent list changes.

**Where in the code.**
- `AGENTS.md` (first paragraph; Provider plugins → Input)

### #6 · To do · Complete the owner's live relay.yml before deploying
- type: Task
- area: config
- origin: #1
- ticket: —
- created: 2026-10-23

**Context.** The new binaries refuse a `relay.yml` whose provider entries lack `effort` (or `thinking` on `claude`), and one that still lists `kimi` (the live file has `kimi` disabled, D1). Both live services read the owner's per-user `relay.yml`. This task never touched `~/.config/pumice`.

**To do.**
- [ ] Back up `relay.yml`.
- [ ] Delete the `kimi` entry (and any `antigravity` one).
- [ ] Add `effort: low` to every remaining entry and `thinking: false` to `claude` (today's behaviour), or the levels the owner picks.
- [ ] Validate with the candidate binary's `pumice relay check-config`, then restart both services and check both ports.

**Acceptance criteria.**
- Both services start with the new binaries and answer as before.

## Deferred

## Closed
