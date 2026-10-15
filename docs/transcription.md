# pumice-transcribe

`pumice-transcribe` is Pumice's local speech-to-text module. It takes an audio
file and a Whisper GGUF model and returns the raw transcript. It is a
standalone CLI; it does not talk to the relay or to any AI provider.

When the transcription program is installed next to the `pumice`
executable, every command below also works through the dispatcher with a
`pumice transcribe` prefix — for example `pumice transcribe file audio.wav
--model tiny` — with arguments, output and exit status passed through
unchanged. The dispatcher never searches `PATH` or the working directory;
it only runs the `pumice-transcribe` sitting in the real `pumice`
executable's own install directory (a symlinked `pumice` still resolves to
its actual install directory).

On Windows, Ctrl-C/Ctrl-Break is delivered to the console, not to a single
process: while the transcription program handles the event and shuts down,
`pumice` keeps waiting and then exits with the same full exit code.

> **npm installs:** the npm package's real native binary lives inside the
> platform package (`node_modules/@brasillero/pumice-<platform>/bin/`), and
> no transcription binary ships there yet, so `pumice transcribe` is not
> satisfied by an npm install of the launcher today. Until the Pumice
> transcription archive ships (planned), install both programs into the
> same directory, for example `cargo install --path . --locked` and
> `cargo install --path crates/transcription --features engine --locked`.

## Building from source

