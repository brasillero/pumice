---
title: P17.2: discover transcription models in the models folder
state: Done
ticket: —
branch: p17.2-models-folder
mr: PR #124
start: 2026-10-20
end: 2026-10-20
---

Implements p17-serve-model-names/D4, D7 and D8: the models folder replaces the models list in transcribe.yml. Matching size and SHA-256 give catalog ids regardless of file names; other files use their file stems. CLI selection, listings, downloads, orchestrator preflight, examples and CI use the inventory. Implementation and independent Codex review are complete. All 1,246 workspace tests pass on Linux, with formatting and Clippy clean; PR #124 awaits CI before merge. The owner also rejected configuration-free file transcription (p17.1-explicit-config/D1); every file call now requires complete settings, and CI, scripts and documentation supply them. Review findings and validation are in reports/2026-10-20-review.md and reports/2026-10-20-required-settings-review.md. The live configuration and model files are unchanged. Served model combinations and relay-off execution follow in P17.3.
