# Real-GPU check on server-1 — 2026-10-16

First run of Pumice transcription on a real GPU. Done by the orchestrator on server-1 with the CI artifact `pumice-transcribe-linux-x64-vulkan-preview` from run 38946199873 (tarball SHA-256 verified against its `SHA256SUMS`), extracted to `~/scratch/p14.5-gpu/`. Nothing was installed; the Vulkan loader (`libvulkan.so.1` 1.4.341) and Mesa's Intel ANV driver were already on the host.

## Hardware and detection

- CPU: Intel Core i5-14500T (20 threads); selected CPU module `alderlake`.
- GPU: Intel UHD Graphics 770 (ADL-S GT1), integrated, driver Mesa ANV via `/dev/dri/renderD128`.
- `backends --backend auto --json`: `bundle: verified` (5 files, 47.8 MB in 28 ms), `gpu_module: scanned`, `gpu_devices: 1`, devices `CPU` and `Vulkan0` (kind `vulkan`, type `igpu`), `fallback_reason: null`. So `auto` selects the integrated GPU.

## Results (14.2 s Portuguese sample, `crates/transcription/tests/fixtures/sample-pt.wav`)

One-shot `file --json` runs (cold process each time):

| Model | Backend | Load | Transcription | Max RSS |
|---|---|---:|---:|---:|
| tiny | vulkan (UHD 770) | 134 ms | 1,772 ms | 158 MB |
| tiny | cpu (alderlake) | 57 ms | 402 ms | 101 MB |
| turbo | vulkan (UHD 770) | 456 ms | 9,920 ms | 196 MB |
| turbo | cpu (alderlake) | 366 ms | 6,489 ms | 979 MB |

Warm `serve` (model loaded once, three requests each, wall time per request through curl):

| Backend | Request 1 | Request 2 | Request 3 |
|---|---:|---:|---:|
| vulkan (UHD 770) | 9.86 s | 9.91 s | 9.89 s |
| cpu (alderlake) | 5.93 s | 5.91 s | 5.86 s |

Transcripts: turbo's GPU and CPU text are identical. tiny differs in one punctuation mark ("chovendo, está" vs "chovendo. Está"). The compute snapshot reported the actual device in every run (`backend: vulkan`, `Intel(R) UHD Graphics 770 (ADL-S GT1)`), and the `cpu` server logged `gpu/vulkan not scanned`.

## Conclusions

1. The Linux Vulkan bundle works on real hardware from a relocated folder with no setup: detection, `bundle.json` verification, loading, inference and reporting are correct.
2. On this machine the integrated GPU is about 1.7× slower than the CPU for turbo (and 4× for tiny), while using far less host memory (the weights live in GPU-visible memory). A warm server does not close the gap.
3. With the current policy `auto` picks the integrated GPU here, which would make transcription slower. This led to `p14.5-linux-vulkan/D1`.
4. Not measured: a discrete GPU (the owner's Windows check), longer audio, power use.
