# Decisions — p17.1-explicit-config

### D1 · Active · File transcription requires a complete settings file
- date: 2026-10-20
- decided by: brasillero
- origin: #4
- area: config
- revisit if: —

**Decision.** Every `pumice-transcribe file` command, including a direct `--model-file PATH`, requires a complete `transcribe.yml`. Flags may override settings, but there is no configuration-free transcription path. This also applies through `pumice transcribe`.

**Why.** The owner expects the settings file to be present for every transcription and explicitly rejected the one-off exception after its behavior was explained.

**Rejected.** Transcribing a one-off file without settings using built-in language, thread and CPU defaults.

**Consequences.** Remove the exception and its flag defaults, update smoke jobs and manual instructions to provide complete settings files, and test missing and incomplete settings as errors. Whether first-time setup should write the initial files is being clarified separately; the current installer does not create them.
