---
title: P17.2: discover transcription models in the models folder
state: In progress
ticket: —
branch: p17.2-models-folder
mr: —
start: 2026-10-20
end: —
---

Implements p17-serve-model-names/D4, D7 and D8: the models folder replaces the models list in transcribe.yml. Matching size and SHA-256 give catalog ids regardless of file names; other files use their file stems. CLI selection, listings, downloads, orchestrator preflight, examples and CI use the inventory. Implementation is complete and the workspace tests pass on Linux; independent review is pending. The live configuration and model files are unchanged. Served model combinations and relay-off execution follow in P17.3.
