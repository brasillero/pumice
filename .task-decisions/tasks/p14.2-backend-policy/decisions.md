# Decisions — p14.2-backend-policy

### D1 · Active · Only failures that make the engine unsafe to reuse retire a GPU process
- date: 2026-10-16
- decided by: claude (orchestrator, owner-delegated 2026-10-16)
- origin: p14.1-windows-vulkan/D4
- area: transcription
- revisit if: the owner's real-GPU check shows device-loss errors arriving under another error kind

**Decision.** Narrows `p14.1-windows-vulkan/D4`. A request that fails because of its content or input (transcribe-cpp's `OutputTruncated`, `OutputRepetition`, `InvalidArgument`, `Unsupported` and other request errors) returns its typed error and leaves a GPU process running. Only the remaining native failures, the ones that may leave the device or engine unusable, retire the process. Retirement marks the engine unusable and withdraws admission before the work permit is released, so no second inference can start on a failed GPU.

**Why.** Retiring on content errors would let a few bad recordings use up `pumice serve`'s three restarts and stop transcription until it is restarted (Codex P14.2 review round 1). transcribe-cpp already tells these error kinds apart.

**Rejected.** Retiring on every native failure (the first P14.2 implementation).
