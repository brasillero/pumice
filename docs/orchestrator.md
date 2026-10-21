# Pumice orchestrator (`pumice run`, `pumice serve`)

`pumice run` combines the modules into one dictation flow: it transcribes an
audio file with the sibling `pumice-transcribe` program, applies an
explicitly selected formatter, and calls the relay — the last two
in-process, inside the `pumice` executable (arch-modules/D13). It starts no
server and listens on no port; one invocation handles one audio file and
exits.

```sh
pumice run meeting.wav --flow clean            # plain text on stdout
pumice run meeting.wav --flow clean --json     # one JSON result document
```

Because the transcription program starts cold, every run pays the model load
once (a fraction of a second for the small models). For frequent dictations,
[`pumice serve`](#pumice-serve) keeps one transcription model loaded behind a
local service.

## Setup

`pumice run` reads exactly one configuration file: `--config <FILE>`, else
the per-user `orchestrator.yml` (`$XDG_CONFIG_HOME/pumice` or `~/.config/pumice`
on Linux and macOS, `%APPDATA%\pumice` on Windows). These bases must be
nonempty absolute paths: a relative `XDG_CONFIG_HOME` is ignored in favour
of `~/.config`, and with no usable base the lookup is unavailable and
`--config` is required. There is no built-in default; a missing file is an
error, and so is a setting the file leaves out: every setting is written,
and an omitted one is reported with the file and the line of the mapping it
belongs in (line 1 for a top-level one). The file names the module
configurations, the flows and the `pumice serve` section:

```yaml
schema_version: 1
modules:
  transcription:
    config: ./transcribe.yml
  formatter:                  # or `formatter: null` when no flow uses one
    config: ./formatter.yml
  relay:
    location: in_process
    config: ./relay.yml
flows:
  - id: clean
    enabled: true
    transcription_model: whisper-large-v3-turbo-q8_0
    formatter: builtin:clean
    relay_route: claude
    budgets_ms:
      total: 180000
      transcription: 120000
      formatter: 1000
      relay: 60000
serve:
  enabled: false              # pumice serve stays off
  port: 17570
  transcription_model: null   # null only while serve is disabled
  startup_timeout_ms: 60000
  budgets_ms:
    total: 180000
    transcription: 120000
    formatter: 1000
    relay: 60000
  relay:
    enabled: true
    default_provider: codex
    default_preset: clean
    presets: [clean, clean-en]
    expand_providers: false
    expand_presets: false
  allowed_origins: []
```

Relative module paths resolve against the orchestrator file's directory.
Module settings (the models folder, provider binaries, timeouts, language)
live in the module files and are never copied here. A flow's
`transcription_model` is a model id from the transcription models folder:
a catalog model's catalog id (`whisper-large-v3-turbo-q8_0`), any other
`.gguf` file's exact name without `.gguf`
([transcription.md](transcription.md#the-models-folder)). `modules.formatter` names a
file once any enabled flow selects a formatter — even a built-in (an empty
registry file is enough); otherwise it may be `null`. `modules.relay.location` must be `in_process`;
external relays are a later option. See
[`orchestrator.example.yml`](../orchestrator.example.yml) for the full
schema.

## Flows

A flow is a named, explicit combination: which transcription model, which
formatter (or an explicit `formatter: null` for none), which relay route.
Nothing is chosen implicitly: every flow writes `enabled` (true or false)
and its four `budgets_ms`, and a missing `formatter` key is an error rather
than a silent choice.

- A flow with `formatter: null` adds no instructions; combine it with
  `relay_route: passthrough` for a raw-only flow that never calls an AI.
- The `inspect` route cannot be used in a flow, and a formatter cannot be
  combined with `passthrough` (its rules would never execute); both are
  rejected at load.
- The raw transcript is sent to the relay as literal text — dictation that
  contains `<transcript>` tags keeps them as data.

`pumice run` requires `pumice-transcribe` installed next to `pumice` (the
same rule as `pumice transcribe`). Before transcribing, it validates the
module configurations, every enabled flow's route and formatter, and the
selected flow's transcription model (it must be in the models folder, via
the sibling's `models list`), so a misconfiguration fails fast, before any
inference and before any paid call. Listing hashes the folder's files that
have a catalog model's size (a few seconds for large models), within the
flow's `budgets_ms.total`; the `file` call then reads the folder again.

## Budgets and cancellation

One overall clock runs from command start (`budgets_ms.total`; the example
writes 180000). Each stage's deadline is the earlier of the remaining
overall budget and its own cap (`transcription`, `formatter` and `relay`;
the example writes 120000, 1000 and 60000 ms); the budget never resets at a
stage boundary, and the
last 2500 ms are reserved for cleanup and output. Module limits (the relay's
own timeouts) can only shorten a stage further. A spent overall budget is
`flow_timeout`; a spent stage cap is `<stage>_timeout`.

Ctrl-C (and SIGTERM on Unix) cancel the run at any point: the current stage
stops, every child process tree — transcription or provider CLI, including
anything they spawned — is killed and reaped, and a cancelled result is
still reported (exit code 130 for Ctrl-C, 143 for SIGTERM). Under `--json`,
a completed transcript is included in the cancelled result.

## Results, errors and exit codes

Without `--json`, stdout carries only the final text on success and stays
empty on failure; stderr explains the failure with its code. When
transcription succeeded but a later stage failed, stderr also notes that the
raw transcript is included only in the `--json` envelope. `pumice run`
stores nothing, so that transcript cannot be retrieved afterwards: rerunning
with `--json` repeats the whole flow — a new transcription and possibly
another paid provider call. Callers that must keep the dictation on failure
should use `--json`. The raw transcript is never printed as if it were the
result.

With `--json`, stdout carries exactly one `schema_version: 1` document,
success or failure alike:

```json
{"schema_version":1,"operation_id":"…","module":"orchestrator","ok":true,"flow":"clean","transcript":{"text":"raw words","language":"en","audio_seconds":2.1},"result":{"text":"Raw words."},"stages":{"prepare":{"status":"ok","elapsed_ms":8},"transcription":{"status":"ok","elapsed_ms":490,"model_load_ms":350,"inference_ms":120},"formatter":{"status":"ok","elapsed_ms":1},"relay":{"status":"ok","elapsed_ms":800}},"timings_ms":{"total":1300}}
```

- `transcript` is the raw transcription, byte for byte, present once
  transcription succeeded — also when a later stage failed, so the dictation
  is never lost. `result` is present only on success.
- `stages` always lists `prepare`, `transcription`, `formatter` and `relay`
  with status `ok`, `error`, `cancelled` or `skipped` (skipped stages have a
  null elapsed).
- On failure, `error` carries a stable `code`, the failing `stage`, a safe
  `message`, optionally a module `cause_code` and a config `location`
  (`file`, `line`, `column`). `retryable` is always false: a possibly paid
  call is never replayed automatically; a caller may offer an explicit new
  attempt.
- Under `--json`, stderr carries a bounded stream of metadata-only JSONL
  events (`started`, `stage_started`, `stage_finished`, `finished`). Events
  never contain text, can be dropped under backpressure, and stdout stays
  authoritative.

Exit codes: **0** success; **2** invalid arguments, configuration, audio
input, model or route; **1** runtime failure (provider errors, timeouts,
protocol problems); **130** Ctrl-C; **143** SIGTERM. When stdout cannot be
written (for example, its reader has closed the pipe), the command exits 1
without writing to stderr, so a blocked stderr can never hold up the exit.

Stable error codes, by stage: `invalid_arguments`, `invalid_config`,
`flow_not_found`, `flow_disabled` (prepare); `transcription_not_installed`,
`transcription_protocol_error`, `transcription_invalid_input`,
`transcription_model_not_found`, `transcription_engine_failed`,
`transcription_timeout` (prepare or transcription); `formatter_not_found`,
`formatter_instruction_conflict`, `formatter_invalid_request`,
`formatter_timeout` (prepare or formatter); `relay_route_invalid`,
`relay_not_installed`, `relay_not_logged_in`, `relay_quota_exceeded`,
`relay_rate_limited`, `relay_timeout`, `relay_invalid_output`,
`relay_failed` (prepare or relay); plus `flow_timeout`, `cancelled`,
`cleanup_failed` and `internal`.

## Privacy

The result document intentionally contains the dictation: it is a sensitive
artifact for the caller to store securely. Nowhere else — not stderr, not
the events, not error messages — does `pumice run` write the transcript,
the final text or the audio contents, and the orchestrator keeps no log of
its own. Provider calls run in the relay's usual restricted form (no tools,
a private empty workspace per call).

The provider's argv carries no dictation either, with one exception: the
Kimi CLI accepts its message only as a command-line argument (`-p MESSAGE`;
see [`research/S2.4-kimi-adapter.md`](research/S2.4-kimi-adapter.md)), so a
flow routed to `kimi` puts the user message — the transcript — in Kimi's
argv (the formatter's instructions go into a private agent file). While
that call runs, other local users can see the transcript in the process
list. Use another provider for dictation
that must stay private on a shared machine.

## `pumice serve`

`pumice serve` runs the local stack as a service on `127.0.0.1`: one
loopback frontend and one warm `pumice-transcribe serve --managed` child
(the sibling program, found like `pumice run` finds it) with the served
model loaded for the service's lifetime. It takes dictations
(`POST /pumice/v1/dictations`: audio plus a flow id in, the raw transcript
and the final text out, through the same flows as `pumice run`), answers
OpenAI transcription clients using the real model id or a
`model:provider:preset` combination (`POST /v1/audio/transcriptions`, and
`GET /v1/models`: see [OpenAI transcriptions](#openai-transcriptions)) and
exposes its health, readiness, status and flow list. There is no
`/v1/chat/completions` here: chat clients keep using `pumice relay serve`
(all HTTP APIs: [`api.md`](api.md)).

```sh
pumice serve                          # per-user orchestrator.yml, port 17570
pumice serve --config ./orchestrator.yml --port 0 --json
```

It runs from the `serve:` section of the orchestrator file, which every
file writes in full (`enabled: false` keeps it off for `pumice run`):

```yaml
serve:
  enabled: true               # nothing starts implicitly
  port: 17570                 # 7567 is refused; 0 picks a free port
  transcription_model: whisper-large-v3-turbo-q8_0  # a models folder id; null only while off
  startup_timeout_ms: 60000   # 1000 to 300000; the models list and each child start
  budgets_ms:
    total: 180000
    transcription: 120000
    formatter: 1000
    relay: 60000
  relay:
    enabled: true
    default_provider: codex
    default_preset: clean
    presets: [clean, clean-en]
    expand_providers: false
    expand_presets: false
  allowed_origins: []         # [] accepts none besides 127.0.0.1; see "Reaching pumice serve over Tailscale"
```

- Every **enabled** flow must use `serve.transcription_model` (one model is
  loaded; a flow naming another is a startup error at its
  `transcription_model` line). Disabled flows may use any model. At most 128
  enabled flows, with ids of at most 128 bytes.
- The real `serve.transcription_model` id is always listed first and returns
  raw text, independently of `flows:`. With `serve.relay.enabled: true`, the
  default combination is also listed as `model:provider:preset`. The default
  provider must be enabled in `relay.yml`; the default preset must be in
  `serve.relay.presets`. Presets name formatter definitions in `formatter.yml`
  or the built-in `builtin:clean` (which produces an id such as
  `whisper-large-v3-turbo-q8_0:codex:builtin:clean`). All keys are required.
- `expand_providers` lists every enabled provider in `relay.yml` order;
  `expand_presets` lists every preset in the configured order. Both switches
  give every combination, providers first. The relay routes `passthrough` and `inspect`
  are never expanded as providers. Duplicate presets, unregistered references, invalid
  generated ids and collisions fail before listening. Ids are exact,
  case-sensitive, at most 512 bytes and contain no control characters.
- `serve.budgets_ms` supplies the four budgets for every public transcription
  model. Upload and connection limits include these budgets as well as the
  dictation flow budgets. `flows:` still selects dictation behavior, and
  `pumice run` keeps its own flow budgets and behavior.
- With `serve.relay.enabled: false`, only the raw model is listed. The service
  loads no relay or formatter runtime or configuration. Raw dictations work;
  AI dictations fail with 503 `relay_disabled` before transcription. The
  complete serve settings remain required, including the inactive relay keys.
- `serve.transcription_flows` was removed. Old files fail with a migration
  message: replace the list with `serve.budgets_ms` and `serve.relay`.
- The model list and the enabled flow list must each fit 128 KiB in every
  state. Excessive expansion fails at startup; disable expansion or shorten
  the model, provider or preset ids. No entries are silently truncated.
- `allowed_origins` lists browser origins accepted besides
  `http://127.0.0.1:<port>`, for a reverse proxy on the same machine
  ([below](#reaching-pumice-serve-over-tailscale)); nothing is accepted
  unless listed. Each entry is an exact origin as browsers send it:
  `http://` or `https://`, a lowercase host name or IPv4 address, and a port
  only when it is not the scheme's default (`https://host.example:17570`,
  `https://host.example`); no path (not even a trailing `/`), query,
  fragment, user name, wildcard or IPv6 address, and no duplicates (checked
  at load, with the entry's line). `pumice serve` still listens on
  `127.0.0.1` only.
- `--port` overrides `serve.port` only, with the same rules. Port 7567 (the
  relay's) is always refused. An occupied port fails; there is no fallback
  port.
- A missing or disabled `serve:` section, and every other configuration
  problem, is reported with its `file:line:column` and exit status 2.

**Startup.** Signals are handled from the start. The configuration, the relay
and formatter configurations and every enabled flow's references are checked;
`pumice-transcribe models list` confirms the served model is in the
transcription models folder, within `serve.startup_timeout_ms` (listing
hashes files with a catalog model's size, a few seconds for large models;
there is no other limit). The frontend then claims its port (before any child starts), and
the child is started with `--port 0 --managed --instance-id <fresh id>` and
the absolute transcription configuration. Its `ready` record must match the
requested instance id, protocol version 2, pumice's exact version, the
served model, the required capabilities, `127.0.0.1` with a nonzero port and
a well-formed compute snapshot (known values, bounded strings, consistent
fallback reason), and so must the child's own `GET /ready`, whose snapshot
must equal the announced one on every probe. The compute backend comes from
`backend:` in the transcription configuration; `orchestrator.yml` has no
backend setting. A failed initial startup is not
retried: the child is killed and reaped and `pumice serve` exits 1 (2 when
the transcription module reports a configuration error).

**Recovery.** Once ready, the child's exit is watched continuously and its
`/ready` is probed every second (500 ms deadline, one probe at a time; a
transcription in progress still answers). After an exit, or three failed
probes in a row, the child is killed, reaped and restarted after 0.5, 1 and
2 s — at most three restarts per episode; 60 s continuously ready ends the
episode. A child that retires itself after a GPU engine failure (`stopping`
with reason `engine_failed`, then its exit) is restarted the same way. When
the restarts are used up the transcription state stays
`failed` until `pumice serve` is restarted. Every child start reads the
transcription models folder again and loads the same model id, or fails:
a model no longer in the folder (`model_not_found`) is never replaced by
another. A protocol, version or configuration error, or a missing model,
goes to `failed` at once, and so does a restarted child
reporting `gpu_unavailable` (`backend: gpu` without a usable GPU). A previous child is always
reaped before the next one starts. Nothing is ever replayed.

**Shutdown.** Ctrl-C or SIGTERM stops admission (the frontend closes) and
cancels a running dictation, which still answers 503 within a 2.5 s drain
(with its raw transcript if transcription had finished); then it closes the
child's stdin, gives it 2.5 s to exit, then kills and reaps it; exit 130
(Ctrl-C) or 143 (SIGTERM), or 1 when an owned process could not be confirmed
stopped. The whole stop stays within 5 s. Windows and macOS console teardown
is checked manually.

**Core failures.** A panic inside `pumice serve` itself (its supervisor, a
request handler or a dictation's task, including the in-process relay and
formatter) is a core failure, not something it recovers from: the
service takes the same bounded stop (admission first, then the child, then
any killed process still to reap) and exits 1 with the error code
`internal`. The panic is reported as a fixed line with its source location
(`--json`: a `panicked` event with code `internal`), never its message,
which could quote request content. A crash of the transcription child is
not a core failure: it is restarted as above.

### Routes

Every request must carry the `Host` `127.0.0.1:<port>` (no `localhost` or
other names) or the host of a `serve.allowed_origins` entry (with its port,
which may be left out on the scheme's default port; any letter case). A
request with an `Origin` header is accepted only when it is the origin of
that host: `http://127.0.0.1:<port>`, or the listed origin exactly (so a
listed origin is refused on the loopback host, and the other way round).
One with a `Sec-Fetch-Site` header is accepted only when it is
`same-origin` or `none`. Anything else gets 403 `host_not_allowed`,
`origin_not_allowed` or `cross_site_not_allowed`, before any routing. No
CORS headers are sent. All answers are JSON with `Cache-Control: no-store`
(an OpenAI transcription asked for as `text` is plain text, the
[test page](#test-page) is HTML, JavaScript and CSS);
errors use the OpenAI error object (`message`, `type`, `param`, `code`),
including 404 `not_found` and 405 `method_not_allowed` (with `Allow: GET`).
On the dictation route every answer, its 403 and its 405
`method_not_allowed` (with `Allow: POST`) included, is the dictation
document below, whose `error` is that object with a few more fields. The
guard still comes first: a refused origin gets 403 whatever the method. On
`POST /v1/audio/transcriptions` the 403 and the 405 (with `Allow: POST`)
are the plain OpenAI error object. The metadata routes, the model list
included, only read
in-memory state: they never wait for the transcription child, a provider, a
model or a running dictation.

- `GET /health` — `200` `{"status":"ok","version":"0.2.0"}` while the
  service runs, whatever the child's state.
- `GET /ready` — `200` only when the transcription child is ready, the
  service is not stopping and dictations are admitted (a running dictation
  does not change that); otherwise `503` with `code` `service_starting`,
  `transcription_restarting`, `transcription_failed`, `admission_stopped`
  or `shutting_down`.
- `GET /pumice/v1/status`:
  ```json
  {"schema_version":1,"module":"orchestrator","version":"0.2.0",
   "service":{"state":"running","ready":true},
   "transcription":{"state":"ready","model":"turbo","generation":1,
     "restart_attempts":0,"next_retry_delay_ms":null,"model_load_ms":362,
     "compute":{"requested":"auto","backend":"vulkan",
       "device":{"name":"Vulkan0","description":"NVIDIA GeForce RTX 4070"},
       "cpu_variant":"haswell","fallback_reason":null},
     "last_failure":null},
   "flows":{"enabled":2,"published":2},
   "dictations":{"served":true,"admission":"busy","active":true,
     "stage":"transcription"}}
  ```
  `service.state` is `starting`, `running` or `stopping`;
  `transcription.state` is `starting`, `ready`, `restarting`, `failed` or
  `stopping`; `generation` counts the children started (1 is the first);
  `compute` is the ready child's compute snapshot as it announced it
  ([transcription.md](transcription.md#compute-backend-cpu-auto-gpu)), `null`
  while no child is ready;
  `last_failure` is a fixed code such as `transcription_exited`,
  `transcription_probe_failed` or `transcription_recycled`.
  `flows.published` counts the public transcription models.
  `dictations.admission` is `open`, `busy` (a dictation or an OpenAI
  transcription is running; they share one permit, and `stage` names its
  current stage: `prepare`, `transcription`, `formatter`, `relay` or
  `cleanup`) or `stopped` (a cleanup could not be confirmed; see below).
  No paths, child address, operation ids or content.
- `GET /pumice/v1/flows` — the enabled flows:
  ```json
  {"schema_version":1,"module":"orchestrator","flows":[
    {"id":"clean","transcription_model":"turbo","formatter":"builtin:clean",
     "relay_route":"claude","ready":true}]}
  ```
  When the service is not ready, each flow has `"ready":false` and an
  `unavailable` code (the same codes as `/ready`).
- `GET /v1/models`, `GET /v1/models/{id}` — the public transcription models as OpenAI
  models ([below](#openai-transcriptions)).
- `GET /`, `GET /page.js`, `GET /page.css` — the [test page](#test-page).

Limits: at most 64 connections (more are closed at once), one request per
connection, request headers within 5 s and 16 KiB. These metadata routes, and
every rejection, also bound the whole connection to 10 s from its arrival and
answers to 128 KiB; a dictation's connection may last its longest enabled
flow's total budget plus 5 s, with answers up to 16 MiB (an OpenAI
transcription's too).

### Dictations

`POST /pumice/v1/dictations` takes `multipart/form-data` with exactly two
parts, in either order:

- `file`: one uploaded, non-empty audio file (a file part). Its bytes are
  passed to the transcription child unchanged, held in memory only (never
  written to disk); its file name and declared type are not passed on. The
  format is detected by the transcription module ([formats and
  limits](transcription.md#supported-formats-and-limits)).
- `flow`: the id of an enabled flow (a text part, UTF-8 without NUL, at
  most 128 bytes). The flow chooses the formatter and the relay route;
  transcription options (language, prompt) come from `transcribe.yml`.

Anything else — a missing, duplicated or unknown part, a `file` sent as
text or a `flow` sent as a file — is refused with 400 before any work.

```sh
curl --fail-with-body --max-time 200 http://127.0.0.1:17570/pumice/v1/dictations \
  -F 'file=@clip.wav' -F 'flow=clean'
```

The answer is the same `schema_version: 1` document as `pumice run --json`,
with an HTTP status. Success is `200`:

```json
{"schema_version":1,"operation_id":"…","module":"orchestrator","ok":true,"flow":"clean","transcript":{"text":"raw words","language":"en","audio_seconds":2.1},"result":{"text":"Raw words."},"stages":{"prepare":{"status":"ok","elapsed_ms":2},"transcription":{"status":"ok","elapsed_ms":5900,"inference_ms":5800},"formatter":{"status":"ok","elapsed_ms":0},"relay":{"status":"ok","elapsed_ms":2400}},"timings_ms":{"total":8303}}
```

`prepare` measures receiving the upload and selecting the flow;
`inference_ms` is the engine call alone (the model was loaded once, at
startup: see status). A whitespace-only transcript succeeds unchanged, with
the formatter and relay skipped; a `passthrough` flow returns the
transcript without a provider call.

Every failure is non-2xx and carries the same document with `ok: false`, no
`result`, and the raw transcript in `transcript` whenever transcription had
finished (also after a provider failure or a shutdown) — read the body of an
error answer to recover the dictation; nothing is stored, so a lost answer
cannot be fetched again. The error object has the OpenAI fields plus the
flow's:

```json
{"schema_version":1,"operation_id":"…","module":"orchestrator","ok":false,"flow":"clean","transcript":{"text":"raw words","audio_seconds":2.1},"stages":{"prepare":{"status":"ok","elapsed_ms":2},"transcription":{"status":"ok","elapsed_ms":5900,"inference_ms":5800},"formatter":{"status":"ok","elapsed_ms":0},"relay":{"status":"error","elapsed_ms":900}},"timings_ms":{"total":6802},"error":{"message":"The provider returned output that could not be parsed.","type":"server_error","param":null,"code":"relay_invalid_output","stage":"relay","retryable":false,"cause_code":"invalid_output"}}
```

`code` is one of the [stable codes](#results-errors-and-exit-codes) or a
request code below; `param` names `file` or `flow` when one of them is at
fault; there is no `location` (no server paths). Messages are fixed text
and never quote what the client sent (flow ids, part names, file names),
the transcript or the transcription module's own messages. A request
refused before its flow was selected has no `flow`, and its later stages
are `skipped`.

| Status | `type` | When (`code`) |
| --- | --- | --- |
| 400 | `invalid_request_error` | `not_multipart`, `malformed_multipart`, `missing_file`, `missing_flow`, `duplicate_parameter`, `unsupported_parameter`, `empty_file`, `file_not_upload`, `flow_not_text`, `invalid_utf8`, `nul_in_text`, `flow_disabled`; undecodable, unsupported or too long audio (`transcription_invalid_input`, with the module's `cause_code`) |
| 403 | `invalid_request_error` | the `Host`/`Origin`/`Sec-Fetch-Site` guard: `host_not_allowed`, `origin_not_allowed`, `cross_site_not_allowed` |
| 404 | `invalid_request_error` | `flow_not_found` |
| 405 | `invalid_request_error` | `method_not_allowed` (not a `POST`; `Allow: POST`) |
| 408 | `invalid_request_error` | `upload_timeout` |
| 413 | `invalid_request_error` | `body_too_large`, `file_too_large`, `field_too_large` |
| 429 | `rate_limit_error` | `rate_limit_exceeded` (another dictation is running; `Retry-After: 1`), `relay_quota_exceeded`, `relay_rate_limited` |
| 500 | `server_error` | `transcription_engine_failed`, `cleanup_failed`, `internal`, server configuration codes |
| 502 | `server_error` | `transcription_unavailable` (the child crashed or closed the connection), `transcription_protocol_error`, `relay_failed`, `relay_invalid_output` |
| 503 | `server_error` | `service_starting`, `transcription_restarting`, `transcription_failed`, `transcription_busy`, `admission_stopped`, `shutting_down`, `relay_not_installed`, `relay_not_logged_in`; `cancelled` with `cause_code` `service_shutdown` |
| 504 | `server_error` | `transcription_timeout`, `formatter_timeout`, `relay_timeout`, `flow_timeout` |

**Admission.** One dictation runs at a time. A second one gets 429 with
`Retry-After: 1` at once, before its body is read; there is no queue. When
the service is starting, the child is restarting or failed, or the service
is stopping, a dictation gets 503 before its body is read. The work permit
is held from admission to the end of the dictation's cleanup. The metadata
routes never wait for it.

**Limits and deadlines.** The whole body (chunked or not, epilogue
included) is at most 26,000,000 bytes, the file at most 25,000,000 bytes,
a declared larger `Content-Length` is refused before reading. Every clock
starts at the request's arrival: the upload must be complete within
`min(15 s, smallest enabled flow's budgets_ms.total − 2.5 s)` (408 after
that; one unusually short flow shortens the window for all), and the
selected flow's whole budget is counted from the arrival too, so a slow
upload leaves less for the flow, never more. Each stage's deadline is the
earlier of its own budget and the flow's; the transcription module's own
120 s cap still applies. The child's answer is read up to 1 MiB, the whole
answer is at most 16 MiB: a final text that does not fit fails as
`relay_invalid_output` with `cause_code` `output_too_large`, never cut.
No stage is retried and nothing falls back to another provider.

**Cancellation.** A client that closes or resets its connection cancels its
dictation at once with `cancelled`, `cause_code` `client_disconnected`,
during the upload, the transcription or the provider call (the provider's
process tree is killed and reaped; a provider call not started yet never
starts). Half-close is not supported: the client must keep its connection
open, including its write side, until the response arrives. A client that
half-closes (`shutdown(SHUT_WR)`) after sending its request is treated as
gone, like a reset — its dictation is cancelled and the connection closes
without an answer — because the service cannot tell it apart from a client
that closed or was killed. A transcription abandoned after it was sent to
the child — by a disconnect, a deadline or a shutdown — leaves the child
busy with work nobody waits for, so that child is killed, reaped and
replaced (status: a new `generation`, `last_failure`
`transcription_recycled`) before the next dictation is admitted; it is never
asked to cancel and nothing is replayed. A cancellation during the formatter
or the provider call leaves the child alone. After every dictation the
killed provider processes are reaped and the provider's temporary directory
is removed; if any of that, or the recycle (the old child confirmed reaped),
cannot be confirmed within its bound, the dictation fails with
`cleanup_failed` (never a success; after another failure that error stays
the reported one) and no further dictation is admitted (`admission:
"stopped"`, `/ready` 503 `admission_stopped`) until `pumice serve` is
restarted, so no paid call starts over processes that may still run.

### OpenAI transcriptions

The real transcription model id returns raw text. With the relay on,
`model:provider:preset` ids return text processed by that provider and preset.
A generic client needs only the base URL `http://127.0.0.1:17570/v1` and the
model id; it needs no API key (any `Authorization` header is ignored).

`GET /v1/models` lists raw first, then the configured combinations.
`GET /v1/models/{id}` looks up the exact id after decoding percent escapes
once. A literal `+` stays `+`; malformed escapes or invalid UTF-8 give 400
`invalid_model`. An unknown id gives 404 `model_not_found` with `param: model`.
Multipart `model` fields are already text and are never URL-decoded.
Each OpenAI model object includes a `pumice` object standard clients ignore:

```json
{"object":"list","data":[
  {"id":"whisper-large-v3-turbo-q8_0","object":"model","created":0,"owned_by":"pumice",
   "pumice":{"schema_version":1,"flow":null,"model":"whisper-large-v3-turbo-q8_0","transcription_model":"whisper-large-v3-turbo-q8_0",
     "post_processing":{"preset":null,"relay_route":"passthrough"},
     "response_formats":["json","text"],"ready":true}},
  {"id":"whisper-large-v3-turbo-q8_0:codex:clean","object":"model","created":0,"owned_by":"pumice",
   "pumice":{"schema_version":1,"flow":null,"model":"whisper-large-v3-turbo-q8_0:codex:clean","transcription_model":"whisper-large-v3-turbo-q8_0",
     "post_processing":{"preset":"clean","relay_route":"codex"},
     "response_formats":["json","text"],"ready":true}}]}
```

`preset` is null and `relay_route` is `passthrough` for raw. While the service
is unready, every model stays listed with `ready: false` and an `unavailable`
code. Listing never waits for the child, a provider or a running request.

`POST /v1/audio/transcriptions` takes `multipart/form-data` with these
parts, in any order, each at most once (OpenAI's subset, D6):

| Part | |
| --- | --- |
| `file` | Required. One uploaded, non-empty audio file, handled like a dictation's ([formats and limits](transcription.md#supported-formats-and-limits)) |
| `model` | Required. A public transcription model id, matched exactly |
| `language` | Optional. A two-letter ISO-639-1 code for the transcription (blank: use the transcription default, `transcribe.yml`'s `language`; otherwise detect) |
| `prompt` | Optional. Whisper's initial prompt for the transcription (empty: none) |
| `temperature` | Optional. 0 to 1, for the transcription |
| `response_format` | Optional. `json` (default) or `text` |
| `stream` | Optional. Only `false` |

`language`, `prompt` and `temperature` go to the transcription child only,
replacing `transcribe.yml`'s defaults for this request; they never reach
the formatter or the AI, which get exactly what a dictation gives them.
Any other part (`timestamp_granularities[]`, `include[]`,
`chunking_strategy`, …), `verbose_json`, `srt`, `vtt`, `diarized_json`
and `stream=true` are refused with 400 before any work.

```sh
curl --fail-with-body --max-time 200 http://127.0.0.1:17570/v1/audio/transcriptions \
  -F 'file=@clip.wav' -F 'model=whisper-large-v3-turbo-q8_0:codex:clean' -F 'language=en'
```

With `json`, `text` is the final text, next to a `pumice` object:

```json
{"text":"Raw words.","pumice":{"schema_version":1,"operation_id":"…","flow":null,"model":"whisper-large-v3-turbo-q8_0:codex:clean","transcription_model":"whisper-large-v3-turbo-q8_0","transcript":{"text":"raw words","language":"en","audio_seconds":2.1},"post_processing":{"status":"applied","preset":"clean","relay_route":"codex"},"stages":{"prepare":{"status":"ok","elapsed_ms":2},"transcription":{"status":"ok","elapsed_ms":5900,"inference_ms":5800},"formatter":{"status":"ok","elapsed_ms":0},"relay":{"status":"ok","elapsed_ms":2400}},"timings_ms":{"total":8303}}}
```

With `text`, the body is exactly the final text (`text/plain;
charset=utf-8`). Both carry the header `Pumice-Postprocessing-Status` with
`post_processing.status`:

- `off` — the plain model: `text` is the raw transcript;
- `applied` — the formatter and the AI route ran;
- `skipped` — a silent (whitespace-only) transcript: no AI call, `text` is
  the transcript unchanged;
- `failed` — the transcription worked but something after it failed: the
  formatter or the AI route (an error, a timeout, an empty or too large
  output, a quota or login problem), a shutdown during the formatter or the
  AI call, or a cleanup that could not be confirmed (`stage` `cleanup`,
  `code` `cleanup_failed`). The answer is still `200`, `text` is the **raw
  transcript**, and `post_processing.error` has the failure's safe `stage`,
  `code`, `message` and `cause_code` (D4, D9). Nothing is retried and no
  other provider is tried. An unconfirmed cleanup still stops admission
  afterwards, exactly as for a dictation (`admission: "stopped"`, and
  every later dictation or transcription gets 503 `admission_stopped`).

Every other failure is an OpenAI error object — exactly `message`, `type`,
`param` and `code` — with the statuses of the [dictation
table](#dictations) and these request codes: 400 `missing_model`,
`text_param_as_file`, `invalid_language`, `invalid_temperature`,
`invalid_response_format`, `stream_unsupported`, `invalid_stream` (plus the
dictation's multipart codes; `language` is also refused by the child when
Whisper does not know it), 404 `model_not_found`, 405 with `Allow: POST`,
and 429 `rate_limit_exceeded` with `Retry-After: 1`. Transcription
failures (undecodable audio, engine failure, timeout, an unavailable or
broken child) are errors, since there is no text to return; so is a
cleanup that could not be confirmed when no transcript was obtained (500
`cleanup_failed`). The dictation route keeps its own rule: a dictation
over an unconfirmed cleanup is never a success.

A transcription is admitted, limited, timed, cancelled and cleaned up
exactly like a dictation, with the same work permit: while one of them
runs the other gets 429, and an abandoned transcription recycles the child
the same way. The same privacy holds: no audio, transcript, prompt or final
text is logged, and events are the dictation's metadata-only events.

### Test page

`pumice serve` has a small browser page for trying its transcription
models (p15-serve-transcriptions/D8). It is a test page, not the product
interface. On the same machine, open <http://127.0.0.1:17570/> (your
`serve.port`; `localhost` is refused by the Host check). The page:

- lists `GET /v1/models` with each model's preset, AI route and readiness
  (Refresh reloads it);
- takes an optional two-letter language (blank: the transcription default);
- records from the microphone — the browser's own WebM/Opus through
  MediaRecorder (default), or a WAV built in the page, which is also the
  fallback when the browser cannot record WebM/Opus — and sends at 2
  minutes at the latest, or uploads a file (WAV, MP3, FLAC, Ogg/Vorbis,
  WebM/Opus, up to 25,000,000 bytes and 120 s); the microphone is released
  when recording stops;
- sends it to `POST /v1/audio/transcriptions` with
  `response_format=json` and shows the final `text` (with a Copy button),
  the raw transcript with its language and length, the post-processing
  status (`failed` is highlighted with its safe stage, code and message),
  the per-stage and total server timings, the round trip, the HTTP status
  and the raw answer; OpenAI errors are shown readably (for example 429
  busy, 404 unknown model, 503 not ready);
- has a Cancel button, which discards a recording or aborts the request
  (closing the request also cancels the transcription on the server), and
  gives up after 200 s with a message.

Audio and results stay in the page's memory (reload to clear them);
nothing is stored and everything is rendered as text. It loads only its
own `page.js` and `page.css` and talks only to its own origin, under
`Content-Security-Policy: default-src 'none'; script-src 'self'; style-src
'self'; connect-src 'self'; img-src 'self' data:; media-src 'self' blob:;
frame-ancestors 'none'; base-uri 'none'; form-action 'none'`, with
`X-Content-Type-Options: nosniff`, `Cache-Control: no-store`,
`Referrer-Policy: no-referrer` and `Permissions-Policy: microphone=(self)`.
The three files are GET only (other methods: 405 with `Allow: GET`) and
behind the same guard as every route.

**From another machine** the page must reach `pumice serve` through
something that keeps the guard's Host and Origin. Never expose the port
itself beyond loopback. Two ways:

- **SSH local forward to the same port** (nothing to configure):

  ```sh
  ssh -L 17570:127.0.0.1:17570 server
  ```

  then open <http://127.0.0.1:17570/> on your machine. Use the same local
  port as the server's: the Host check expects `127.0.0.1:<serve.port>`.
  The browser treats `127.0.0.1` as a secure context, so the microphone
  works.
- **Tailscale**, [below](#reaching-pumice-serve-over-tailscale): HTTPS on
  your tailnet, also from a phone.

### Reaching pumice serve over Tailscale

`tailscale serve` can put `pumice serve` on your tailnet over HTTPS while
it keeps listening on `127.0.0.1` only. The proxy passes the browser's own
`Host` (`<machine>.<tailnet>.ts.net:<port>`) and `Origin`, so list that
origin in `serve.allowed_origins` first; without it every request through
the proxy gets 403 `host_not_allowed`. HTTPS is also what lets a browser on
another device use the microphone.

1. Find the machine's tailnet name (for example with `tailscale status`) and
   add the origin, with the HTTPS port you will serve on, to the `serve:`
   section:

   ```yaml
   serve:
     # …the section's other keys…
     allowed_origins:
       - https://server-1.example-tailnet.ts.net:17570
   ```

   Restart `pumice serve`.
2. Proxy that HTTPS port on the tailnet to the loopback port:

   ```sh
   tailscale serve --bg --https=17570 http://127.0.0.1:17570
   ```

3. Open `https://server-1.example-tailnet.ts.net:17570/` from a device on
   the tailnet.

Use `tailscale serve` (tailnet only), **never `tailscale funnel`**, which
publishes to the internet. There is no authentication: every device and
user that can reach this machine on your tailnet (as your tailnet's access
rules allow) can use the page and the API and spend your AI subscriptions.
When you stop using it, turn the proxy off (see `tailscale serve --help`
for your version) and remove the origin from `serve.allowed_origins`. The
SSH forward above is the alternative without any listed origin.

### Output

Plain mode writes short lines on stderr, starting with
`pumice serve: listening on http://127.0.0.1:<port>`, and nothing on
stdout. With `--json`, stdout carries lifecycle records, one JSON object per
line:

```json
{"schema_version":1,"event":"listening","module":"orchestrator","version":"0.2.0","host":"127.0.0.1","port":17570}
{"schema_version":1,"event":"ready","module":"orchestrator","version":"0.2.0","host":"127.0.0.1","port":17570}
{"schema_version":1,"event":"stopped","module":"orchestrator","exit_code":143}
```

`listening` comes once the port is claimed (with the actual port, also for
`--port 0`), `ready` once the first child is ready; a failure instead writes
one `{"event":"error","code":…,"message":…}` record (with `cause_code` when
the transcription module reported its own error code). stderr then carries
metadata-only events (`child_starting`, `child_ready`, `child_failed`,
`child_restarting`, `child_recycled`, `transcription_unavailable`,
`admission_stopped`, `stopping`, `panicked`, …) with the child `generation`
and fixed codes, and each dictation's or transcription's own events
(`started`, `stage_started`, `stage_finished`, `finished`, with stage
names, elapsed milliseconds and error codes) under its `operation_id`, as
`pumice run --json` writes them. No event, record or plain line ever
carries audio, a transcript, a prompt, a final text, a flow or model id
sent by a client or a file name. Like `pumice run`, dictations call the relay's pipeline
in-process, so the relay's `debug_log` (written by `pumice relay serve`'s
HTTP handlers) does not record them; `pumice serve` keeps no content log of
its own. Both streams are bounded and
written by their own threads; a stalled reader loses events, never blocks
the service.
The child's own stderr is drained and discarded, never shown.
