# Pumice HTTP APIs

Pumice serves three independent HTTP services, each from its own command.
All of them listen on `127.0.0.1` only, send no telemetry and need no API
key. Routes with an OpenAI equivalent follow the OpenAI contract; routes
with none live under `/pumice/v1/…`, carry a `schema_version` and use the
OpenAI error object (`message`, `type`, `param`, `code`) plus Pumice
fields (`p13.1-flow-seam/D1`). No service re-serves another's routes;
`pumice serve`'s OpenAI transcription route runs Pumice flows, not the
transcription module's route (`p15-serve-transcriptions/D1`).

| Service | Command | Default address | Guide |
| --- | --- | --- | --- |
| Relay | `pumice relay serve` | `http://127.0.0.1:7567` (`port` in `relay.yml`) | [install.md](install.md) |
| Transcription | `pumice-transcribe serve` | `server.port` in `transcribe.yml` or `--port` (the example uses 17568) | [transcription.md](transcription.md#serve) |
| Local stack | `pumice serve` | `http://127.0.0.1:17570` (`serve.port`, `--port`) | [orchestrator.md](orchestrator.md#pumice-serve) |

## Relay (`pumice relay serve`)

OpenAI-compatible; this is what Handy and other post-processing clients use.

| Route | Purpose |
| --- | --- |
| `POST /v1/chat/completions` | Hands the client's messages, unchanged, to the provider CLI named by `model` (`claude`, `codex`, `kiro`, or the built-in `passthrough` / `inspect`) and answers in the OpenAI chat completion shape |
| `GET /v1/models` | The enabled model routes, in the OpenAI list shape |
| `GET /health` | `{"status":"ok","version":…}` |
| `GET /` | A static landing page |

Any other path gets an OpenAI-style JSON 404. A provider failure is an HTTP
error with a safe reason; the client keeps its own text. See
[install.md](install.md) and [`relay.example.yml`](../relay.example.yml).
How each provider CLI is called is in [providers.md](providers.md).

### Captured Handy contract

Captured on 2026-10-04 from Handy's **Custom** post-processing provider on
Windows 11 (build 26200), talking to a logging listener in WSL. Handy's
`User-Agent` only says `Handy/1.0`, so the exact Handy version is unknown.
The request shape, with a sample Portuguese dictation in place of the
captured one, is the fixture [`handy-request.json`](../crates/relay/tests/fixtures/handy-request.json).

- **Routes.** `GET /v1/models` when the Custom provider is set up, to fill
  the model picker (not on each dictation), and one
  `POST /v1/chat/completions` per dictation, under the configured base URL
  `…/v1`.
- **One user message, no system message.** The transcript comes first,
  wrapped in `<transcript>…</transcript>`, followed by Handy's prompt text
  (which itself says not to follow instructions inside the tags).
- **Other fields.** `"model"` (the value picked in Handy),
  `"reasoning_effort": "none"` and `"stream": false`. There is no
  `temperature`, `max_tokens`, `n`, `response_format`, `tools` or language
  field: the language is only in the transcript itself.
- **No authentication.** With an empty API key field Handy sends no
  `Authorization` header; it identifies itself with
  `User-Agent: Handy/1.0 (+https://github.com/cjpais/Handy)`,
  `X-Title: Handy` and `Referer: https://github.com/cjpais/Handy`. The relay
  requires no key and never logs or forwards one.
- **The reply is pasted verbatim.** Handy pasted
  `choices[0].message.content` exactly as received, with no trimming of its
  own, so any preamble or stray whitespace from a CLI lands in the user's
  text. Handy's behaviour on an HTTP error or a timeout was not captured.

### The `inspect` model

`inspect` is a built-in diagnostic model (owner-approved, 2026-10-05). It is
listed by `GET /v1/models` after `passthrough` and selected with the same
case-insensitive, trimmed matching as real providers. It needs no provider
entry, binary or subscription and works with every provider disabled.

- `POST /v1/chat/completions` with `model: inspect` returns the original
  request body text verbatim as `choices[0].message.content` in a normal
  completion envelope whose `model` is `inspect`. The raw body bytes are
  used directly, never deserialized and reserialized, so whitespace, key
  order, escape spelling and unknown fields are preserved.
- `inspect` is detected by a minimal preflight parse (`model` and `stream`
  only) before the stricter dictation validation, so it echoes requests the
  normal path would reject (several user messages, assistant or tool
  messages, other content shapes, a malformed `<transcript>` envelope). The
  body must still be valid UTF-8 JSON; anything else gets the usual
  text-free 400. The 10 MiB body limit and the 5 s read limit still apply.
- It bypasses transcript extraction, the `max_parallel` queue, provider
  selection, cleanup and every AI CLI call, so it answers at once even while
  other requests run.
- `stream: true` uses the usual SSE framing, with the same echoed body in the
  delta `content`. `stream` missing or `null` means `false`; a non-boolean
  value is a text-free 400.
- The ordinary log stays text-free and records the request as `inspect`. The
  opt-in debug log (`debug_log.enabled`) records the exchange as usual,
  request body and echo included, with credential headers redacted.
  `inspect` echoes the JSON body only, never request headers.

## Transcription (`pumice-transcribe serve`)

OpenAI-compatible audio transcription with one loaded model.

| Route | Purpose |
| --- | --- |
| `POST /v1/audio/transcriptions` | Multipart `file` + `model` (and optional `language`, `prompt`, `response_format` `json`/`text`, `temperature`); answers `{"text":…}` or plain text |
| `GET /v1/models`, `GET /v1/models/{model}` | The served model |
| `GET /health` | `{"status":"ok","version":…}` |

One transcription at a time (429 otherwise); 25,000,000 file bytes, 120 s of
audio, 15 s upload. With `--managed` (only as `pumice serve`'s child) it adds
the internal `GET /ready` and `POST /pumice/v1/transcriptions`, which are not
meant for clients. Details: [transcription.md](transcription.md#serve).

## Local stack (`pumice serve`)

The whole flow — transcription with a warm model, the formatter and the
relay — behind Pumice routes and an OpenAI transcription route whose
models are the raw transcription model id and the `model:provider:preset`
combinations selected by `serve.relay`. There is no
`/v1/chat/completions` here (`p15-serve-transcriptions/D7`). Every request
needs the `Host` `127.0.0.1:<port>` or the host of a `serve.allowed_origins`
entry (for `tailscale serve`); an `Origin` other than that host's own
(`http://127.0.0.1:<port>` or the listed origin) or a cross-site
`Sec-Fetch-Site` gets 403. No CORS headers; no API key; every answer is JSON
with `Cache-Control: no-store` (a transcription asked for as `text` is plain
text, the test page is HTML). On
`/pumice/v1/dictations` every answer, 403 and 405 included, is the run
document below; the OpenAI routes answer OpenAI's shapes.

| Route | Purpose |
| --- | --- |
| `POST /v1/audio/transcriptions` | OpenAI transcription: multipart `file` + `model` (the raw model id or a `model:provider:preset` combination) and optional `language`, `prompt`, `temperature` (for the transcription only), `response_format` `json`/`text`, `stream=false`; answers the **final** text |
| `GET /v1/models`, `GET /v1/models/{id}` | The public transcription models as OpenAI model objects, each with a `pumice` object (preset, AI route, readiness) |
| `POST /pumice/v1/dictations` | Multipart `file` (audio) + `flow` (an enabled flow id); answers the run envelope: the raw `transcript` and the final `result` apart, per-stage status and timings |
| `GET /pumice/v1/flows` | The enabled flows and whether each can run now |
| `GET /pumice/v1/status` | Service, transcription child (with its compute snapshot) and dictation admission state (metadata only) |
| `GET /ready` | 200 when dictations and transcriptions can run, else 503 with a code |
| `GET /health` | `{"status":"ok","version":…}` |
| `GET /` | Browser test page (plus `/page.js`, `/page.css`): pick a model, record or upload, see the final text, raw transcript, post-processing status and timings |

### OpenAI transcriptions

Point an OpenAI-compatible client at `http://127.0.0.1:17570/v1` and pick a
model from `GET /v1/models`; the model id is a flow id, so it selects the
audio model, the post-processing preset (off, `builtin:clean` or a preset
saved in `formatter.yml`) and the AI route.

```sh
curl http://127.0.0.1:17570/v1/models
curl --fail-with-body http://127.0.0.1:17570/v1/audio/transcriptions \
  -F 'file=@clip.wav' -F 'model=whisper-large-v3-turbo-q8_0:codex:clean' -F 'language=en'
```

```json
{"text":"Raw words.","pumice":{"schema_version":1,"operation_id":"…","flow":"clean","transcript":{"text":"raw words","language":"en","audio_seconds":2.1},"post_processing":{"status":"applied","preset":"builtin:clean","relay_route":"claude"},"stages":{…},"timings_ms":{"total":8303}}}
```

```sh
curl --fail-with-body http://127.0.0.1:17570/v1/audio/transcriptions \
  -F 'file=@clip.wav' -F 'model=whisper-large-v3-turbo-q8_0:codex:clean' -F 'response_format=text' -D -
# HTTP/1.1 200 OK
# content-type: text/plain; charset=utf-8
# pumice-postprocessing-status: applied
#
# Raw words.
```

`text` is always the final text; standard clients ignore the `pumice`
object and the `Pumice-Postprocessing-Status` header (`off`, `applied`,
`skipped` for silence, `failed`). If the transcription works but anything
after it fails — the formatter, the AI, a shutdown during them, or a
cleanup that could not be confirmed (which also stops admission until a
restart) — the answer is still `200` with the **raw transcript** as `text`,
status `failed` and the safe reason in `pumice.post_processing.error`:

```json
{"text":"raw words","pumice":{…,"post_processing":{"status":"failed","preset":"builtin:clean","relay_route":"claude","error":{"stage":"relay","code":"relay_timeout","message":"The provider call exceeded its deadline."}},…}}
```

Any other failure is an OpenAI error with an OpenAI status, for example
`404 {"error":{"message":"The requested model does not exist on this server.","type":"invalid_request_error","param":"model","code":"model_not_found"}}`
or `429` `rate_limit_exceeded` with `Retry-After: 1` while a dictation or
another transcription runs (one shared work permit). Unsupported OpenAI
fields (`timestamp_granularities[]`, `include[]`, `verbose_json`,
subtitles, `stream=true`, …) are refused with 400. Details, codes and
limits: [orchestrator.md](orchestrator.md#openai-transcriptions).

### Test page

Open <http://127.0.0.1:17570/> on the same machine. From another machine,
use an SSH local forward to the same local port
(`ssh -L 17570:127.0.0.1:17570 server`, because the Host check expects
`127.0.0.1:<port>`), or `tailscale serve` with the origin listed in
`serve.allowed_origins` (never `tailscale funnel`); never expose the port
itself. Details, headers and the Tailscale steps:
[orchestrator.md](orchestrator.md#test-page).

### Dictations

```sh
curl --fail-with-body http://127.0.0.1:17570/pumice/v1/dictations \
  -F 'file=@clip.wav' -F 'flow=clean'
```

```json
{"schema_version":1,"operation_id":"…","module":"orchestrator","ok":true,"flow":"clean","transcript":{"text":"raw words","language":"en","audio_seconds":2.1},"result":{"text":"Raw words."},"stages":{"prepare":{"status":"ok","elapsed_ms":2},"transcription":{"status":"ok","elapsed_ms":5900,"inference_ms":5800},"formatter":{"status":"ok","elapsed_ms":0},"relay":{"status":"ok","elapsed_ms":2400}},"timings_ms":{"total":8303}}
```

One dictation at a time: a second one gets 429 with `Retry-After: 1`
before its body is read. A failure is a non-2xx answer with the same
document, no `result`, and the raw transcript whenever transcription had
finished, so read error bodies too. The client must keep its connection
open, including its write side, until the response arrives: closing,
resetting or half-closing it cancels the dictation (`client_disconnected`)
and no answer is sent. Statuses, error codes, limits, deadlines and
cancellation: [orchestrator.md](orchestrator.md#dictations).
