# Backlog — p16-provider-effort

## Active

### #1 · To do · Add the common effort setting to the YAML
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
- [ ] Map the levels per provider, checked against the installed CLIs (Claude Code 2.1.304, codex-cli 0.163.0, kiro-cli).
- [ ] Remove the cheapest-settings sentence from AGENTS.md (D3).

**Acceptance criteria.**
- Startup refuses an unknown or unsupported value with the file and line.
- Fake-CLI tests show the mapped arguments for each level.

**Where in the code.**
- `crates/relay/src/providers/claude.rs` (`CHEAPEST_ENV`), `codex.rs`, `kiro.rs`
- `crates/relay/src/config/raw.rs`

### #2 · To do · Archive the Kimi and Antigravity providers
- type: Task
- area: providers
- origin: D1
- ticket: —
- created: 2026-10-20

**Context.** D1 archives both providers. The owner's live `relay.yml` already has `kimi` disabled; Antigravity is not configured there.

**To do.**
- [x] Owner: what archived means (D6: delete the plugins).
- [ ] Update docs and research notes that list them.

**Where in the code.**
- `crates/relay/src/providers/kimi.rs`, `antigravity.rs`

## Deferred

## Closed
