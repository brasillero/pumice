#!/usr/bin/env bash
# Build pumice-transcribe with runtime CPU-variant selection and stage a
# self-contained folder: the binary plus every shared library and ggml CPU
# backend module the build produced. Linux only.
#
#   crates/transcription/scripts/build-cpu-variants.sh [--debug]
#
# The result lands in target/cpu-variants/stage/ and can be moved anywhere;
# the binary resolves its libraries from its own directory ($ORIGIN rpath) and
# loads the ggml CPU modules only from that same trusted directory.
#
# The default portable build (feature `engine`) is untouched: this script uses
# its own CARGO_TARGET_DIR so CMake never reuses the static build's cache, and
# it exports an explicit TRANSCRIBE_CMAKE_ARGS, overriding the conservative
# portable baseline pinned in .cargo/config.toml (force=false), so nothing in
# that baseline can suppress the CPU variants. The `cpu-variants` feature
# itself sets GGML_BACKEND_DL=ON, GGML_CPU_ALL_VARIANTS=ON and GGML_NATIVE=OFF.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
profile=release
if [ "${1:-}" = "--debug" ]; then
    profile=debug
fi

target_dir="$repo_root/target/cpu-variants"
stage_dir="$target_dir/stage"

export CARGO_TARGET_DIR="$target_dir"
export TRANSCRIBE_CMAKE_ARGS="-DGGML_NATIVE=OFF -DTRANSCRIBE_USE_SYSTEM_BLAS=OFF"

echo "building pumice-transcribe (cpu-variants, $profile) in $target_dir"
if [ "$profile" = release ]; then
    timeout 1200 cargo build --release -p pumice-transcription --features cpu-variants --locked
else
    timeout 1200 cargo build -p pumice-transcription --features cpu-variants --locked
fi

bin="$target_dir/$profile/pumice-transcribe"
sys_out=""
for out in "$target_dir/$profile/build"/transcribe-cpp-sys-*/out; do
    if ls "$out"/lib/libggml-cpu-*.so >/dev/null 2>&1; then
        sys_out="$out"
        break
    fi
done
if [ -z "$sys_out" ]; then
    echo "error: no transcribe-cpp-sys build output with CPU variant modules found" >&2
    exit 1
fi

rm -rf "$stage_dir"
mkdir -p "$stage_dir"
cp "$bin" "$stage_dir/pumice-transcribe"
# -L dereferences the versioned symlinks so the folder is self-contained.
cp -L "$sys_out"/lib/*.so* "$stage_dir/"

echo "staged $stage_dir:"
ls -1 "$stage_dir"

if ldd "$stage_dir/pumice-transcribe" | grep -q "not found"; then
    echo "error: staged binary does not resolve all its libraries:" >&2
    ldd "$stage_dir/pumice-transcribe" >&2
    exit 1
fi
echo "ldd ok: all libraries resolve"
ldd "$stage_dir/pumice-transcribe"
