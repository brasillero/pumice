# Decisions — p15-serve-transcriptions

### D1 · Active · `pumice serve` exposes OpenAI transcription, and the model name selects the post-processing
- date: 2026-10-18
- decided by: owner (2026-10-18)
- origin: conversation
- area: api
- revisit if: the metadata-based selection in #1 is built

**Decision.** `pumice serve` adds `POST /v1/audio/transcriptions` and `GET /v1/models` on its port. Each model id is an explicitly configured combination of an audio model, a post-processing preset and, when the preset is not off, an AI route; `text` in the response is the final text. Narrows `p13.1-flow-seam/D2`, whose revisit condition (a client needing OpenAI transcription on the dictation port) is now met; the dictation, flow and status routes stay.

**Why.** Generic OpenAI clients let the user choose only the base URL, the model and sometimes the language, so the model name is the one place every client can carry the post-processing choice. The owner accepts this as a first step and wants a metadata-based selection later (#1).

**Rejected.** A single endpoint taking both audio and text (no OpenAI client could call it); formatter selection through the chat endpoint only (dictation apps send audio to the transcription endpoint); Pumice-specific request fields as the only selector (generic clients cannot send them).

### D2 · Active · Custom post-processing presets are user formatter files named in YAML
- date: 2026-10-18
- decided by: owner (2026-10-18)
- origin: D1
- area: config
- revisit if: a UI for editing presets is built

**Decision.** Besides off and the shipped presets, a user's custom preset is a formatter file the user writes and registers in `formatter.yml` under a name of their choice; it becomes selectable through a model id like any other preset. Presets are not defined from a client UI.

**Why.** The formatter already loads user files; a UI is out of scope (AGENTS.md).

**Rejected.** Free-form prompts sent per request by the client as the custom option.

### D3 · Superseded by D4 · When post-processing fails, the transcription endpoint returns the raw transcript by default
- date: 2026-10-18
- decided by: claude (orchestrator, owner-delegated 2026-10-18)
- origin: D1
- area: api
- revisit if: the owner wants strict errors as the default

**Decision.** Each transcription model has a failure setting: `raw` (default) returns the raw transcript as `text` with the failure marked in Pumice-specific response metadata, `error` returns an HTTP error whose body carries the raw transcript. A transcription failure is always an error.

**Why.** The client sent audio and has no text of its own, so an error would lose the whole dictation; the owner agreed to the proposed per-model setting with a raw default. The AGENTS.md rule "on failure return an HTTP error, the app keeps its own text" assumes a text request.

**Rejected.** Errors only; raw fallback without a setting.

### D4 · Active · When post-processing fails, the transcription endpoint always returns the raw transcript
- date: 2026-10-18
- decided by: owner (2026-10-18)
- origin: D3
- area: api
- revisit if: —

**Decision.** If transcription succeeds and post-processing (formatter or AI route) fails, the response is a normal success with the raw transcript as `text`; the failure is marked only in Pumice-specific metadata that standard clients ignore. If transcription fails, the endpoint returns an error in OpenAI's error format and status codes. There is no per-model failure setting.

**Why.** The owner: "return the transcribed text, if possible. If the transcription fails, then we will need an error. Just follow the OpenAI API standards."

**Rejected.** D3's per-model `raw`/`error` setting.
