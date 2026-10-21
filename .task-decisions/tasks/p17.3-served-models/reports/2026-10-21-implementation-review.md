# P17.3 implementation and review

Implementation commit: `6edc200`, based on PR #124's merge `21bd03e`.

## Result

`pumice serve` publishes the selected real transcription model first for raw
text. With the relay enabled, it also publishes `model:provider:preset`
combinations selected by the explicit `serve.relay` settings. Provider order
comes from `relay.yml`, preset order from `serve.relay.presets`; both expansion
switches produce their Cartesian product. The immutable registry uses exact
lookup and never splits an incoming id. GET model paths decode percent escapes
once, preserving literal plus signs; multipart model fields are not decoded.

All public plans use `serve.budgets_ms`, which participates in shared upload
and connection limits even when there are no dictation flows. The existing
work permit, child lifecycle, cancellation, cleanup and raw-transcript fallback
remain shared with dictations. Response extensions retain the actual public
model and transcription model, with `flow: null`.

With the relay disabled, startup reads no relay or formatter configuration and
builds no AI runtime. Only the raw model is published. Raw dictations work;
AI dictations report `relay_disabled` before inference and their flow-list
entries report that they are unavailable. `pumice run` and relay-enabled raw
dictation stage reports retain their existing behavior.

## Review

The requested Claude implementation attempt hit its API rate limit before
editing files. Codex completed implementation, parent review and a separate
independent review. The independent reviewer approved the final working tree.

Parent review corrections preserved precise configuration reference lines,
the formatter-module requirement for generated presets, special formatter ids,
and existing run/dictation stage reports. Independent review found one P2:
AI dictation flows were incorrectly marked ready while the relay was disabled.
The final fix marks those flows unavailable without masking a service-wide
failure and includes the new status in metadata size checks. The reviewer
independently reran its focused unit and relay-off integration checks and
reported no unresolved findings.

## Validation

- `cargo test --workspace -q`: 1,251 tests passed before the final readiness
  correction, with no failures.
- After that correction, all 14 HTTP unit tests and all 15 transcription-route
  integration tests passed, including the added readiness-priority check.
- `cargo clippy --workspace --all-targets -- -D warnings`, formatting and
  `git diff --check` passed after the correction.
- Example/workflow YAML validation passed. CI's generated orchestrator
  configuration uses the new explicit settings.
- No real AI calls, model downloads or live service changes were made.

## Deployment boundary

Live settings, binaries and model locations remain unchanged until this third
implementation PR merges. The authorized migration then preserves the current
`clean` and `clean-en` choices using preset expansion, moves the two verified
models to the default per-user models folder, installs matching binaries and
native libraries, and checks both live services. The initial example keeps
both expansion switches false.
