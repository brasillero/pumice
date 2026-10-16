# Backlog — p14.2-backend-policy

## Active

## Deferred

### #1 · Deferred · Owner check: the Windows Vulkan bundle on a real GPU
- type: Task
- area: transcription
- origin: p14.1-windows-vulkan/D1
- ticket: —
- created: 2026-10-16
- reason: needs the owner's Windows machine with a GPU; CI runners have none

**Context.** P14.1 and P14.2 build and test the Windows Vulkan bundle on CI, but no model has run on a real GPU. The checklist is in `docs/transcription.md` (Windows Vulkan preview section). The bundle is the `pumice-transcribe-windows-x64-vulkan-preview` artifact of the latest Transcription engine run on `main` (kept 7 days; re-run the workflow to get a fresh one). Unverified: speed versus the best CPU variant, transcript equality with CPU, VRAM use, behavior after a driver reset.

**To do.**
- [ ] Run the checklist (`backends --json`, `file --backend gpu` and `auto`, CPU comparison, `serve` with `pumice serve`).
- [ ] Decide whether `auto` becomes the default (`p14.1-windows-vulkan/D3` revisit condition).


## Closed
