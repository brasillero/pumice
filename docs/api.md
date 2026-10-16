# Pumice HTTP APIs

Pumice serves three independent HTTP services, each from its own command.
All of them listen on `127.0.0.1` only, send no telemetry and need no API
key. Routes with an OpenAI equivalent follow the OpenAI contract; routes
with none live under `/pumice/v1/…`, carry a `schema_version` and use the
OpenAI error object (`message`, `type`, `param`, `code`) plus Pumice
fields (`p13.1-flow-seam/D1`). No service re-serves another's routes.

| Service | Command | Default address | Guide |
| --- | --- | --- | --- |
| Relay | `pumice relay serve` | `http://127.0.0.1:7567` (`port` in `relay.yml`) | [install.md](install.md) |
| Transcription | `pumice-transcribe serve` | `server.port` in `transcribe.yml` or `--port` (the example uses 17568) | [transcription.md](transcription.md#serve) |
| Local stack | `pumice serve` | `http://127.0.0.1:17570` (`serve.port`, `--port`) | [orchestrator.md](orchestrator.md#pumice-serve) |

## Relay (`pumice relay serve`)

OpenAI-compatible; this is what Handy and other post-processing clients use.

| Route | Purpose |
| --- | --- |
| `POST /v1/chat/completions` | Hands the client's messages, unchanged, to the provider CLI named by `model` (`claude`, `codex`, `kimi`, `kiro`, or the built-in `passthrough` / `inspect`) and answers in the OpenAI chat completion shape |
| `GET /v1/models` | The enabled model routes, in the OpenAI list shape |
| `GET /health` | `{"status":"ok","version":…}` |
| `GET /` | A static landing page |

Any other path gets an OpenAI-style JSON 404. A provider failure is an HTTP
error with a safe reason; the client keeps its own text. See
[install.md](install.md) and [`relay.example.yml`](../relay.example.yml).

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
relay — behind custom routes only (no OpenAI routes, `p13.1-flow-seam/D2`).
Every request needs the `Host` `127.0.0.1:<port>`; an `Origin` other than
`http://127.0.0.1:<port>` or a cross-site `Sec-Fetch-Site` gets 403. No CORS
headers; every answer is JSON with `Cache-Control: no-store`. On
`/pumice/v1/dictations` every answer, 403 and 405 included, is the run
document below.

| Route | Purpose |
| --- | --- |
| `POST /pumice/v1/dictations` | Multipart `file` (audio) + `flow` (an enabled flow id); answers the run envelope: the raw `transcript` and the final `result` apart, per-stage status and timings |
| `GET /pumice/v1/flows` | The enabled flows and whether each can run now |
| `GET /pumice/v1/status` | Service, transcription child and dictation admission state (metadata only) |
| `GET /ready` | 200 when dictations can run, else 503 with a code |
| `GET /health` | `{"status":"ok","version":…}` |

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
