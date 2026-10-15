# Decisions — p9-transcribe-downloader

### D1 · Active · The first catalog has Whisper tiny and Large v3 Turbo, Q8_0, from Handy's pinned repositories
- date: 2026-10-15
- decided by: claude (orchestrator), brasillero (authorized)
- origin: p6-transcribe-http/reports/2026-10-15-implementation-order.md
- area: transcription
- revisit if: users need other models or quantizations

**Decision.** The built-in catalog lists two models: `whisper-tiny-q8_0` and `whisper-large-v3-turbo-q8_0`, each pinned to the `handy-computer` Hugging Face repository revision, file name, byte size and SHA-256 already verified in this project (s10.1 research).

**Why.** These are the two models in use and tested; their hashes were checked against Handy's catalog.

**Rejected.** Shipping Handy's whole catalog: unverified files and licences.

### D2 · Active · Downloaded models live in a per-user data directory and never edit the config
- date: 2026-10-15
- decided by: claude (orchestrator), brasillero (authorized)
- origin: p6-transcribe-http/reports/2026-10-15-implementation-order.md
- area: config
- revisit if: the setup wizard takes over model registration

**Decision.** `models download ID` stores the verified file under the per-user data directory (`$XDG_DATA_HOME/pumice/models`, else `~/.local/share/pumice/models`; `%LOCALAPPDATA%\pumice\models` on Windows; `~/Library/Application Support/pumice/models` on macOS), overridable by `models_dir` in `transcribe.yml`. A model entry in `transcribe.yml` can refer to a catalog model instead of a path; downloading never edits the config and never enables a model.

**Why.** Keeps downloads explicit and the config file owned by the user (`arch-modules/D9`, `D10`).

**Rejected.** Writing downloaded paths into `transcribe.yml` automatically.

### D3 · Active · Downloads restart cleanly instead of resuming
- date: 2026-10-15
- decided by: claude (orchestrator), brasillero (authorized)
- origin: p6-transcribe-http/reports/2026-10-15-implementation-order.md
- area: transcription
- revisit if: downloads over slow links fail often enough to need resuming

**Decision.** An interrupted download leaves no installed file and restarts from zero next time; there is no HTTP range resume.

**Why.** The plan allows a clean restart first (item 3c optional); resume adds identity checks (ETag, size) and more failure modes.

**Rejected.** Range-based resume in the first version.
