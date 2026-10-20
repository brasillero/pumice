# Backlog — p15-serve-transcriptions

## Active

## Deferred

### #1 · Deferred · Select post-processing through metadata instead of model names
- type: Improvement
- area: api
- origin: D1
- ticket: —
- created: 2026-10-18
- reason: first version uses model names (D1); the owner wants a less clunky solution later

**Context.** Model ids combine audio model, preset and AI route, so the list grows with every combination. A later version could describe the options as metadata (for example a `pumice` object per model in `GET /v1/models` and optional explicit request fields) so a UI can offer the audio model, the preset and the AI route as separate choices.

**To do.**
- [ ] Design the metadata and request fields once a client or Pumice's own UI can use them.
- [ ] Consider a formatter `user_template` with placeholders (`{{transcript}}` in `<transcript>` tags, `{{language}}`): on 2026-10-20 Claude Haiku followed the owner's list rules when they sat in the user message with the tagged transcript (Handy's layout), but mostly not as a system prompt.

### #3 · Deferred · Try `pumice serve` from T3 Code
- type: Task
- area: api
- origin: conversation
- ticket: —
- created: 2026-10-18
- reason: upstream T3 Code (9c16146) has no configurable OpenAI transcription endpoint (design report); needs a T3 build that offers one

**Context.** The owner wants to use Pumice from T3 Code and similar tools through their OpenAI-compatible settings.

**To do.**
- [ ] Owner configures the base URL and a model in T3 Code and records what works.

## Closed

### #2 · Moved · Browser test page on `pumice serve`
- type: Task
- area: api
- origin: p13.4-dictations#1
- ticket: —
- created: 2026-10-18
- closed: 2026-10-18
- outcome: → p15.3-test-page#1

**Context.** The owner wants to try the first version from an HTML page before T3. The page records or uploads audio, lists the models from `GET /v1/models` and calls `POST /v1/audio/transcriptions`, showing the final text and, when present, the raw transcript and timings. It replaces the planned P13.5 page for the dictation route.

**To do.**
- [ ] Serve the page from `pumice serve` and test it with the owner's flows on server-1.
