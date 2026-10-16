# Backlog — p14.4-macos-metal

## Active

## Deferred

### #1 · Deferred · Real-Metal check on an Apple Silicon Mac
- type: Task
- area: transcription
- origin: D1
- ticket: —
- created: 2026-10-16
- reason: needs an Apple Silicon Mac; GitHub's hosted runners have no usable GPU

**Context.** GitHub's Apple Silicon runners have no GPU acceleration, so CI proves the build, the layout and the CPU fallback, not Metal itself. A real Mac must confirm `--backend gpu` reports `metal`, the transcript matches the CPU's, and the speed difference against the best CPU run, which decides whether `auto` should pick Metal (D1).

**To do.**
- [ ] Run the Apple Silicon checklist in `docs/transcription.md` on a real Mac (ideally also one on macOS 11, the deployment floor) and record the results here.
- [ ] If Metal gives correct text and is faster, revisit D1 and D2.

### #2 · Deferred · Sign and notarize the macOS bundle with a Developer ID
- type: Improvement
- area: distribution
- origin: conversation
- ticket: —
- created: 2026-10-16
- reason: needs a paid Apple Developer account, the owner's call

**Context.** The bundle is ad-hoc signed. A copy downloaded with a browser gets the quarantine attribute, and Gatekeeper may refuse it until the user runs `xattr -dr com.apple.quarantine` on the folder (documented in `docs/transcription.md`). Signing every Mach-O with a Developer ID after the loader-path edits, then notarizing, removes that step; `bundle.json` hashes must be generated after signing.

**To do.**
- [ ] Decide whether to get a Developer ID, then sign and notarize in the release job.

## Closed
