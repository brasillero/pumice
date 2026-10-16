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

Linux x86-64, opt-in (x86-64 Windows: see the Vulkan preview below). The
default build above is a single static binary with a portable x86-64 baseline;
on modern CPUs it leaves a lot of transcription speed unused. The `cpu-variants` Cargo feature instead links the engine as
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
CLI. It prints the selected CPU module on stderr at startup, and the
[compute snapshot](#compute-backend-cpu-auto-gpu) that `file --json`, `serve`
and managed mode report names it in `cpu_variant`.

The portable build's engine linkage and compiler baseline are unaffected: the
script uses its own `CARGO_TARGET_DIR` and overrides the conservative
`TRANSCRIBE_CMAKE_ARGS` from `.cargo/config.toml`, so nothing in the portable
build changes. (Ordinary `engine` builds report the same compute snapshot,
with `cpu_variant: null`; the binary stays a single static portable
executable.) macOS builds
are unchanged (the feature is off by default; other architectures have no
module mapping). On x86-64 Windows the same mechanism loads
`cpu/<variant>/ggml-cpu-<variant>.dll` from the nine variants an MSVC build
produces; it is only built as part of the Vulkan preview below.

Measurements for both builds on server-1 and the details of the mechanism are
in [`docs/research/P7-cpu-variants.md`](../docs/research/P7-cpu-variants.md).

## Windows Vulkan preview (experimental)

**Not a release.** A preview of GPU transcription on Windows x64, for checking
on real hardware. CI builds it in the `Transcription engine` workflow and
uploads it as the short-lived artifact
`pumice-transcribe-windows-x64-vulkan-preview`; no release ships it.

The artifact is one folder; keep it together:

```text
pumice-transcribe.exe
transcribe.dll, ggml.dll, ggml-base.dll   engine core, beside the exe
cpu/<variant>/ggml-cpu-<variant>.dll      runtime CPU variants (9)
gpu/vulkan/ggml-vulkan.dll                optional Vulkan module
bundle.json                               manifest: every file's size and SHA-256
```

Prerequisites: a GPU driver with Vulkan support (it provides `vulkan-1.dll`).
Nothing else: no Vulkan SDK, Visual C++ runtime, Rust or build tools. CI
checks that every file uses the static C runtime and that only
`gpu/vulkan/ggml-vulkan.dll` imports Vulkan, so the CPU path also starts on a
machine without a Vulkan driver.

Supported on Windows 10 or later, x64. DLLs load only from the folder itself
and from `System32`, never from the current directory or `PATH`: the
executable delay-loads `transcribe.dll` and first limits the DLL search with
`SetDefaultDllDirectories`, which also covers the CPU and Vulkan modules'
own dependencies. `vulkan-1.dll` therefore comes from the driver in
`System32` or not at all, and a missing core DLL stops startup instead of
being looked up elsewhere. The folder's own DLLs are trusted, so keep it
where only you can write. CI checks this with decoy DLLs in the current
directory and on `PATH`.

Before loading any engine module, startup checks the core DLLs, the selected
CPU module and, unless the backend is `cpu`, the Vulkan module against
`bundle.json` (sizes and SHA-256, about 25–100 ms for the 43 MB Vulkan module
depending on the CPU). A missing
manifest, a damaged or replaced file, or an unlisted file inside `cpu/<variant>/`
or `gpu/vulkan/` stops startup with an installation error that names the
file and `(bundle manifest mismatch)`; reinstall the whole folder. Deleting
`gpu/` is allowed and gives a CPU-only install (`auto` then reports
`gpu_module_absent`); a Vulkan module that loads but finds no device (or no
Vulkan driver) leaves `auto` on the CPU too.

At startup the CPU variant is chosen as described above. With `backend: auto`
or `gpu`, `gpu/vulkan/` is then scanned with the backend mask fixed to CPU and
Vulkan (plugins named by `GGML_BACKEND_PATH` are never loaded); with
`backend: cpu`, the default, the GPU module is never checked or loaded, so a
broken GPU driver cannot affect it. See
[Compute backend](#compute-backend-cpu-auto-gpu) for the setting and the
[real-GPU checklist](#checking-a-real-gpu-on-windows).

Build it yourself (Windows, Visual Studio 2022 C++ tools and the Vulkan SDK
1.4.350.0 at build time only):

```powershell
pwsh crates/transcription/scripts/build-windows-vulkan.ps1
# result: target\windows-vulkan\stage\
```

## Compute backend (`cpu`, `auto`, `gpu`)

`backend:` in `transcribe.yml` chooses what the model may run on; `--backend`
on `file`, `serve` and `backends` overrides it for one run. There is no
device index, no per-request choice and no backend setting in
`orchestrator.yml`: `pumice serve`'s child reads it from `transcribe.yml`.

| Mode | GPU module | Model loads on | Without a usable GPU |
| --- | --- | --- | --- |
| `cpu` (default) | never checked or loaded | the CPU | — |
| `auto` | checked and scanned | a GPU when one initializes (discrete before integrated), else the CPU | the CPU, with a `fallback_reason` |
| `gpu` | checked and scanned | a GPU | error `gpu_unavailable`, never the CPU |

Only the Windows Vulkan preview has a GPU module. In every other build (the
default `engine` build, Linux `cpu-variants`, macOS) `auto` runs on the CPU
with `fallback_reason: "gpu_not_built"` and `gpu` fails.

`fallback_reason` (only when `auto` runs on the CPU):

- `gpu_not_built` — this build has no GPU module;
- `gpu_module_absent` — the build supports one, but `gpu/vulkan/` is not
  installed next to the executable;
- `gpu_unavailable` — the module registered no GPU device, or no GPU device
  initialized. The engine cannot tell a missing Vulkan loader, a missing
  driver, an excluded device or a failed initialization apart, so neither
  does this reason.

`gpu` fails before loading the model with `gpu_unavailable` (exit status 1)
and one of three fixed messages: the build has no GPU support, no GPU module
is installed, or no usable GPU device registered or initialized. Each one
points to `backend: cpu`.

**The compute snapshot.** After the model loads, one snapshot is taken from
the device the engine actually loaded it on, not from the request, and every
report shows that same object:

```json
{"requested":"auto","backend":"vulkan","device":{"name":"Vulkan0","description":"NVIDIA GeForce RTX 4070"},"cpu_variant":"haswell","fallback_reason":null}
```

- `requested`: `cpu`, `auto` or `gpu`;
- `backend`: `cpu` or `vulkan`, the device's own kind;
- `device`: the engine's name (`CPU`, `Vulkan0`, …) and description (the
  CPU or GPU model);
- `cpu_variant`: the loaded ggml CPU module in `cpu-variants` builds, else
  `null`; on a GPU it is the CPU helper, not the primary device;
- `fallback_reason`: as above, else `null`.

Hardware strings are trimmed, control characters become spaces, names and
`cpu_variant` are cut to 128 bytes and descriptions to 256 bytes (at a
character boundary). The snapshot appears in `file --json` (`result.compute`),
in `serve`'s startup line on stderr (`loaded model … in N ms; compute: {…}`),
in managed `ready` and `GET /ready`, and in `pumice serve`'s
`GET /pumice/v1/status` (`transcription.compute`). The OpenAI
`/v1/audio/transcriptions` response does not change.

**Failures** (p14.1-windows-vulkan/D4, narrowed by p14.2-backend-policy/D1):

- A model that fails to load with `auto` or `gpu` on a GPU fails startup with
  `model_load_failed` and a message pointing to `backend: cpu`. It is not
  retried on the CPU: a GPU allocation failure after the device initialized
  is not a fallback case.
- A request the engine refuses because of its content or input (output
  truncated or repeating before the end, an invalid or unsupported request,
  input too long for the model) answers with the usual error
  (`engine_failed`; HTTP 500 `transcription_failed`) and leaves the process
  serving, on a GPU too.
- Any other native engine failure during a transcription on a GPU (out of
  memory, a backend error, an unclassified failure) may leave the device or
  engine unusable. It answers that request with the same error, is never
  retried or replayed, and retires the process: before the next request can
  be admitted it stops accepting work (new requests and `/ready` answer
  503), then it drains and exits with status 1. This holds even if the
  client already disconnected. Standalone `serve` prints why on stderr;
  managed mode writes `stopping` with reason `engine_failed`, then
  `stopped`, so `pumice serve` restarts it within its bounded restart
  policy. Cancellation (a disconnect, a deadline, a shutdown) and audio
  input errors never retire, and on the CPU nothing retires.
- A crash or hang inside the native engine is handled by the process
  supervision that already exists (managed mode's shutdown watchdog,
  `pumice serve`'s liveness probes and restarts); there is no in-process
  driver reset.

### `backends`

```bash
pumice-transcribe backends --backend auto [--json]
```

Registers the compute backends exactly as a model load in that mode does,
without loading a model, and reports what the bundle ships and what
registered:

```json
{"schema_version":1,"ok":true,"result":{"requested":"auto","gpu_build":"vulkan","bundle":"verified","cpu_variant":"alderlake","gpu_module":"scanned","gpu_devices":0,"allowed_backends":["cpu","vulkan"],"devices":[{"name":"CPU","description":"Intel(R) Core(TM) i5-14500T","kind":"cpu","type":"cpu"}],"fallback_reason":"gpu_unavailable"}}
```

`gpu_build` is `vulkan` or `null`; `bundle` is `verified`, `absent` (no
manifest, Linux only) or `null` (no runtime modules); `gpu_module` is
`not_built`, `not_requested` (`cpu` mode), `absent` or `scanned`;
`gpu_devices` counts registered GPU devices; `allowed_backends` is the
engine's backend mask (static builds report the engine's default mask, with
only the CPU compiled in); `fallback_reason` is set when `auto` will run on
the CPU whatever happens at load. A registered GPU can still fail to
initialize when the model loads; only the loaded snapshot is certain.
`backends` exits 0 in `gpu` mode without a GPU (`gpu_devices: 0`); `file`
and `serve` are what refuse. Installation errors (a damaged bundle) fail as
they would at startup.

### Checking a real GPU on Windows

The owner's check of a preview artifact on a Windows machine with a GPU. Use
the same model, audio and language for every run (for example
`whisper-tiny-q8_0`, then Turbo, and the repository's `sample-pt.wav` with
`--language pt`). Nothing is installed; only the extracted folder is used.

1. Extract the artifact to a new folder (try a path with a space) and open a
   terminal there.
2. `pumice-transcribe.exe backends --backend auto --json`: `gpu_module`
   `scanned`, `gpu_devices` ≥ 1 and the GPU listed with kind `vulkan`. Run
   with `--backend cpu` too: `gpu_module` `not_requested`, only the CPU.
3. CPU baseline: `pumice-transcribe.exe file sample-pt.wav --model-file
   MODEL --language pt --backend cpu --json`, three times; note the text,
   `model_load_ms`, `transcription_ms` and `compute`.
4. Required GPU: the same with `--backend gpu`. `compute.backend` must be
   `vulkan` and `compute.device` your GPU; compare the text with the CPU run
   (identical text is not guaranteed) and the timings (first run cold, then
   warm; a longer clip, a short one and silence).
5. `--backend auto` on the same machine: the same device as step 4, with
   `fallback_reason: null`.
6. `serve --backend gpu --port 17568`: the startup line shows the GPU
   snapshot; send a few requests, stop it with Ctrl-C.
7. Without a usable GPU (disable the GPU in Device Manager, or a machine or
   VM without a Vulkan driver): `--backend auto` runs on the CPU with
   `fallback_reason: "gpu_unavailable"`, `--backend gpu` fails with
   `gpu_unavailable`, `--backend cpu` is unaffected.
8. Report the GPU model and driver version, the snapshots, the timings and
   anything unexpected (crashes, hangs, wrong text, high memory or VRAM).

## Getting models

`pumice-transcribe` needs a Whisper GGUF model. The built-in catalog carries
the two models the owner uses, pinned by full repository revision, byte size
and SHA-256:

| Catalog ID | Model | Source | Revision | SHA-256 |
| --- | --- | --- | --- | --- |
| `whisper-large-v3-turbo-q8_0` | `whisper-large-v3-turbo-Q8_0.gguf` | `handy-computer/whisper-large-v3-turbo-gguf` | `5eaf945c7978e564bae5b28a5b1639dd93c2bfb1` | `b2e30cc286bc9f3aba4db9099fc7403543497c05ce7100d0d83091ddfd25a183` |
| `whisper-tiny-q8_0` | `whisper-tiny-Q8_0.gguf` | `handy-computer/whisper-tiny-gguf` | `6687f30c99641ee265df421e582354adbc8848fc` | `325b9c7997cd1eff81ef709d55766565e71be696130cc3a3d444713798706834` |

The weights come from OpenAI's Whisper models: `whisper-tiny` is Apache-2.0
and `whisper-large-v3-turbo` is MIT, per their Hugging Face model cards.
`tiny` is fast and rough; `turbo` is slower and usually more accurate.

Models from outside the catalog stay supported: download them any way you
like, verify the bytes yourself and register a local `path:` entry.

## Downloading models

Model downloads are explicit: nothing is fetched at startup, by a request,
or by `serve`. The only outbound traffic this module ever initiates is a
`models download` of a catalog-pinned file.

```bash
# What can be downloaded and what is already installed
pumice-transcribe models catalog
pumice-transcribe models catalog --json

# Download (or confirm) a catalog model
pumice-transcribe models download whisper-tiny-q8_0
pumice-transcribe models download whisper-tiny-q8_0 --json

# Re-hash an installed model against the catalog
pumice-transcribe models verify whisper-tiny-q8_0
pumice-transcribe models verify whisper-tiny-q8_0 --json
```

Downloads stream into `<file name>.part` next to the final file, hashing
while writing. The staging file and the per-model lock are created private
to the user (on Unix, mode 0600). The models directory itself is created
private too (mode 0700, along with any parents the download creates), and a
download refuses to run into an existing directory that group or others can
write — installation renames by name and is only safe in a directory other
users cannot modify (fix permissions with `chmod 700` or choose another
`models_dir`). The directories above the models directory (the user's home
and data directories) are trusted: the downloader does not validate them, on
the grounds that a writer able to rename those can already replace anything
the user runs. The file is renamed into place only when both the exact byte
size and the catalog SHA-256 match, and the installed file is then reopened
without following links — on Unix with `O_NOFOLLOW`, on Windows by opening
the leaf with `FILE_FLAG_OPEN_REPARSE_POINT` so a symlink or junction is
opened as itself and rejected — and re-read and re-hashed against the
catalog: a swap between hashing and rename removes the installed file and
fails. On Unix the models directory is synced before success is reported, so
the rename survives a power loss; on Windows there is no directory sync —
the rename is the commit point.

On any error (network error, HTTP error status, wrong size, wrong hash,
timeout) the partial file is deleted and nothing is left looking installed.
If the process is killed or the machine loses power mid-download, the
`.part` file stays behind; the next `models download` of the same model
removes it before starting over. There is no resume: an interrupted download
restarts from zero. A `Content-Length` that disagrees with the catalog size
is refused before any byte is stored, and redirects are followed manually —
at most 5, HTTPS targets only. Two concurrent downloads of the same model
fail fast (the second reports "already in progress").

When the file is already installed and verifies, `models download` reports
`already installed` (exit 0) without touching the network. When a file
exists but fails verification (corrupt or modified), the command reports it
and requires `--force` to replace it.

Timeouts: 30 s to connect, and one overall per-download deadline of at
least 10 minutes that grows with the model size
(`max(10 min, size / 256 KiB per s)`). The deadline is a single absolute
budget computed when the download starts: every HTTP request the download
makes — including each redirect hop, which gets only the time remaining, so
a chain of redirects cannot extend the budget — carries the remaining time
as its overall timeout, and it bounds DNS resolution, connecting, receiving
the response headers and reading the body. There is no separate read-stall
limit. The deadline is an HTTP transfer/install-admission budget: disk
writes, the post-install re-hash and the directory sync are not
wall-clock-interruptible, and the budget is checked once more right before
the verified file is renamed into place, so a transfer that only finished
after the deadline is never installed. One caveat: the blocking DNS lookup
itself cannot be interrupted, so a stalled resolver is only stopped once it
returns. The only request header is `User-Agent:
pumice-transcribe/<version>`; no credentials are sent.

Progress (designed for a future UI, `arch-modules/D10`): with `--json`,
progress and state events are JSON Lines on stderr. Byte progress, at most
about 4 per second:

```json
{"schema_version":1,"event":"progress","model":"whisper-tiny-q8_0","bytes":20971520,"total":45981088}
```

Phase transitions, each emitted once before the corresponding phase starts:

```json
{"schema_version":1,"event":"state","model":"whisper-tiny-q8_0","state":"connecting"}
```

`state` is one of `connecting` (before the HTTP request), `downloading`
(streaming bytes), `verifying` (comparing the SHA-256) or `installing`
(renaming into place).

The final result is the usual envelope on stdout. Without `--json`, a simple
overwriting progress line is shown on stderr and no JSONL events are
emitted.

### Where downloaded files go

By default, downloaded models live in the per-user data directory (decision
D2 — downloads never edit the config and never enable a model):

- Linux: `$XDG_DATA_HOME/pumice/models`, else `~/.local/share/pumice/models`
- macOS: absolute `$XDG_DATA_HOME/pumice/models` when set, else
  `~/Library/Application Support/pumice/models`
- Windows: `%LOCALAPPDATA%\pumice\models`

Set `models_dir` in `transcribe.yml` to override it (relative values resolve
against the config file's directory; `~` is not expanded). On Unix the
directory is created mode 0700, and a download refuses an existing directory
that group or others can write — see
[Downloading models](#downloading-models).

### Registering a catalog model

A model entry can use `catalog: <catalog id>` instead of `path:`; exactly one
of the two is required. The entry resolves to the catalog file inside the
models directory:

```yaml
schema_version: 1
models:
  - id: turbo
    catalog: whisper-large-v3-turbo-q8_0
  - id: local
    path: /opt/models/my-model.gguf
```

`check-config` validates catalog ids; `models list` shows which entries use
`catalog:` and whether the file is present.

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
backend: cpu             # optional: cpu (default), auto or gpu
models_dir: /absolute/path/to/models  # optional; where `catalog:` files live
                                      # (see "Downloading models" for the
                                      # default). `~` is NOT expanded here —
                                      # use an absolute path, or a relative
                                      # one (resolved against this file)
models:                  # explicit list; nothing is enabled implicitly
  - id: turbo
    catalog: whisper-large-v3-turbo-q8_0   # downloaded catalog model
  - id: tiny
    path: whisper-tiny-Q8_0.gguf           # local file
```

Relative `path` values and a relative `models_dir` resolve against the
directory that contains the config file. A leading `~` is not expanded by
YAML or by this module — write an absolute path or a relative one. Paths
must be nonempty. Model ids
must match `[a-z0-9][a-z0-9._-]*`. A model entry has exactly one of `path`
(a local GGUF file) or `catalog` (a built-in catalog id, see "Downloading
models"). Duplicate ids, dangling `default_model`, unknown catalog ids,
unknown keys, an unknown `backend` and invalid `schema_version` are
rejected with a file:line:column message.

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

# Choose the compute backend for one call
pumice-transcribe file audio.wav --backend auto --json
```

With `--json`, `result.compute` is the
[compute snapshot](#compute-backend-cpu-auto-gpu) of the loaded model.

`--model ID` and `--model-file PATH` are mutually exclusive. If neither is
given, the configured `default_model` is used; if there is none, the command
fails.

### `models list`

```bash
pumice-transcribe models list
pumice-transcribe models list --json
```

Lists configured models with their resolved path, whether it is a regular
file, its size in bytes when it is a regular file, and which model is the
default. Entries that use `catalog:` instead of `path` show the catalog id
in the `catalog` field (JSON) and resolve their path inside the models
directory. Missing paths and directories have `exists: false` and no `size`
field in JSON. No hashing or network access is performed.

### `models catalog`, `models download`, `models verify`

```bash
pumice-transcribe models catalog [--json]
pumice-transcribe models download <catalog-id> [--json] [--force]
pumice-transcribe models verify <catalog-id> [--json]
```

These manage the built-in pinned model catalog; see
[Downloading models](#downloading-models) for the full behavior (where files
go, verification, progress output, network rules).

In `models catalog`, `installed: yes` means a regular file with the expected
size is present — presence and size only, not integrity (any right-sized
file counts, even with different content). Use `models verify` to re-hash
the installed file against the catalog SHA-256.

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
`--backend` overrides `backend:` in `transcribe.yml`; the startup line on
stderr shows the [compute snapshot](#compute-backend-cpu-auto-gpu). Startup
load failure exits non-zero before the server starts listening. On a GPU, a
native engine failure that may leave the device unusable stops the server
after answering that request (exit status 1); see
[Failures](#compute-backend-cpu-auto-gpu).

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

#### Managed mode (used by `pumice serve`)

Internal protocol, version 2 (version 2 added the compute snapshot and the
`engine_failed` stop). It exists so `pumice serve` can own one warm
`pumice-transcribe` child; it is not meant for direct use and may change with
the protocol version.

```bash
pumice-transcribe --config ./transcribe.yml serve --model turbo --port 0 \
  --managed --instance-id gen-1
```

`--managed` and `--instance-id` go together. The id comes from the owner and
must be 1 to 64 characters from `A-Z a-z 0-9 . _ -`. The served model id
must be at most 512 bytes, so `ready` can carry it unchanged; a longer one
fails before loading with `invalid_config`. Everything else, including
the OpenAI routes above, behaves exactly as without `--managed`.

**stdout** carries only lifecycle records, one JSON object per line, each
under 4096 bytes; human messages stay on stderr. A run writes `ready` or
`error`, then possibly `stopping` and `stopped`:

```json
{"schema_version":1,"event":"ready","module":"transcription","instance_id":"gen-1","protocol_version":2,"version":"0.2.0","host":"127.0.0.1","port":49152,"model":"turbo","capabilities":["openai_transcriptions","detailed_transcriptions","managed_shutdown"],"model_load_ms":362,"compute":{"requested":"cpu","backend":"cpu","device":{"name":"CPU","description":"Intel(R) Core(TM) i5-14500T"},"cpu_variant":null,"fallback_reason":null}}
{"schema_version":1,"event":"stopping","module":"transcription","instance_id":"gen-1","reason":"stdin_closed"}
{"schema_version":1,"event":"stopped","module":"transcription","instance_id":"gen-1"}
```

- `ready` comes only after the model is loaded and the listener is bound, and
  never after `stopping`. `compute` is the
  [compute snapshot](#compute-backend-cpu-auto-gpu); with the longest model
  and instance ids and every snapshot string at its bound, `ready` still
  fits in 4096 bytes.
- `error` replaces `ready` when startup fails: `code` is one of the
  [error codes](#json-output-and-exit-codes) and `message` is cut to 512
  bytes. An invalid `--instance-id` or invalid arguments give
  `invalid_arguments` without `instance_id`. The exit status is non-zero.
- `stopping` has `reason` `stdin_closed`, `signal` or `engine_failed` (a
  native failure that may leave the GPU unusable retired the instance; it then writes
  `stopped` and exits with status 1).

**stdin** is the owner's lifeline: end of file, or a read error, starts a
shutdown; anything written to it is ignored. It is watched from the start,
before the model loads. A shutdown (from stdin or Ctrl-C/SIGTERM) stops new
work, answers a running request with 503 and cancels its transcription. If
the server has not drained 2 seconds after the request, for example because
the engine is between cancellation checks or the model is still loading, the
process exits at once with status 3; if the thread that enforces this cannot
be started, it exits with status 3 right away, without `stopping`. Otherwise
it writes `stopped` and exits with status 0. Ctrl-C/SIGTERM are handled this
way once the model is loaded; during loading they end the process at once, as
without `--managed`.

Two more routes exist only in managed mode (404 otherwise). Both accept only
the `Host` `127.0.0.1:<port>` (no `localhost` or other names) and refuse any
request with an `Origin` header, with 403 `host_not_allowed` or
`origin_not_allowed`, whatever the method; past those checks, an unsupported
method gets 405 `method_not_allowed` with an `Allow` header. Their errors use
the OpenAI error object with `code` always set (for example
`rate_limit_exceeded`, `missing_model`, `unsupported_format`,
`audio_too_long`, `body_too_large`, `shutting_down`, `deadline_exceeded`,
`transcription_failed`).

- `GET /ready` — `200` while the instance accepts work (a running
  transcription does not change that); `503` `shutting_down` once stopping:
  ```json
  {"schema_version":1,"module":"transcription","instance_id":"gen-1","protocol_version":2,"version":"0.2.0","model":"turbo","capabilities":["openai_transcriptions","detailed_transcriptions","managed_shutdown"],"compute":{"requested":"cpu","backend":"cpu","device":{"name":"CPU","description":"Intel(R) Core(TM) i5-14500T"},"cpu_variant":null,"fallback_reason":null},"state":"ready"}
  ```
  `compute` is the same snapshot as in `ready`.
- `POST /pumice/v1/transcriptions` — the same multipart fields, limits,
  deadlines and single-transcription admission as `/v1/audio/transcriptions`
  (the two routes share it: either one busy gives 429), except that
  `response_format` can only be `json`. The answer is a versioned result:
  ```json
  {"schema_version":1,"text":"...","language":"pt","audio_seconds":2.1,"inference_ms":5800}
  ```
  `language` is the hint the engine was given, or the detected language, and
  is left out when neither is known. `inference_ms` is the engine call alone,
  without reading, decoding or resampling the audio.

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
| `gpu_unavailable` | `backend: gpu` without a GPU build, GPU module or usable GPU device | 1 |
| `checksum_mismatch` | A downloaded or installed model file failed size or SHA-256 verification (see `--force`) | 1 |
| `download_failed` | Network or HTTP failure during `models download` | 1 |
| `internal` | Unexpected internal error | 1 |

Config errors include `error.location.file`, `line`, and `column` when the
position is known.

`serve --managed` uses exit status 3 when its shutdown grace runs out (see
[Managed mode](#managed-mode-used-by-pumice-serve)). `serve` exits with
status 1 after a native engine failure that retires a GPU process.

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
regular file. For a `catalog:` entry the error names the fixing command,
e.g. `pumice-transcribe --config FILE models download whisper-tiny-q8_0`
(`--config` is kept when you passed one, because a `models_dir` override
only applies through that config file).
- **"backend must be one of cpu, auto, gpu"**: the config's `backend` is not
  one of the three modes.
- **"backend gpu needs a GPU build ..." / "backend gpu found no GPU module
  ..." / "backend gpu found no usable GPU device ..."** (`gpu_unavailable`):
  `gpu` mode cannot get a GPU. Use `backend: cpu`, or `auto` to fall back to
  the CPU.
- **"failed to load model with backend auto|gpu; a GPU load failure is not
  retried on the CPU ..."** (`model_load_failed`): the model could not be
  loaded (for example not enough GPU memory, or a damaged model file). Try
  `backend: cpu`.
- **"unknown catalog model ..."**: the id passed to `models download`,
  `models verify` or a config `catalog:` entry is not in the built-in
  catalog; run `pumice-transcribe models catalog`.
- **"... does not match the catalog entry (corrupt or modified); re-run with
  --force"**: a file is already installed under the catalog's file name but
  its size or SHA-256 differs. The file is left untouched; pass `--force` to
  replace it.
