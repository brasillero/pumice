# Backlog — p14.4-macos-metal

## Active

### #1 · To do · Real-Metal check on an Apple Silicon Mac
- type: Task
- area: transcription
- origin: D1
- ticket: —
- created: 2026-10-16

**Context.** GitHub's Apple Silicon runners have no GPU acceleration, so CI proves the build, the layout and the CPU fallback, not Metal itself. A real Mac must confirm `--backend gpu` reports `metal`, the transcript matches the CPU's, and the speed difference against the best CPU run, which decides whether `auto` should pick Metal (D1).

**To do.**
- [ ] Run the checklist in `docs/transcription.md` on an Apple Silicon Mac and record the results here.

## Deferred

## Closed
