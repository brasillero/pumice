# pumice-transcribe

`pumice-transcribe` is Pumice's local speech-to-text module. It takes an audio
file and a Whisper GGUF model and returns the raw transcript. It is a
standalone CLI; it does not talk to the relay or to any AI provider.

When the transcription program is installed next to the `pumice`
executable, every command below also works through the dispatcher with a
`pumice transcribe` prefix — for example `pumice transcribe --config
./transcribe.yml file audio.wav --model whisper-tiny-q8_0` — with arguments, output and exit status passed through
unchanged. The dispatcher never searches `PATH` or the working directory;
it only runs the `pumice-transcribe` sitting in the real `pumice`
executable's own install directory (a symlinked `pumice` still resolves to
its actual install directory).

On Windows, Ctrl-C/Ctrl-Break is delivered to the console, not to a single
process: while the transcription program handles the event and shuts down,
`pumice` keeps waiting and then exits with the same full exit code.

> **npm installs:** the Windows x64, Linux x64 (glibc 2.35 or newer) and
> Apple Silicon platform packages carry the complete bundle described in the
> Windows, Linux and macOS sections below, in
> `node_modules/@brasillero/pumice-<platform>/bin/` next to `pumice`, so
> `pumice transcribe` works after `npm install -g @brasillero/pumice` (see
> the [installation guide](install.md#option-2-package-manager-nodejs-22)).
> The Intel Mac package has `pumice` only. The portable release archives
> also have `pumice` only; with those, or from source, install both programs
> into the same directory, for example `cargo install --path . --locked` and
> `cargo install --path crates/transcription --features engine --locked`.

Before transcribing, copy [`transcribe.example.yml`](../transcribe.example.yml)
to your own `transcribe.yml` and edit the settings. Every `file` invocation,
including `--model-file`, needs a complete configuration. Pumice does not
create it automatically. Pass `--config /path/to/transcribe.yml` or put it
at the [per-user configuration path](#configuration).

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

Linux x86-64, opt-in (x86-64 Windows: see the Vulkan preview below; the
Linux Vulkan preview includes the same variants). The
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
executable.) macOS on Intel
is unchanged (the feature is off by default; other architectures have no
module mapping); Apple Silicon gets runtime modules only in the Metal preview
below, with one portable CPU module instead of variants. On x86-64 Windows the same mechanism loads
`cpu/<variant>/ggml-cpu-<variant>.dll` from the nine variants an MSVC build
produces; it is only built as part of the Vulkan preview below.

Measurements for both builds on server-1 and the details of the mechanism are
in [`docs/research/P7-cpu-variants.md`](../docs/research/P7-cpu-variants.md).

## Windows Vulkan preview (experimental)

**Not a release.** A preview of GPU transcription on Windows x64, for checking
on real hardware. CI builds it in the `Transcription engine` workflow and
uploads it as the short-lived artifact
`pumice-transcribe-windows-x64-vulkan-preview`; no release archive ships it
(the npm package `@brasillero/pumice-win32-x64` carries the same bundle).

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

Supported on Windows 10 or later, x64. The DLLs the program looks up by name
(the bundle's own DLLs and their dependencies, `vulkan-1.dll` included) are
searched only in the folder itself and in `System32`, never in the current
directory or `PATH`: the executable delay-loads `transcribe.dll`, first
limits the DLL search with `SetDefaultDllDirectories`, which also covers the
CPU and Vulkan modules' own dependencies, and after the bundle check loads
`transcribe.dll` by full path with the same restriction. `vulkan-1.dll`
therefore comes from `System32` or not at all, and a core DLL that is
missing or cannot load stops startup with an installation error naming
the file and the Windows error code (`core library unloadable`) instead of
being looked up elsewhere. This is the default for lookups by name; a DLL
loaded by explicit path, or with its own loader flags, is not bound by it.
In particular the GPU driver itself is not looked up by Pumice: the Vulkan
loader finds it through the driver's registration, usually in the Windows
driver store (`System32\DriverStore\FileRepository`). The folder's own DLLs
are trusted, so keep it where only you can write. CI checks this with decoy
DLLs in the current directory and, separately, on `PATH`, for the core DLLs
and for the dependency of a module ggml loads after startup; it changes no
system file, so a machine without a Vulkan loader in `System32` is checked
by hand on a clean machine or VM (step 7 below).

Before loading any engine module, startup checks the core DLLs, the selected
CPU module and, unless the backend is `cpu`, the Vulkan module against
`bundle.json` (sizes and SHA-256, about 25–100 ms for the 43 MB Vulkan module
depending on the CPU). A missing
manifest, a damaged or replaced file, or an unlisted file inside `cpu/<variant>/`
or `gpu/vulkan/` stops startup with an installation error that names the
file and `(bundle manifest mismatch)`; reinstall the whole folder. Deleting
`gpu/` is allowed and gives a CPU-only install (`auto` then reports
`gpu_module_absent`); a Vulkan module that loads but finds no device (or no
Vulkan driver), or only integrated or software GPUs, leaves `auto` on the CPU
too.

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

## Linux Vulkan preview (experimental)

**Not a release.** The same preview for x86-64 Linux with glibc (never
musl). CI builds it in the `Transcription engine` workflow on Ubuntu 22.04
and uploads it as the short-lived artifact
`pumice-transcribe-linux-x64-vulkan-preview`, which holds one tarball (a
tarball keeps the executable bit, which artifact downloads drop) and its
`SHA256SUMS`; no release archive ships it (the npm package
`@brasillero/pumice-linux-x64` carries the same bundle). Extract it anywhere and keep the folder
together:

```text
pumice-transcribe
libtranscribe.so.0.3, libggml.so.0,              engine core, beside the
  libggml-base.so.0                                executable ($ORIGIN rpath)
cpu/<variant>/libggml-cpu-<variant>.so           runtime CPU variants (14)
gpu/vulkan/libggml-vulkan.so                     optional Vulkan module
bundle.json                                      manifest: every file's size and SHA-256
```

Prerequisites:

- glibc 2.35 or newer and the C++ runtime (`libstdc++6` from GCC 12 or
  newer) — Ubuntu 22.04, Debian 12, Fedora 36 or later. CI checks that no
  file needs a newer symbol version and runs the bundle in a plain
  `ubuntu:22.04` container.
- For the GPU only: a Vulkan loader (`libvulkan.so.1`, Debian/Ubuntu package
  `libvulkan1`) and a Vulkan driver: `mesa-vulkan-drivers` for Intel and AMD
  GPUs, the vendor driver for NVIDIA.

Nothing else: no Vulkan SDK, compiler, Rust or `LD_LIBRARY_PATH`. CI checks
every file's dependencies after relocating the folder: only glibc, the C++
runtime and the bundled core libraries, and only `gpu/vulkan/libggml-vulkan.so`
needs `libvulkan.so.1`, so the CPU path starts on a machine without a Vulkan
loader (CI checks that too). The modules find the core library they need
(`libggml-base.so.0`) as the copy the executable already loaded.

`bundle.json`, the startup checks, the `cpu`/`auto`/`gpu` behavior and the
messages are the same as on Windows (see above): the manifest is required,
`gpu/` may be deleted for a CPU-only install, and nothing else may be added
to `cpu/<variant>/` or `gpu/vulkan/`. The core libraries are the
executable's own imports, so the system loader maps them before any check
runs: a core library damaged in a way the loader accepts fails the manifest
check, but one the loader rejects stops the program with the loader's own
`error while loading shared libraries` message (exit status 127). The plain
`cpu-variants` folder from `build-cpu-variants.sh` has no manifest and still
works without one.

Check a machine with `backends`, which loads no model:

```bash
./pumice-transcribe backends --backend auto --json
```

`gpu_module: "scanned"` with `gpu_devices` ≥ 1 and your GPU listed with kind
`vulkan` means `gpu` can use it; `auto` uses it only when its `gpu_class` is
`discrete` (see [Compute backend](#compute-backend-cpu-auto-gpu)). An
integrated GPU (`gpu_class: "integrated"`, `fallback_reason:
"gpu_integrated_only"`) is used only with `backend: gpu`: on the Intel UHD 770
of the owner's test machine it was about 1.7× slower than the CPU for Turbo.
`gpu_devices: 0` with `fallback_reason: "gpu_unavailable"` means no usable
device: install the loader and driver above (`vulkaninfo --summary` from
`vulkan-tools` lists what the loader sees). ggml leaves out software Vulkan
devices such as Mesa's lavapipe (`llvmpipe`) unless `GGML_VK_VISIBLE_DEVICES`
selects them; then they are listed with `gpu_class: "software"` (their
description names `llvmpipe`), `auto` stays
on the CPU with `fallback_reason: "gpu_software_only"` and only `gpu` uses
them. CI uses that to run the tiny model on lavapipe, which proves the module
works but says nothing about speed or real hardware. For a real-GPU check,
follow the [Windows checklist](#checking-a-real-gpu-on-windows) with
`./pumice-transcribe`.

Build it yourself (x86-64 Linux with glibc; CMake, a C++ compiler, `jq`, and
the Vulkan headers, loader library, `glslc` and SPIRV-Headers at build time
only — CI uses LunarG's Vulkan SDK 1.4.350.0):

```bash
crates/transcription/scripts/build-linux-vulkan.sh
# result: target/linux-vulkan/stage/
```

The script also audits every staged file (allowed libraries, `$ORIGIN` as
the only RUNPATH, no glibc symbol newer than 2.35), so on a build machine
newer than Ubuntu 22.04 the audit fails by design.

## macOS Metal preview (experimental)

**Not a release.** The same kind of preview for Apple Silicon Macs
(`aarch64-apple-darwin`); Intel Macs stay CPU-only. CI builds it in the
`Transcription engine` workflow on a GitHub-hosted Apple Silicon runner
(`macos-15`) and uploads it as the short-lived artifact
`pumice-transcribe-macos-arm64-metal-preview`: one tarball (it keeps the
executable bit) and its `SHA256SUMS`; no release archive ships it (the npm
package `@brasillero/pumice-darwin-arm64` carries the same bundle). Extract it
anywhere and keep the folder together:

```text
pumice-transcribe
libtranscribe.0.3.dylib, libggml.0.dylib,        engine core, beside the
  libggml-base.0.dylib                             executable (@rpath, @executable_path)
cpu/arm64/libggml-cpu.so                          the portable arm64 CPU module
gpu/metal/libggml-metal.so                        optional Metal module
bundle.json                                       manifest: every file's size and SHA-256
```

Prerequisites: macOS 11 (Big Sur) or newer on an Apple Silicon Mac (M1 or
later). Nothing else: no Xcode, command line tools, Homebrew, Rust or
`DYLD_*` variables. The Metal shaders are inside `libggml-metal.so` as
source, and the macOS Metal framework compiles them the first time the GPU
module initializes (see the checklist below for that latency); there is no
`.metal` or `.metallib` file. There are no CPU variants: ggml builds one
portable arm64 module, reported as `cpu_variant: "arm64"`.

CI checks every file after relocating the folder: arm64 only, a minimum macOS
of exactly 11.0 (`LC_BUILD_VERSION minos`), dependencies only on `/usr/lib`,
system frameworks (Accelerate for the CPU path; Metal, MetalKit and
Foundation, with Metal and MetalKit for `gpu/metal/libggml-metal.so` alone)
and the bundled core libraries; the executable looks for those only next to
itself (`@executable_path`), and the modules only in the folder above them
(`@loader_path/../..`), never in their own directory. A run of the relocated
folder from a path with spaces and non-ASCII characters, without any `DYLD_*`
variable, may load images only from that folder and the OS.

**Signing and quarantine.** The files carry ad-hoc signatures only (the
linker's; `install_name_tool` re-signs the two modules whose rpath the stager
edits), with no Developer ID and no notarization. macOS marks files from a browser download
with the `com.apple.quarantine` attribute, and Gatekeeper refuses to run
quarantined code that is not notarized. Remove the attribute from the whole
extracted folder once, after checking `SHA256SUMS`:

```bash
shasum -a 256 -c SHA256SUMS
tar -xzf pumice-transcribe-macos-arm64-metal-preview.tar.gz
xattr -dr com.apple.quarantine pumice-transcribe-macos-arm64-metal-preview
```

(`curl` and `gh run download` do not set the attribute; `xattr -lr` shows
whether it is there.) On CI's runner, with Gatekeeper assessments enabled,
`spctl --assess` rejects the executable, yet a copy given a quarantine
attribute by a script still ran from the shell; a browser download on a
desktop Mac is still expected to be blocked (checklist step 1 records it).
CI checks that the copy runs after this command.

`bundle.json`, the startup checks, the `cpu`/`auto`/`gpu` behavior and the
messages are the same as on Windows and Linux: the manifest is required,
`gpu/` may be deleted for a CPU-only install, and nothing else may be added
to `cpu/arm64/` or `gpu/metal/`, except the `.DS_Store` files Finder writes
into folders it shows (neither ggml nor the system loader ever opens them).
The core libraries are the executable's own imports, so dyld maps them before
any check runs: a core library changed in a way dyld accepts fails the
manifest check, but one dyld or the kernel's code-signing check rejects stops
the program before it starts.

Check a Mac with `backends`, which loads no model:

```bash
./pumice-transcribe backends --backend auto --json
```

`gpu_module: "scanned"` with `gpu_devices: 1` and the GPU listed with kind
`metal` (`MTL0`, its description the chip, for example `Apple M2`) means
`gpu` can use it. **`auto` stays on the CPU** (p14.4-macos-metal/D1): the
engine reports a Metal GPU as type `gpu` with no PCI bus id, so it is
`gpu_class: "unverified"`, and `auto` would not pick it until a real Mac
shows Metal is faster than the CPU for Turbo (p14.4-macos-metal#1). Set
`backend: gpu` to use Metal.

So that `auto` costs nothing for a GPU it never uses, inference with `auto`
does not touch the Metal module at all (p14.4-macos-metal/D2): `file`,
`serve` and a managed `serve` register with the CPU-only mask, neither check
nor load `gpu/metal/`, and report `fallback_reason: "gpu_auto_disabled"`.
`backends --backend auto` is a diagnostic: it still scans Metal so the
device is listed, and says what inference does instead with
`auto_uses_gpu_module: false` and `fallback_reason: "gpu_auto_disabled"`.

`gpu` (and the `auto` diagnostic) initialize the Metal module at startup,
and that compiles the embedded shaders; `backend: cpu` and `auto` inference
never load the module. macOS caches the compiled shaders per program
location: on CI's virtual machine the first Metal start from a new folder
took 43–45 s and later starts from the same folder about 0.1 s, while an
`auto` start from a new folder is as fast as a `cpu` one (CI checks it with
a corrupted Metal module, which `auto` must not notice). The cost on real
hardware is part of the checklist below.

GitHub's Apple Silicon runners (CI uses `macos-15`) are virtual machines
without GPU acceleration. Their Metal device, `Apple Paravirtual device`,
registers as `MTL0` and `backend: gpu` runs the tiny model on it, but slowly
(137–141 s for the 14 s sample) and with a degenerate transcript (`ass!!!…`),
so CI proves the build, the relocation, the CPU path, the manifest checks and
that the Metal module loads and executes; it says nothing about real Metal
output or speed. The suspected cause is the paravirtual device, not Pumice
or the engine: llama.cpp saw the same repetitive garbage on this device
(missing simdgroup capabilities) while physical Macs were fine
([ggml-org/llama.cpp#8617](https://github.com/ggml-org/llama.cpp/issues/8617)),
and both llama.cpp's and whisper.cpp's hosted macOS CI build Metal without
running model tests on it. This is not established: CI records, as
diagnostics that never fail the job, the same run without flash attention
(`TRANSCRIBE_NO_FLASH=1`) and ggml's `test-backend-ops` for the ops Whisper
uses on `MTL0` against the CPU, with the capabilities ggml-metal reports.
On the first such run (`macos-15`, transcribe.cpp 3f32fbc) ggml-metal
reported `simdgroup reduction` and `simdgroup matrix mul.` as false, so
`test-backend-ops` found `MUL_MAT`, `FLASH_ATTN_EXT`, `SOFT_MAX` and `NORM`
unsupported on `MTL0` (no failures among the ops it did run), and the text
stayed degenerate without flash attention. Only a real Mac settles it
(p14.4-macos-metal#1).

Build it yourself (an Apple Silicon Mac with the Xcode command line tools,
CMake and `jq`; build time only):

```bash
crates/transcription/scripts/build-macos-metal.sh
# result: target/macos-metal/stage/
```

The script builds with `MACOSX_DEPLOYMENT_TARGET=11.0`, rewrites the
modules' rpath (re-signing them ad hoc if the edit left no valid signature)
and audits every staged file with `otool`, `lipo` and `codesign`; `--self-test` checks the audit rules alone,
on any OS.

## Compute backend (`cpu`, `auto`, `gpu`)

`backend:` in `transcribe.yml` chooses what the model may run on; `--backend`
on `file`, `serve` and `backends` overrides it for one run. There is no
device index, no per-request choice and no backend setting in
`orchestrator.yml`: `pumice serve`'s child reads it from `transcribe.yml`.

| Mode | GPU module | Model loads on | Without a usable GPU |
| --- | --- | --- | --- |
| `cpu` (default) | never checked or loaded | the CPU | — |
| `auto` | checked and scanned (Vulkan); never checked or loaded with Metal (D2) | the first discrete GPU, else the CPU (always the CPU with Metal) | the CPU, with a `fallback_reason` |
| `gpu` | checked and scanned | a GPU of any class (the engine's choice: discrete before integrated) | error `gpu_unavailable`, never the CPU |

With Metal (Apple Silicon), inference with `auto` registers the CPU only and
reports `fallback_reason: "gpu_auto_disabled"` (p14.4-macos-metal/D2); only
the `backends --backend auto` diagnostic scans the Metal module.

Only the Vulkan previews (Windows, Linux) and the macOS Metal preview have a
GPU module. In every other build (the default `engine` build, Linux
`cpu-variants`, macOS on Intel) `auto` runs on the CPU with
`fallback_reason: "gpu_not_built"` and `gpu` fails.

**Which GPUs `auto` uses** (p14.5-linux-vulkan/D1, D2). `auto` only uses a
GPU it can identify as discrete hardware. Each registered GPU device gets a
class from the engine's device type, its PCI bus id and its description (the
Vulkan device name, or the Metal device name):

| `gpu_class` | When | `auto` | `gpu` |
| --- | --- | --- | --- |
| `integrated` | the engine reports an integrated GPU | skipped | usable |
| `software` | a software rasterizer: the description names `llvmpipe`/`lavapipe` (Mesa), `SwiftShader` or `Microsoft Basic Render` (WARP), even behind a virtual layer; or a device the engine does not report as a GPU | skipped | usable |
| `unverified` | the engine reports a GPU, but it has no PCI bus id, or its description names a virtual or layered implementation: `Venus`/`Virtio`/`virgl` (Mesa's Vulkan over virtio-gpu, `Virtio-GPU Venus (…)`) or `Dozen`/`Microsoft Direct3D12` (Mesa's Vulkan over Direct3D 12) | skipped | usable |
| `discrete` | the engine reports a GPU, it has a PCI bus id and its description names none of the above | used | usable |

The names are matched case-insensitively as substrings. The checks exist
because the engine reports every Vulkan device that is not integrated as a
GPU, including a CPU-type device forced in with `GGML_VK_VISIBLE_DEVICES`,
and because a virtual GPU can report PCI information while it runs on a
software renderer; environment variables therefore never make `auto` use a
software device. A real card whose driver lacks the optional
`VK_EXT_pci_bus_info` extension is `unverified`, so `auto` skips it; use
`backend: gpu` for it. `auto` loads exactly the first discrete device in
registry order, even when another device registered first. Integrated GPUs
are skipped because they are usually slower than a modern CPU for this work
(about 1.7× slower for Turbo on an Intel UHD 770); set `backend: gpu` to use
one. Metal devices never report a PCI bus id, so on Apple Silicon the GPU is
`unverified` and `auto` could never pick it (p14.4-macos-metal/D1);
inference with `auto` therefore does not load the Metal module and reports
`gpu_auto_disabled` (D2).

`fallback_reason` (only when `auto` runs on the CPU):

- `gpu_not_built` — this build has no GPU module;
- `gpu_module_absent` — the build supports one, but `gpu/vulkan/` (Metal:
  `gpu/metal/`) is not installed next to the executable;
- `gpu_unavailable` — the module registered no GPU device. The engine cannot
  tell a missing Vulkan loader, a missing driver, an excluded device or a Mac
  without a Metal device apart, so neither does this reason;
- `gpu_integrated_only` — no discrete GPU, but an integrated one (others
  may be there too); `backend: gpu` uses it;
- `gpu_unverified` — no discrete or integrated GPU, but a GPU that could not
  be verified as discrete hardware (`gpu_class: "unverified"`, software ones
  may be there too); `backend: gpu` uses it;
- `gpu_software_only` — the only GPU devices are software rasterizers
  (`gpu_class: "software"`); `backend: gpu` uses them;
- `gpu_auto_disabled` — this build's `auto` does not use its GPU module, so
  it was not loaded (Apple Silicon, p14.4-macos-metal/D2); `backend: gpu`
  uses Metal.

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
- `backend`: `cpu`, `vulkan` or `metal`, the device's own kind;
- `device`: the engine's name (`CPU`, `Vulkan0`, `MTL0`, …) and description
  (the CPU or GPU model);
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
  is not a fallback case. With `auto` this includes a discrete GPU that
  registered but does not initialize, because `auto` loads on that exact
  device. p14.5-linux-vulkan/D3 would let `auto` fall back to the CPU
  (`gpu_unavailable`) when the device fails to initialize before any weights
  are allocated, but only if the engine can tell that apart, and
  transcribe-cpp 0.3.1 cannot: it reports a device that fails to initialize
  with the same `backend` error as an exception thrown while the weights
  are uploaded (for example a failed Vulkan staging-buffer allocation), and
  has no separate call to initialize a device before the model load. Use
  `backend: cpu` on such a machine.
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
registered. The one exception is `auto` with Metal (p14.4-macos-metal/D2):
the diagnostic still scans the Metal module so the device is listed, while
inference with `auto` registers the CPU only; it reports
`auto_uses_gpu_module: false` and `fallback_reason: "gpu_auto_disabled"`.

```json
{"schema_version":1,"ok":true,"result":{"requested":"auto","gpu_build":"vulkan","auto_uses_gpu_module":true,"bundle":"verified","cpu_variant":"alderlake","gpu_module":"scanned","gpu_devices":1,"allowed_backends":["cpu","vulkan"],"devices":[{"name":"CPU","description":"Intel(R) Core(TM) i5-14500T","kind":"cpu","type":"cpu","gpu_class":null},{"name":"Vulkan0","description":"Intel(R) UHD Graphics 770 (ADL-S GT1)","kind":"vulkan","type":"igpu","gpu_class":"integrated"}],"fallback_reason":"gpu_integrated_only"}}
```

`gpu_build` is `vulkan`, `metal` or `null`; `auto_uses_gpu_module` is whether
inference with `auto` may use that GPU module (`false` on Apple Silicon, D2;
`null` without one); `bundle` is `verified`, `absent` (no
manifest, plain Linux `cpu-variants` only) or `null` (no runtime modules); `gpu_module` is
`not_built`, `not_requested` (`cpu` mode), `absent` or `scanned`;
`gpu_devices` counts registered GPU devices of every class; each device's
`type` is the engine's own (`cpu`, `gpu`, `igpu`, `accel`, `unknown`) and
`gpu_class` is the class `auto` decides by (`discrete`, `integrated`,
`unverified`, `software`; `null` for the CPU); `allowed_backends` is the
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
`--language pt`). Copy `transcribe.example.yml` to a complete settings
file, edit it, and set `$config` to its absolute path. Nothing is installed;
only the extracted folder is used.

1. Extract the artifact to a new folder (try a path with a space). Open a
   PowerShell terminal in a separate, new and empty working directory
   outside that folder, and run the program by its absolute path, set once
   as `$exe = 'C:\...\extracted folder\pumice-transcribe.exe'` and called
   as `& $exe ...` in every step below; pass the model and audio files by
   absolute path too. The extracted folder is the program's own folder,
   where DLL lookups are allowed; only a separate working directory shows
   whether the current directory is searched. Use a clean extraction on a
   machine without the Vulkan SDK, and a terminal without `PATH` changes
   (nothing added for Pumice, the SDK or the build tools). Record the
   Windows version (`winver`), the GPU driver version
   (`Get-CimInstance Win32_VideoController | Select-Object Name,
   DriverVersion`) and the Vulkan loader version
   (`(Get-Item $env:SystemRoot\System32\vulkan-1.dll).VersionInfo.FileVersion`).
2. `& $exe backends --backend auto --json`: `gpu_module`
   `scanned`, `gpu_devices` ≥ 1 and the GPU listed with kind `vulkan`.
   `fallback_reason` depends on the machine: `null` with a dedicated card,
   `"gpu_integrated_only"` with only an integrated GPU, `"gpu_unverified"`
   when the only other GPU is `unverified`. A dedicated card must be
   classified `gpu_class: "discrete"`; if it shows `"unverified"` or
   `"software"`, note its `description` and `type`, and whether
   `--backend gpu` (step 4) works on it. Run with `--backend cpu` too:
   `gpu_module` `not_requested`, only the CPU.
3. CPU baseline: `& $exe --config $config file sample-pt.wav --model-file
   MODEL --language pt --backend cpu --json`, three times; note the text,
   `model_load_ms`, `transcription_ms` and `compute`.
4. Required GPU: the same with `--backend gpu`. It must succeed with
   `compute.backend` `vulkan` and `compute.device` your GPU; compare the text with the CPU run
   (identical text is not guaranteed) and the timings (first run cold, then
   warm; a longer clip, a short one and silence).
5. `--backend auto` on the same machine: on a machine with a discrete GPU,
   `auto` must pick it (`compute.device` the dedicated card, even when the
   machine also has an integrated GPU), with `fallback_reason: null`. On a
   machine with only an integrated GPU, `auto` runs on the CPU with
   `fallback_reason: "gpu_integrated_only"` by design; step 4 then measures
   the integrated GPU, which needs `backend: gpu`.
6. `& $exe --config $config serve --backend gpu --port 17568`: the startup
   line shows the GPU snapshot; send a few requests, stop it with Ctrl-C.
7. Without a usable GPU (disable the GPU in Device Manager, or a machine or
   VM without a Vulkan driver): `--backend auto` runs on the CPU with
   `fallback_reason: "gpu_unavailable"`, `--backend gpu` fails with
   `gpu_unavailable`, `--backend cpu` is unaffected. On a clean machine or
   VM with no `vulkan-1.dll` in `System32` (p14.1-windows-vulkan#2, never
   simulated by removing system files), also put a copy of any
   `vulkan-1.dll` in the working directory from step 1 (never in the
   extracted folder, where it would be loaded by design), then, removed
   from there, in a new directory prepended to `PATH` for that terminal
   only (`$env:PATH = "C:\...\decoy;$env:PATH"`). In both runs `auto`
   must still run on the CPU, and Process Monitor (step 8) must show the
   copy was not loaded.
8. DLL loads: with [Process Monitor](https://learn.microsoft.com/sysinternals/downloads/procmon),
   filter on Process Name `pumice-transcribe.exe` and capture two runs:
   `backends --backend auto --json` and one `file ... --backend gpu`
   (step 4), from the working directory of step 1. Save the events with
   Operation `Load Image` (every DLL loaded) and the DLL lookups (Operation
   `CreateFile`, Path ending in `.dll`, any Result, `NAME NOT FOUND` and
   `PATH NOT FOUND` included). Allowed: images loaded from, and lookups in,
   the extracted folder (its subfolders `cpu\` and `gpu\vulkan\`
   included) and `System32`, plus the GPU driver's own files, which the
   Vulkan loader opens by path from the driver store. Forbidden: any image
   or lookup in the working directory or in a `PATH` directory outside
   `System32` (for example `C:\Windows` itself or a tools folder on `PATH`).
   A lookup that fails in the extracted folder and then succeeds in
   `System32` (for example `vulkan-1.dll`) is the expected order. Report
   any forbidden event with its stack (Event Properties, Stack), which shows
   whether Pumice's files or the driver's made it.
9. Report the Windows, GPU driver and Vulkan loader versions, the GPU
   model, the snapshots, the timings, the Process Monitor captures and
   anything unexpected (crashes, hangs, wrong text, high memory or VRAM).

### Checking real Metal on Apple Silicon

The owner's check of the macOS preview artifact on an Apple Silicon Mac
(p14.4-macos-metal#1). Use the same model, audio and language for every run
(`whisper-tiny-q8_0`, then Turbo, with `sample-pt.wav` and `--language pt`).
Copy `transcribe.example.yml` to a complete settings file, edit it, and set
`config` to its absolute path. Nothing is installed; only the extracted
folder is used.

1. Download the artifact, check `SHA256SUMS` and extract it to a new folder
   (try a path with a space), then remove the quarantine attribute as
   described [above](#macos-metal-preview-experimental). Before removing it
   on one copy, note what running `pumice-transcribe` does while it is still
   quarantined. Work from a new, empty directory outside the folder, call
   the program by its absolute path (`exe="/…/extracted folder/pumice-transcribe"`,
   then `"$exe" …`) and pass the model and audio files by absolute path. Use
   a terminal without `DYLD_*` variables (`env | grep DYLD` prints nothing).
   Record the macOS version (`sw_vers`), the chip (`sysctl -n
   machdep.cpu.brand_string`) and its GPU cores (`system_profiler
   SPDisplaysDataType`), and whether Xcode or its command line tools are
   installed (`xcode-select -p`; a Mac without them also proves that none is
   needed).
2. `"$exe" backends --backend auto --json`: `gpu_module` `scanned`,
   `gpu_devices: 1`, the GPU listed with kind `metal`, type `gpu` and
   `gpu_class: "unverified"`, `auto_uses_gpu_module: false` and
   `fallback_reason: "gpu_auto_disabled"` (D1, D2). With `--backend cpu`:
   `gpu_module` `not_requested`, only the CPU. Time both (`time "$exe"
   backends …`), twice each: the first `auto` diagnostic includes the Metal
   shader compilation, the second may reuse the system's shader cache.
3. CPU baseline: `"$exe" --config "$config" file sample-pt.wav --model-file
   MODEL --language pt --backend cpu --json`, three times; note the text, `model_load_ms`,
   `transcription_ms` and `compute` (`cpu_variant: "arm64"`).
4. Metal: the same with `--backend gpu`, three times. It must succeed with
   `compute.backend` `metal` and `compute.device` `MTL0` with your chip as
   description; compare the text with the CPU run (identical text is not
   guaranteed) and the timings: the first run (cold, shaders compiled), then
   warm runs; a longer clip, a short one and silence. Also note the first-run
   compile latency again after a reboot.
5. `--backend auto`: runs on the CPU with `fallback_reason:
   "gpu_auto_disabled"` by design, and as fast as `--backend cpu` even from
   a new folder (it never loads Metal, D2). If Turbo is clearly faster on
   Metal than on the CPU (warm and cold), say so: that is the evidence D1
   waits for before `auto` may pick Metal.
6. `"$exe" --config "$config" serve --backend gpu --port 17568`: the startup
   line shows the Metal snapshot; send a few requests, stop it with Ctrl-C. While it runs,
   Activity Monitor (Window → GPU History) should show GPU activity during a
   request.
7. Relocation and loads: copy the folder elsewhere and run step 2 from the
   copy with `DYLD_PRINT_LIBRARIES=1` (it only reports): every image must
   come from the copy, `/usr/lib` or `/System`. Then delete `gpu/` from the
   copy: `backends --backend auto` reports `gpu_module` `absent` (still
   `gpu_auto_disabled`), `gpu` fails with `gpu_unavailable`, `cpu` and
   `auto` transcription are unaffected.
8. Report the macOS version, the chip and GPU core count, the snapshots, the
   timings (CPU vs Metal, cold and warm, the first-run compile latency),
   what the quarantined copy did and anything unexpected (crashes, hangs,
   wrong text, high memory).

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
like, verify the bytes yourself and put the `.gguf` file in the
[models folder](#the-models-folder).

## The models folder

The transcription models available are the `.gguf` files in one folder:
`models_dir` in `transcribe.yml`, or with `models_dir: null` the per-user
data directory (see [Where downloaded files go](#where-downloaded-files-go)).
Nothing else lists them: putting a file there makes it available, removing
it makes it unavailable (p17-serve-model-names/D4, D7). Every command that
needs a model reads the folder once when it starts (a snapshot; nothing
watches the folder afterwards, so restart `serve` after changing it).

Each file's **model id**:

- A file whose byte size and SHA-256 match a [catalog](#getting-models)
  entry takes the catalog id, whatever its file name: a downloaded
  `whisper-large-v3-turbo-Q8_0.gguf`, or the same bytes renamed to
  `turbo.gguf`, are both `whisper-large-v3-turbo-q8_0` (D8). Only files
  with a catalog model's size are hashed; hashing a large model takes a
  few seconds, each time a command reads the folder.
- Any other `.gguf` file takes its exact file name without `.gguf`, with no
  lowercasing or other changes: `My Model v2.gguf` is `My Model v2`. Ids
  are compared exactly, so `--model my model v2` does not match.
- A custom file named after a catalog id (`whisper-tiny-q8_0.gguf`)
  keeps its stem but is marked `catalog: false`: only matching bytes
  establish catalog provenance. A file under a catalog *file name*
  (`whisper-tiny-Q8_0.gguf`) with other bytes keeps its file name as its
  id and gets a warning (it may be corrupt; `models verify` checks it).

What counts:

- Only the folder's own files, not subfolders. The `.gguf` extension is
  matched in any letter case (`Model.GGUF` is `Model`).
- Ignored: names starting with `.` (hidden files, macOS `._*` files),
  folders, partial downloads (`*.gguf.part`), download locks
  (`*.gguf.lock`), `SHA256SUMS` and every other extension.
- A symbolic link to a file counts, with the target's bytes (the id comes
  from the link's name, or the target's catalog match). A link to a folder
  is not followed. A broken `.gguf` link, or one to something that is not a
  file, is skipped with a warning.

Errors (the folder cannot be used until they are fixed, and the message
names the files):

- two files with the same id — never resolved by directory order. This
  includes two copies of one catalog model, and on Linux `a.gguf` next to
  `a.GGUF`. On Windows and macOS (case-insensitive by default) two names
  differing only in letter case cannot exist together; on Linux `A.gguf`
  and `a.gguf` are two models, `A` and `a`;
- a file name that gives an empty, non-UTF-8, control-character or
  over-512-byte id.

An empty folder, or a folder that does not exist yet, has no models; it
never blocks `models catalog`, `models download` or `models verify`, and
neither does a `default_model` that is not there. The selected model is
checked when a command uses it, before any audio is transcribed.

`pumice-transcribe models list` shows the folder's models and warnings.

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

When the model is already installed and verifies, `models download` reports
`already installed` (exit 0) without touching the network. That includes a
copy under another name: any `.gguf` file in the folder with the catalog
size and SHA-256 is the model, and a second copy would give its id twice. When a file
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

Downloads go into the [models folder](#the-models-folder), under the
catalog's file name, so a downloaded model is available under its catalog
id; downloads never edit the config. By default the folder is the per-user
data directory:

- Linux: `$XDG_DATA_HOME/pumice/models`, else `~/.local/share/pumice/models`
- macOS: absolute `$XDG_DATA_HOME/pumice/models` when set, else
  `~/Library/Application Support/pumice/models`
- Windows: `%LOCALAPPDATA%\pumice\models`

Set `models_dir` in `transcribe.yml` to a path to override it (`models_dir:
null` keeps this directory; relative values resolve against the config
file's directory; `~` is not expanded). On Unix the
directory is created mode 0700, and a download refuses an existing directory
that group or others can write — see
[Downloading models](#downloading-models).

## Configuration

`pumice-transcribe` reads one configuration file:

1. the file passed with `--config`, or
2. the per-user file:
   - Linux/macOS: absolute `$XDG_CONFIG_HOME/pumice/transcribe.yml`, or
     `$HOME/.config/pumice/transcribe.yml`
   - Windows: `%APPDATA%\pumice\transcribe.yml`

A missing file is an error for the commands that read settings (`file`,
`serve`, `check-config`, `models list`); `models catalog`, `models download`,
`models verify` and `backends --backend MODE` also run without one (the
models folder is then the per-user directory). `file` always needs the
configuration, including with `--model-file PATH`. A missing explicit
`--config` file is always an error, and a file that exists must be complete
for every command.

Every setting must be written: an omitted one is an error with the file and
line (line 1 for a top-level setting, the `server:` mapping for
`server.port`). There are no built-in defaults; start from
[`transcribe.example.yml`](../transcribe.example.yml), which writes them
all. Copy the example to a file you own and edit it before use; Pumice does
not create the configuration automatically. From a repository checkout:

```bash
cp transcribe.example.yml transcribe.yml
# Edit transcribe.yml, then pass --config ./transcribe.yml to each command.
```

In PowerShell, use `Copy-Item transcribe.example.yml transcribe.yml`. You
can instead copy it to the per-user path above and omit `--config`. The
complete schema is:

```yaml
schema_version: 1
default_model: whisper-large-v3-turbo-q8_0  # a model id in the folder; null: none
language: pt             # language hint; null: detect
threads: 8               # 1-32, or auto (available parallelism, capped at 32)
backend: cpu             # cpu, auto or gpu
server:
  port: 17568            # for `serve`; null: `serve` needs --port
models_dir: null         # the models folder; null: the per-user directory
                         # (see "Where downloaded files go"). A path
                         # overrides it; `~` is NOT expanded here — use an
                         # absolute path, or a relative one (resolved
                         # against this file)
```

A relative `models_dir` resolves against the directory that contains the
config file. A leading `~` is not expanded by YAML or by this module —
write an absolute path or a relative one; it must be nonempty.
`default_model` is a model id from the [models folder](#the-models-folder);
it is checked against the folder when a command uses it (`file` or `serve`
without `--model`, and `check-config`), where a missing one is an error at
its line. Unknown keys, an unknown `backend`, an invalid `threads`, a
missing setting and an invalid `schema_version` are rejected with a
file:line:column message.

The `models:` list of earlier versions is gone: the folder replaces it, and
a file that still has `models:` is rejected with a message saying so. To
migrate, move (or link) each listed model file into the models folder, give
non-catalog files the name you want as their id, remove `models:`, and
change `default_model` (and any orchestrator `transcription_model`) to the
ids `models list` shows — a catalog file's catalog id, for example
`whisper-large-v3-turbo-q8_0` instead of `turbo`.

Validate a file and the models folder without loading a model:

```bash
pumice-transcribe --config ./transcribe.yml check-config
pumice-transcribe --config /path/to/transcribe.yml check-config --json
```

## Commands

### `file`

```bash
# Use a model id from the models folder
pumice-transcribe --config ./transcribe.yml file audio.wav --model whisper-tiny-q8_0

# Use a GGUF file anywhere, with the same complete configuration
pumice-transcribe --config ./transcribe.yml file audio.wav --model-file /path/to/whisper-tiny-Q8_0.gguf

# Use the configured default_model
pumice-transcribe --config ./transcribe.yml file audio.wav

# Override config values for one call
pumice-transcribe --config ./transcribe.yml file audio.wav --model whisper-large-v3-turbo-q8_0 --language en --threads 4 --json

# Choose the compute backend for one call
pumice-transcribe --config ./transcribe.yml file audio.wav --backend auto --json
```

With `--json`, `result.model` is the model id and `result.compute` the
[compute snapshot](#compute-backend-cpu-auto-gpu) of the loaded model. For
`--model-file`, the id follows the [models folder's rule](#the-models-folder)
(the catalog id for a catalog model's bytes, else the file name without its
extension), so the file is hashed when it has a catalog model's size.

`--model ID` and `--model-file PATH` are mutually exclusive. If neither is
given, the configured `default_model` is used; if there is none, the command
fails.

Every `file` call reads a complete configuration (`--config FILE` or the
per-user file), even with `--model-file PATH`. The flags override its
`language`, `threads` and `backend` for that call; they do not replace the
configuration. `--model-file` chooses only the model file. You can set
`default_model: null` when selecting a model explicitly for every run.

### `models list`

```bash
pumice-transcribe models list
pumice-transcribe models list --json
```

Lists the [models folder](#the-models-folder): its path, then each model's
id, size in bytes, whether it matched a catalog entry, whether it is the
`default_model`, and its file; warnings (skipped links, a mismatched
catalog file name, a folder that does not exist yet, a `default_model` not
in the folder) go to stderr. Files with a catalog model's size are hashed;
there is no network access. Problems that make the folder unusable
(duplicate or invalid ids) are errors.

```json
{"schema_version":1,"ok":true,"result":{
  "models_dir":"/home/me/.local/share/pumice/models",
  "models":[
    {"id":"My Model","file":"/home/me/.local/share/pumice/models/My Model.gguf","size":147951465,"catalog":false,"default":false},
    {"id":"whisper-large-v3-turbo-q8_0","file":"/home/me/.local/share/pumice/models/whisper-large-v3-turbo-Q8_0.gguf","size":886381760,"catalog":true,"default":true}],
  "diagnostics":[]}}
```

Models are sorted by id. Each diagnostic is `{"file": ..., "message": ...}`.

### `models catalog`, `models download`, `models verify`

```bash
pumice-transcribe models catalog [--json]
pumice-transcribe models download <catalog-id> [--json] [--force]
pumice-transcribe models verify <catalog-id> [--json]
```

These manage the built-in pinned model catalog; see
[Downloading models](#downloading-models) for the full behavior (where files
go, verification, progress output, network rules).

In `models catalog`, `installed: yes` means a `.gguf` file with the expected
size is in the models folder, under any name — presence and size only, not
integrity (any right-sized file counts, even with different content). Use
`models list` or `models verify` to hash it. `models verify` checks the
catalog file name, or a copy under another name when that file is absent.

### `serve`

```bash
pumice-transcribe serve --config ./transcribe.yml --model whisper-large-v3-turbo-q8_0 --port 17568
```

Runs an OpenAI-compatible HTTP server on `127.0.0.1:PORT`. The bind address is
always `127.0.0.1` and is not configurable. The port can also be set as
`server.port` in `transcribe.yml`; `--port` overrides it. Port `0` is allowed
and the actual bound address is printed on startup.

`serve` loads one model at startup and keeps it in memory. The model is
selected with `--model ID` from the models folder, read once at startup; if
omitted, the configured `default_model` is used.
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
      { "id": "whisper-large-v3-turbo-q8_0", "object": "model", "created": 0, "owned_by": "pumice" }
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
  -F 'file=@clip.wav' -F 'model=whisper-large-v3-turbo-q8_0' -F 'language=pt'

curl --fail-with-body --max-time 130 \
  http://127.0.0.1:17568/v1/audio/transcriptions \
  -F 'file=@clip.wav' -F 'model=whisper-large-v3-turbo-q8_0' -F 'response_format=text'

curl --fail http://127.0.0.1:17568/v1/models
curl --fail http://127.0.0.1:17568/health
```

#### Managed mode (used by `pumice serve`)

Internal protocol, version 2 (version 2 added the compute snapshot and the
`engine_failed` stop). It exists so `pumice serve` can own one warm
`pumice-transcribe` child; it is not meant for direct use and may change with
the protocol version.

```bash
pumice-transcribe --config ./transcribe.yml serve --model whisper-large-v3-turbo-q8_0 --port 0 \
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
default_model"**: you did not pass `--model` or `--model-file`, and the
config has `default_model: null`.
- **"... is missing; every setting must be written"**: the config leaves a
setting out. Every setting is required; copy it from
`transcribe.example.yml`.
- **"the transcription configuration file does not exist"**: there is no
`--config` file and no per-user `transcribe.yml`; copy
`transcribe.example.yml`, edit it and pass its path with `--config`. This
also applies to `file --model-file PATH`.
- **"model `ID` is not in the models folder ... (models there: ...)"**
(`model_not_found`): no file in the [models folder](#the-models-folder) has
that id. For a catalog id the error names the download command, e.g.
`pumice-transcribe --config FILE models download whisper-tiny-q8_0`
(`--config` is kept when you passed one, because a `models_dir` override
only applies through that config file). With a `default_model:` prefix it
is a configuration error at the `default_model` line.
- **"models is no longer a setting"**: the config still has the `models:`
list of earlier versions; see the migration note in
[Configuration](#configuration).
- **"two files in the models folder have the model id ..."**: remove or
rename one of the two files named.
- **"schema_version must be 1"**: the config file has another
`schema_version`.
- **"unknown setting `foo`"**: the config contains a key that is not part of
the schema. Check the file line in the error message.
- **"model file not found"**: the `--model-file` path is not a regular
file, or a selected model's file was removed after the folder was read.
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
- **"unknown catalog model ..."**: the id passed to `models download` or
  `models verify` is not in the built-in catalog; run
  `pumice-transcribe models catalog`.
- **"... does not match the catalog entry (corrupt or modified); re-run with
  --force"**: a file is already installed under the catalog's file name but
  its size or SHA-256 differs. The file is left untouched; pass `--force` to
  replace it.
