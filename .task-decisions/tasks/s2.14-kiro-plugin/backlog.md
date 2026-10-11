# Backlog — s2.14-kiro-plugin

## Active

## Deferred

### #1 · Deferred · Decide whether the plugin deletes Kiro's saved session
- type: Alignment
- area: providers
- origin: conversation
- ticket: —
- created: 2026-10-10
- reason: waiting for the owner's decision

**Context.** Kiro V2 saves every run, dictation included, under the user's own `~/.kiro/sessions/cli/`. Pumice does not touch `~/.kiro`. Should the plugin delete its own session after each run? See `docs/research/S2.14-kiro-plugin.md`. Was the BACKLOG item "Kiro session history".

**To do.**
- [ ] Owner decides.

### #2 · Deferred · Fail at once when Kiro is logged out
- type: Improvement
- area: providers
- origin: conversation
- ticket: —
- created: 2026-10-11
- reason: when the owner schedules provider work

**Context.** A logged-out `kiro-cli` prints its device-code login prompt and then waits, even with `--no-interactive`. The plugin recognizes the prompt (`NotLoggedIn`) only after the CLI exits, so the request runs out the provider timeout (504). Was the BACKLOG item "Kiro logged-out detection".

**To do.**
- [ ] Watch stdout as it streams and fail with a clear reason when the prompt appears.

**Where in the code.**
- `src/providers/kiro.rs` (`DEVICE_LOGIN_PROMPT`)

### #3 · Deferred · Kiro errors are not classified
- type: Tech debt
- area: providers
- origin: conversation
- ticket: —
- created: 2026-10-11
- reason: until Kiro reports structured errors

**Context.** Kiro's `runError` carries only a free-text message, so login, quota and rate-limit failures all become HTTP 502. Was a BACKLOG known limitation.

**To do.**
- [ ] Classify if Kiro adds structured error fields.

### #4 · Deferred · Kiro's V2 engine is pinned
- type: Tech debt
- area: providers
- origin: conversation
- ticket: —
- created: 2026-10-11
- reason: until Kiro CLI 3.0 drops V2

**Context.** The plugin pins `--agent-engine v2`, which `--help` calls "the pre-3.0 default". If Kiro CLI 3.0 drops V2, the plugin needs a V3 recipe. Was a BACKLOG known limitation.

**To do.**
- [ ] Write a V3 recipe when V2 goes away.

### #5 · Deferred · Kiro's built-in instructions can override the app's prompt
- type: Tech debt
- area: providers
- origin: conversation
- ticket: —
- created: 2026-10-11
- reason: until Kiro offers a way to replace its base instructions

**Context.** Kiro's built-in assistant instructions sit under the client's prompt; whether the client's prompt wins depends on the model. With Handy's request, `claude-haiku-4.5` answered the dictation instead of formatting it (2026-10-10); `glm-5` formatted and translated it correctly (owner test, 2026-10-11). Was a BACKLOG known limitation.

**To do.**
- [ ] Recheck if Kiro adds a way to replace its base instructions.

### #6 · Deferred · Kiro sends --effort low even where a lower level exists
- type: Tech debt
- area: providers
- origin: D3
- ticket: —
- created: 2026-10-11
- reason: accepted as debt in D3; until per-model effort levels exist

**Context.** Accepted as debt in D3. GPT-5.6 models also offer `none`, which Claude models do not accept; models without effort levels print a warning and run normally. Was the BACKLOG limitation "Kiro effort".

**To do.**
- [ ] Use per-model lowest effort once `setup-wizard#3` provides the levels.

## Closed
