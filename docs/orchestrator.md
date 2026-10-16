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
error. The file names the module
configurations and the flows:

```yaml
schema_version: 1
modules:
  transcription:
    config: ./transcribe.yml
  formatter:
    config: ./formatter.yml
  relay:
    location: in_process
    config: ./relay.yml
flows:
  - id: clean
    enabled: true
    transcription_model: turbo
    formatter: builtin:clean
    relay_route: claude
```

Relative module paths resolve against the orchestrator file's directory.
Module settings (model files, provider binaries, timeouts, language) live in
the module files and are never copied here. `modules.formatter` is required
once any enabled flow selects a formatter — even a built-in (an empty
registry file is enough). `modules.relay.location` must be `in_process`;
external relays are a later option. See
[`orchestrator.example.yml`](../orchestrator.example.yml) for the full
schema.

## Flows

A flow is a named, explicit combination: which transcription model, which
formatter (or an explicit `formatter: null` for none), which relay route.
Nothing is chosen implicitly: `enabled` defaults to false, and a missing
`formatter` key is an error rather than a silent choice.

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
selected flow's transcription model (via the sibling's `models list`), so a
misconfiguration fails fast, before any inference and before any paid call.

## Budgets and cancellation

One overall clock runs from command start (`budgets_ms.total`, default
180000). Each stage's deadline is the earlier of the remaining overall
budget and its own cap (`transcription` 120000, `formatter` 1000, `relay`
60000 ms by default); the budget never resets at a stage boundary, and the
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
model loaded for the service's lifetime. This release exposes the service's
health, readiness, status and flow list; **dictations arrive in the next
release** (`POST /pumice/v1/dictations`). There are no OpenAI routes here:
clients that want `/v1/chat/completions` or `/v1/audio/transcriptions` keep
using `pumice relay serve` and `pumice-transcribe serve`.

```sh
pumice serve                          # per-user orchestrator.yml, port 17570
pumice serve --config ./orchestrator.yml --port 0 --json
```

It needs a `serve:` section in the orchestrator file; files without one stay
valid for `pumice run`:

```yaml
serve:
  enabled: true               # default false: nothing starts implicitly
  port: 17570                 # optional; 7567 is refused; 0 picks a free port
  transcription_model: turbo  # required when enabled
  startup_timeout_ms: 60000   # optional, 1000 to 300000
```

- Every **enabled** flow must use `serve.transcription_model` (one model is
  loaded; a flow naming another is a startup error at its
  `transcription_model` line). Disabled flows may use any model. At most 128
  enabled flows, with ids of at most 128 bytes.
- `--port` overrides `serve.port` only, with the same rules. Port 7567 (the
  relay's) is always refused. An occupied port fails; there is no fallback
  port.
- A missing or disabled `serve:` section, and every other configuration
  problem, is reported with its `file:line:column` and exit status 2.

**Startup.** Signals are handled from the start. The configuration, the relay
and formatter configurations and every enabled flow's references are checked;
`pumice-transcribe models list` confirms the served model is configured and
installed. The frontend then claims its port (before any child starts), and
the child is started with `--port 0 --managed --instance-id <fresh id>` and
the absolute transcription configuration. Its `ready` record must match the
requested instance id, protocol version 1, pumice's exact version, the
served model, the required capabilities and `127.0.0.1` with a nonzero port,
and so must the child's own `GET /ready`. A failed initial startup is not
retried: the child is killed and reaped and `pumice serve` exits 1 (2 when
the transcription module reports a configuration error).

**Recovery.** Once ready, the child's exit is watched continuously and its
`/ready` is probed every second (500 ms deadline, one probe at a time; a
transcription in progress still answers). After an exit, or three failed
probes in a row, the child is killed, reaped and restarted after 0.5, 1 and
2 s — at most three restarts per episode; 60 s continuously ready ends the
episode. When the restarts are used up the transcription state stays
`failed` until `pumice serve` is restarted. A protocol, version or
configuration error goes to `failed` at once. A previous child is always
reaped before the next one starts. Nothing is ever replayed.

**Shutdown.** Ctrl-C or SIGTERM stops admission (the frontend closes), closes
the child's stdin, gives it 2.5 s to exit, then kills and reaps it; exit 130
(Ctrl-C) or 143 (SIGTERM), or 1 when an owned process could not be confirmed
stopped. The whole stop stays within 5 s. Windows and macOS console teardown
is checked manually.

**Core failures.** A panic inside `pumice serve` itself (its supervisor or a
request handler) is a core failure, not something it recovers from: the
service takes the same bounded stop (admission first, then the child, then
any killed process still to reap) and exits 1 with the error code
`internal`. The panic is reported as a fixed line with its source location
(`--json`: a `panicked` event with code `internal`), never its message,
which could quote request content. A crash of the transcription child is
not a core failure: it is restarted as above.

### Routes

Every request must carry the `Host` `127.0.0.1:<port>` (no `localhost` or
other names), a request with an `Origin` header is accepted only when it is
`http://127.0.0.1:<port>`, and one with a `Sec-Fetch-Site` header only when
it is `same-origin` or `none`; anything else gets 403 `host_not_allowed`,
`origin_not_allowed` or `cross_site_not_allowed`, before any routing. No
CORS headers are sent. All answers are JSON with `Cache-Control: no-store`;
errors use the OpenAI error object (`message`, `type`, `param`, `code`),
including 404 `not_found` and 405 `method_not_allowed` (with `Allow: GET`).
The routes only read in-memory state: they never wait for the transcription
child, a provider or a model.

- `GET /health` — `200` `{"status":"ok","version":"0.2.0"}` while the
  service runs, whatever the child's state.
- `GET /ready` — `200` only when the transcription child is ready and the
  service is not stopping; otherwise `503` with `code`
  `service_starting`, `transcription_restarting`, `transcription_failed` or
  `shutting_down`.
- `GET /pumice/v1/status`:
  ```json
  {"schema_version":1,"module":"orchestrator","version":"0.2.0",
   "service":{"state":"running","ready":true},
   "transcription":{"state":"ready","model":"turbo","generation":1,
     "restart_attempts":0,"next_retry_delay_ms":null,"model_load_ms":362,
     "last_failure":null},
   "flows":{"enabled":2}}
  ```
  `service.state` is `starting`, `running` or `stopping`;
  `transcription.state` is `starting`, `ready`, `restarting`, `failed` or
  `stopping`; `generation` counts the children started (1 is the first);
  `last_failure` is a fixed code such as `transcription_exited` or
  `transcription_probe_failed`. No paths, child address or content.
- `GET /pumice/v1/flows` — the enabled flows:
  ```json
  {"schema_version":1,"module":"orchestrator","flows":[
    {"id":"clean","transcription_model":"turbo","formatter":"builtin:clean",
     "relay_route":"claude","ready":true}]}
  ```
  When the service is not ready, each flow has `"ready":false` and an
  `unavailable` code (the same codes as `/ready`).

Limits: at most 64 connections (more are closed at once), one request per
connection, request headers within 5 s and 16 KiB. These metadata routes, and
every rejection, also bound the whole connection to 10 s from its arrival and
answers to 128 KiB; longer-running routes get their own deadlines.

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
`child_restarting`, `transcription_unavailable`, `stopping`, `panicked`, …)
with the child `generation` and fixed codes. Both streams are bounded and
written by their own threads; a stalled reader loses events, never blocks
the service.
The child's own stderr is drained and discarded, never shown.
