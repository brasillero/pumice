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

### D5 · Active · Published flows are the transcription models
- date: 2026-10-18
- decided by: claude (orchestrator, owner-delegated 2026-10-18)
- origin: reports/2026-10-18-p15-design.md
- area: config
- revisit if: #1 adds separate metadata choices

**Decision.** A model on `pumice serve`'s transcription endpoint is an orchestrator flow, and its id is the flow id. `serve.transcription_flows` lists the flows to publish (empty by default; nothing is published implicitly). Published ids use only `A–Z a–z 0–9 . _ -`, start with a letter or digit and are at most 128 characters. Post-processing is off when a flow has `formatter: null` and `relay_route: passthrough`. All published flows use the served transcription model, because `pumice serve` keeps one audio model loaded.

**Why.** Flows already combine audio model, formatter preset and AI route with their time budgets; a second list would duplicate them.

**Rejected.** A separate models section; generated combinations of every preset and route.

### D6 · Active · The transcription endpoint supports `json` and `text`, with Pumice details in a `pumice` object
- date: 2026-10-18
- decided by: claude (orchestrator, owner-delegated 2026-10-18)
- origin: reports/2026-10-18-p15-design.md
- area: api
- revisit if: a client needs `verbose_json`, subtitles or streaming

**Decision.** Accepted fields: `file`, `model`, `language`, `prompt`, `temperature`, `response_format` (`json` or `text`) and `stream=false`; anything else is a 400. `language`, `prompt` and `temperature` go to the transcription only; `prompt` never reaches the AI. In `json`, `text` is the final text and a `pumice` object carries the raw transcript, the post-processing status (`off`, `applied`, `skipped`, `failed`), stages and timings. In `text`, the body is the final text and a `Pumice-Postprocessing-Status` header carries the status. Errors use OpenAI's error object and status codes. Dictations and transcriptions share one work permit.

**Why.** Standard clients read `text` and ignore extra fields; `verbose_json` and subtitles need segment timings that do not exist after rewriting or translation.

**Rejected.** Fabricated `verbose_json`; returning the dictation run envelope.

### D7 · Active · `pumice serve` does not add `/v1/chat/completions`
- date: 2026-10-18
- decided by: claude (orchestrator, owner-delegated 2026-10-18)
- origin: reports/2026-10-18-p15-design.md
- area: api
- revisit if: a client needs chat on the same port

**Decision.** Chat stays on `pumice relay serve`.

**Why.** The owner's first version is audio in, final text out; chat on this port needs its own validation, routing and model-list design.

**Rejected.** Mounting the relay router on `pumice serve` now.

### D8 · Active · The browser test page is served by `pumice serve` at `/`
- date: 2026-10-18
- decided by: claude (orchestrator, owner-delegated 2026-10-18)
- origin: reports/2026-10-18-p15-design.md
- area: api
- revisit if: —

**Decision.** `pumice` embeds the test page (based on `tools/transcribe-test`) and serves it at `GET /` with a strict content security policy and the existing Host/Origin guard. It lists `/v1/models`, records or uploads audio and calls `/v1/audio/transcriptions` on the same origin. On server-1 the owner opens it through an SSH local forward of `127.0.0.1:17570` to the same local port.

**Why.** One command gives the owner something to test; no Python proxy and no external assets.

**Rejected.** A separate page server; exposing the port beyond loopback.
