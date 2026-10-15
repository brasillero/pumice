# pumice-transcribe

`pumice-transcribe` is Pumice's local speech-to-text module. It takes an audio
file and a Whisper GGUF model and returns the raw transcript. It is a
standalone CLI; it does not talk to the relay or to any AI provider.

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

## Supported formats and limits

Formats are detected by content (magic bytes), not by file extension.

| Format | Container/codec | Limits |
| --- | --- | --- |
| WAV | RIFF/WAVE PCM / IEEE float / extensible | Max file 25 MB, max audio 120 s, sample rate 8–192 kHz, mono or stereo |
| MP3 | MPEG Audio Layer III | Same limits |
| FLAC | Free Lossless Audio Codec | Same limits |
| Ogg Vorbis | Ogg + Vorbis (one logical stream) | Same limits |
| Ogg Opus | Ogg + Opus | Rejected (`unsupported_format`) — no Opus decoder yet |

Known limit: a truncated file is rejected only when its header declares a
frame count (WAV, FLAC, and MP3 with a Xing/Info or VBRI header). A plain MP3
without such a header has no reliable end marker, so a cut-off file decodes as
a shorter recording instead of failing.

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
