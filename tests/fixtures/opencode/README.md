# OpenCode fixtures
#
# Documentation-derived from the OpenCode 1.18.34 bundled source (the
# `--format json` event printing in `cli/cmd/run.ts` and the part/error
# schemas in the core package), inspected on 2026-10-04 — not recorded real
# output. See `docs/research/phase2-architecture.md` §3.1.
#
# - `success.jsonl`: step_start, one completed text part, step_finish.
# - `error.jsonl`: structured `ProviderAuthError` event (not logged in).
# - `tool.jsonl`: a completed tool call (`tool_use`) and a `tool-calls`
#   finish reason; must be rejected as `UnexpectedToolActivity` even with
#   exit code 0.
