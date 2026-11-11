# Backlog — p14.5-linux-vulkan

## Active

## Deferred

## Closed

### #1 · Discarded · Build the Linux Vulkan bundle inside a pinned Ubuntu 22.04 container
- type: Tech debt
- area: distribution
- origin: conversation
- ticket: —
- created: 2026-10-16
- closed: 2026-11-11
- outcome: the feature this item belongs to was removed (`relay-only/D1`); the owner asked on 2026-11-11 to clear stale material

**Context.** The glibc 2.35 floor comes from building on GitHub's `ubuntu-22.04` runner. When GitHub retires that image, the build would silently raise the floor (the ELF audit would then fail by design). Building in a digest-pinned `ubuntu:22.04` container keeps the floor independent of the runner.

**To do.**
- [ ] Move the Linux Vulkan build and stage steps into a pinned container job.
