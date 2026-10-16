---
title: P14.5: Linux Vulkan transcription bundle, checked on a real GPU
state: Done
ticket: —
branch: p14.5-linux-vulkan
mr: —
start: 2026-10-16
end: 2026-10-16
---

Linux x86-64 (glibc 2.35+) GPU transcription. The `vulkan` feature now builds on Linux GNU as well as Windows: the Linux CPU variants plus a loadable `gpu/vulkan/libggml-vulkan.so`, staged by `build-linux-vulkan.sh` with a required `bundle.json`, an exact ELF dependency allowlist (only the GPU module needs `libvulkan.so.1`) and a relocation check. CI builds it on `ubuntu-22.04` with the pinned LunarG SDK and tests no device, forced software Vulkan (lavapipe, which `gpu` runs and `auto` refuses), a missing loader in a pinned container, and damaged bundles. First real-GPU check, on server-1's Intel UHD 770 (`reports/2026-10-16-server-1-real-gpu.md`): everything works, and the integrated GPU is about 1.7× slower than the CPU for turbo, which led to D1–D2: `auto` uses only a GPU positively identified as discrete hardware (PCI bus id and no software/virtual driver name) and otherwise stays on the CPU with `gpu_integrated_only`, `gpu_unverified` or `gpu_software_only`; `backend: gpu` can use any GPU. D3: the engine cannot tell a device that failed to start from one that failed while loading weights, so `auto` keeps a hard error there. Also records the owner's `arch-modules/D16` (one CLI installed as a folder) and fixes a supervisor test race. Implemented by Claude Opus 5.5 subagents; three Codex review rounds (round 1: forced software device in `auto`; round 2: PCI is not proof of hardware, init fallback, checklist; round 3: approve); final review and the real-GPU checks by the orchestrator.