With [Rust](https://rustup.rs/) installed:

```bash
cargo install --path crates/transcription --features engine --locked
```

The default build is a portable x86-64 baseline with no host-specific CPU
tuning, so it runs on any x86-64 machine without SIGILL. To opt into a native
build tuned to the host CPU, override `TRANSCRIBE_CMAKE_ARGS`:

```bash
TRANSCRIBE_CMAKE_ARGS="-DGGML_NATIVE=ON" \
  cargo install --path crates/transcription --features engine --locked
```

Use a separate `CARGO_TARGET_DIR` for native builds to avoid cached CMake flags
leaking between builds:

```bash
CARGO_TARGET_DIR=target/native TRANSCRIBE_CMAKE_ARGS="-DGGML_NATIVE=ON" \
  cargo install --path crates/transcription --features engine --locked
```

See `crates/transcription/README.md` for the full build section, including
Windows notes.

## Faster CPU builds (runtime CPU variants)

Linux x86-64 only, opt-in. The default build above is a single static binary
with a portable x86-64 baseline; on modern CPUs it leaves a lot of transcription
speed unused. The `cpu-variants` Cargo feature instead links the engine as
shared libraries plus one ggml CPU module per x86 ISA tier (14 modules from
baseline `x64` up to `sapphirerapids`). At startup the binary picks the
fastest module the running CPU **and OS** can actually execute, using Rust's
`is_x86_feature_detected!` (which checks OS-enabled state via XGETBV, unlike
ggml's own CPUID-only scoring), and loads only that module, from
`cpu/<variant>/libggml-cpu-<variant>.so` inside the executable's own
directory — never from the current directory or PATH. Each module is staged
alone in its variant directory at build time, so ggml's directory probe can
only ever see the selected one. Nothing is copied or written at runtime: the
staged folder works from a read-only install directory. If the selected
variant's module was not staged, the binary falls back to the next staged
variant this CPU and OS can execute, down to the plain `x64` portable floor,
so an incomplete staged package degrades instead of failing; variants whose
features the host lacks are never fallback candidates, so there is no SIGILL
risk from the fallback walk.

Build and stage a self-contained folder (binary + its shared libraries and
CPU modules) in one step:

```bash
crates/transcription/scripts/build-cpu-variants.sh
# result: target/cpu-variants/stage/
```

The staged binary resolves its libraries from its own directory and works
from any location; copy the whole folder. Run it exactly like the regular
CLI. It prints the selected CPU module on stderr at startup, `serve` reports
the selected backend at model load, and `file --json` includes a `backend`
field, e.g. `"CPU (...) [haswell]"`.

The portable build's engine linkage and compiler baseline are unaffected: the
script uses its own `CARGO_TARGET_DIR` and overrides the conservative
`TRANSCRIBE_CMAKE_ARGS` from `.cargo/config.toml`, so nothing in the portable
build changes. (The one visible addition to ordinary `engine` builds from
this work is the `backend` field in `file --json` and the startup backend
line; the binary stays a single static portable executable.) Windows and
macOS builds are unchanged (the feature is off by default and its runtime
wiring is Linux x86-64-only; other architectures have no module mapping).

Measurements for both builds on server-1 and the details of the mechanism are
in [`docs/research/P7-cpu-variants.md`](../docs/research/P7-cpu-variants.md).

## Getting models

`pumice-transcribe` needs a Whisper GGUF model. The owner uses these two
models, pinned by revision and SHA-256:

| ID | Model | Source | Revision | SHA-256 |
| --- | --- | --- | --- | --- |
| `turbo` | `whisper-large-v3-turbo-Q8_0.gguf` | `handy-computer/whisper-large-v3-turbo-gguf` | `5eaf945c` | `b2e30cc286bc9f3aba4db9099fc7403543497c05ce7100d0d83091ddfd25a183` |
| `tiny` | `whisper-tiny-Q8_0.gguf` | `handy-computer/whisper-tiny-gguf` | `6687f30c` | `325b9c7997cd1eff81ef709d55766565e71be696130cc3a3d444713798706834` |

`tiny` is fast and rough; `turbo` is slower and usually more accurate.

A manual download recipe (verify the exact Hugging Face URL form before you
run it; URLs can change):

```bash
# tiny
url="https://huggingface.co/handy-computer/whisper-tiny-gguf/resolve/6687f30c99641ee265df421e582354adbc8848fc/whisper-tiny-Q8_0.gguf"
curl -L --fail -o whisper-tiny-Q8_0.gguf "$url"
echo "325b9c7997cd1eff81ef709d55766565e71be696130cc3a3d444713798706834  whisper-tiny-Q8_0.gguf" | sha256sum -c -

# turbo
url="https://huggingface.co/handy-computer/whisper-large-v3-turbo-gguf/resolve/5eaf945c/whisper-large-v3-turbo-Q8_0.gguf"
curl -L --fail -o whisper-large-v3-turbo-Q8_0.gguf "$url"
echo "b2e30cc286bc9f3aba4db9099fc7403543497c05ce7100d0d83091ddfd25a183  whisper-large-v3-turbo-Q8_0.gguf" | sha256sum -c -
```

An automatic downloader tied to a pinned catalog is planned for a later phase.

## Configuration

`pumice-transcribe` reads one configuration file:

1. the file passed with `--config`, or
2. the per-user file:
   - Linux/macOS: absolute `$XDG_CONFIG_HOME/pumice/transcribe.yml`, or
     `$HOME/.config/pumice/transcribe.yml`
   - Windows: `%APPDATA%\pumice\transcribe.yml`

A missing per-user file is fine; the CLI then works from flags only. A missing
explicit `--config` file is an error.

See [`transcribe.example.yml`](../transcribe.example.yml) for the full schema.
The short form:

```yaml
schema_version: 1
default_model: tiny      # optional; must name an entry in models
language: pt             # optional default language hint
threads: 8               # optional default thread count (1-32)
models:                  # explicit list; nothing is enabled implicitly
  - id: turbo
    path: whisper-large-v3-turbo-Q8_0.gguf
  - id: tiny
    path: whisper-tiny-Q8_0.gguf
```

Relative `path` values resolve against the directory that contains the config
file. Paths must be nonempty. Model ids must match `[a-z0-9][a-z0-9._-]*`.
Duplicate ids, dangling `default_model`, unknown keys, and invalid
`schema_version` are rejected with a file:line:column message.

Validate a file without loading a model:

```bash
pumice-transcribe check-config
pumice-transcribe check-config --config /path/to/transcribe.yml --json
```

## Commands

### `file`

```bash
# Use a configured model id
pumice-transcribe file audio.wav --model tiny

# Use a direct GGUF path (no config needed)
pumice-transcribe file audio.wav --model-file /path/to/whisper-tiny-Q8_0.gguf

# Use the configured default_model
pumice-transcribe file audio.wav

# Override config values for one call
pumice-transcribe file audio.wav --model turbo --language en --threads 4 --json
```

`--model ID` and `--model-file PATH` are mutually exclusive. If neither is
given, the configured `default_model` is used; if there is none, the command
fails.

### `models list`

```bash
pumice-transcribe models list
pumice-transcribe models list --json
```

Lists configured models with their resolved path, whether it is a regular file,
its size in bytes when it is a regular file, and which model is the default.
Missing paths and directories have `exists: false` and no `size` field in JSON.
No hashing or network access is performed.

### `serve`

```bash
pumice-transcribe serve --config ./transcribe.yml --model turbo --port 17568
```

Runs an OpenAI-compatible HTTP server on `127.0.0.1:PORT`. The bind address is
always `127.0.0.1` and is not configurable. The port can also be set as
`server.port` in `transcribe.yml`; `--port` overrides it. Port `0` is allowed
and the actual bound address is printed on startup.

`serve` loads one model at startup and keeps it in memory. The model is
selected with `--model ID`; if omitted, the configured `default_model` is used.
Startup load failure exits non-zero before the server starts listening.

Only one transcription runs at a time. A second request is rejected
immediately with HTTP 429 and no queue. `/v1/models` and `/health` stay
responsive while a transcription runs.

If the client disconnects, or the request hits its deadline or a shutdown, the
transcription is cancelled and its result is discarded. The server stays busy
until the engine actually stops. Whisper checks for cancellation between
decoding steps, not inside its encoder, so there is no fixed bound: with Turbo
on server-1's CPU (i5-14500T, portable build) a cancelled 14-second clip kept
the server busy for 26 to 32 seconds, and a shutdown waited as long.

#### Endpoints

- `GET /health` — liveness. Returns `200 application/json`:
  ```json
  { "status": "ok", "version": "0.2.0" }
  ```

- `GET /v1/models` — lists only the served model id in the OpenAI list shape:
  ```json
  {
    "object": "list",
    "data": [
      { "id": "turbo", "object": "model", "created": 0, "owned_by": "pumice" }
    ]
  }
  ```

- `GET /v1/models/{model}` — the served model's object, or 404
  `model_not_found` for any other id.

- `POST /v1/audio/transcriptions` — multipart/form-data transcription.

  Required fields:
  - `file`: exactly one uploaded audio file (a file part, not a text field),
    non-empty. Its file name and declared content type are ignored; the format
    is detected from the bytes.
  - `model`: must equal the served model id exactly; any other id gets 404
    `model_not_found`.

  Optional fields:
  - `language`: ISO-639-1 two-letter code (e.g. `pt`, `en`). Overrides the
    configured default language hint.
  - `response_format`: `json` (default) or `text`.
  - `prompt`: client-supplied context, passed unchanged to the engine.
  - `temperature`: finite number in `[0, 1]`.
  - `stream`: must be absent or `false`; `true` is rejected.

  Any other field, duplicate field, timestamp option, or unsupported
  `response_format` is rejected with HTTP 400.

  Responses:
  - `json`: `200 application/json` with `{"text":"..."}`. When the engine
    returns no text, the answer is `{"text":""}`. There is no silence detector:
    Whisper can still produce text for very short silent clips.
  - `text`: `200 text/plain; charset=utf-8` with the exact transcript bytes.

  Limits: 25,000,000 uploaded file bytes, 26,000,000 total body bytes,
  ≤16 KiB per text field, each parameter at most once, 120 seconds of decoded audio, 15
  seconds to receive the body and 120 seconds for the whole response, both
  counted from the request's arrival. The body size limit covers the whole
  body, chunked or not, including bytes after the closing boundary.

  Errors use the OpenAI error object, with `param` naming the offending
  parameter when it is one this server accepts, and `code` set only where
  OpenAI defines one. Text parameters must be valid UTF-8 without NUL
  characters.
  ```json
  { "error": { "message": "...", "type": "invalid_request_error", "param": "file", "code": null } }
  ```
  - `400` `invalid_request_error`: missing, unknown or duplicate parameters,
    malformed multipart, unsupported options, undecodable audio, unsupported
    audio format (as OpenAI does), audio longer than 120 seconds.
  - `404` `invalid_request_error`: model not served (`code: model_not_found`)
    or unknown URL.
  - `405` `invalid_request_error`: wrong HTTP method.
  - `408` `invalid_request_error`: body not received in time.
  - `413` `invalid_request_error`: body, file or text field too large.
  - `429` `rate_limit_error` (`code: rate_limit_exceeded`, `Retry-After: 1`):
    another transcription is running.
  - `500` `server_error`: the transcription failed.
  - `503` `server_error`: the server is shutting down.
  - `504` `server_error`: the response deadline passed.

  Official OpenAI SDKs raise their typed errors for these (`BadRequestError`,
  `NotFoundError`, `RateLimitError`, and so on). Messages are fixed text: they
  never include what the client sent (model names, field names, file names,
  the prompt), the transcript, or decoder and engine messages.

```bash
curl --fail-with-body --max-time 130 \
  http://127.0.0.1:17568/v1/audio/transcriptions \
  -F 'file=@clip.wav' -F 'model=turbo' -F 'language=pt'

curl --fail-with-body --max-time 130 \
  http://127.0.0.1:17568/v1/audio/transcriptions \
  -F 'file=@clip.wav' -F 'model=turbo' -F 'response_format=text'

curl --fail http://127.0.0.1:17568/v1/models
curl --fail http://127.0.0.1:17568/health
```

## Supported formats and limits

Formats are detected by content (magic bytes), not by file extension.

| Format | Container/codec | Limits |
| --- | --- | --- |
| WAV | RIFF/WAVE PCM / IEEE float / extensible | Max file 25 MB, max audio 120 s, sample rate 8–192 kHz, mono or stereo |
| MP3 | MPEG Audio Layer III | Same limits |
| FLAC | Free Lossless Audio Codec | Same limits |
| Ogg Vorbis | Ogg + Vorbis (one logical stream) | Same limits |
| Ogg Opus | Ogg + Opus | `opus` Cargo feature; mono/stereo, one logical stream; same limits |
| WebM/Opus | WebM or Matroska + Opus | `opus` Cargo feature; mono/stereo, one audio track, unlaced blocks; same limits |
| MP4/AAC | — | Not supported |

Opus decoding is compiled behind the `opus` Cargo feature: the CLI's
`engine` feature enables it transitively, and a library build with only
`opus` already decodes these formats. Builds without the feature reject
Opus files with `unsupported_format`. Opus always decodes at 48 kHz and is
resampled to 16 kHz like any other input.

Known limit: a truncated file is rejected only when its container proves
completeness — a strict container check does this for WebM (every element
extent is validated against the actual file size) and for Ogg (per-page
checksums and the end-of-stream marker). A plain MP3 without a declared
frame count has no reliable end marker, so a cut-off file decodes as a
shorter recording instead of failing. As with any compressed format, a
WebM Opus stream recorded live (no duration, no cues) cannot prove that
whole final blocks were not lost at a clean boundary.

## JSON output and exit codes

With `--json`, stdout is a single JSON envelope:

```json
{
  "schema_version": 1,
  "ok": true,
  "result": { "text": "...", "language": "pt", ... }
}
```

Failures use the same envelope with `ok: false` and `error.code`:

| Code | Meaning | Exit status |
| --- | --- | --- |
| `invalid_arguments` | Bad CLI arguments or flags | 2 |
| `invalid_config` | Config file error, including syntax, unknown keys, or validation | 2 |
| `invalid_input` | Audio file missing, not a regular file, or decode error | 2 |
| `input_too_large` | Audio file over 25 MB | 2 |
| `audio_too_long` | Audio over 120 s | 2 |
| `unsupported_format` | Unknown format or unsupported codec | 2 |
| `model_not_found` | Model file does not exist | 2 |
| `engine_not_available` | Build has no transcription engine | 1 |
| `engine_failed` | Engine error during transcription | 1 |
| `cancelled` | Transcription cancelled | 1 |
| `model_load_failed` | Failed to load the model | 1 |
| `internal` | Unexpected internal error | 1 |

Config errors include `error.location.file`, `line`, and `column` when the
position is known.

## Common errors

- **"no model selected; use --model ID, --model-file PATH, or set
default_model"**: you did not pass `--model` or `--model-file`, and there is
no `default_model` in the config.
- **"unknown model id ...; configured ids: ..."**: the id passed to `--model`
is not in the config `models` list.
- **"schema_version is required"**: the config file is missing `schema_version:
1`.
- **"unknown setting `foo`"**: the config contains a key that is not part of
the schema. Check the file line in the error message.
- **"model file not found"**: the resolved model path does not point to a
regular file.
