# Required transcription settings review — 2026-10-20

The owner rejected the one-off configuration exception in p17.1-explicit-config/D1. Commit `e513da5` requires complete settings before every file transcription, including direct `--model-file` calls through either executable. Language, threads and backend flags override the file; their configuration-free defaults have been removed.

The regression tests check missing and incomplete per-user and explicit files, configured language and backend, and per-call overrides using a fake engine. All 1,246 workspace tests pass on Linux, as do formatting, workspace/all-target Clippy with warnings denied, task-notes shape checking and whitespace checking.

A separate implementation pass migrated all 21 CI inference invocations, including Docker, Windows DLL-search checks and release packaging, to a checked-in complete smoke configuration. The benchmark script now requires a settings path. Validation parsed both workflow YAML files, checked all 80 Bash blocks, audited inference calls and accepted the fixture with the built CLI under isolated user directories. No model downloads or real inference were run locally.

Independent Codex review approved the complete change with no blocking findings. Its only stale-comment finding was corrected. Windows/macOS runtime checks remain in PR #124 CI.

The initial settings file is not currently created automatically; installation instructions say to copy the example. A separate owner question about initial setup is pending. No initialization behavior, live configuration, installed binaries, model folders or services changed.
