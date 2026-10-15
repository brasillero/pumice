# Pumice orchestrator (`pumice run`)

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
once (a fraction of a second for the small models). For frequent dictations a
warm service is a planned, additive option; the result contract below stays
the same.

## Setup

`pumice run` reads exactly one configuration file: `--config <FILE>`, else
the per-user `orchestrator.yml` (`$XDG_CONFIG_HOME/pumice` or `~/.config/pumice`
on Linux and macOS, `%APPDATA%\pumice` on Windows). There is no built-in
default; a missing file is an error. The file names the module
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
still reported (exit code 130 for Ctrl-C, 143 for SIGTERM). A completed
transcript is preserved in the cancelled result.

## Results, errors and exit codes

Without `--json`, stdout carries only the final text on success and stays
empty on failure; stderr explains the failure with its code and, when the
dictation survived a later failure, notes that `--json` returns the raw
transcript. The raw transcript is never printed as if it were the result.

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
the events, not error messages, not the provider's argv — does `pumice run`
write the transcript, the final text or the audio contents, and the
orchestrator keeps no log of its own. Provider calls run in the relay's
usual restricted form (no tools, a private empty workspace per call).
