#!/usr/bin/env bash
# Build pumice-transcribe with runtime CPU-variant selection and stage a
# self-contained folder: the binary, its core shared libraries, and one
# directory per ggml CPU backend module the build produced. Linux only.
#
#   crates/transcription/scripts/build-cpu-variants.sh [--debug]
#
# The result lands in target/cpu-variants/stage/ and can be moved anywhere;
# the binary resolves its core libraries from its own directory ($ORIGIN
# rpath) and loads the ggml CPU module only from
# stage/cpu/<variant>/libggml-cpu-<variant>.so — the directory the startup
# chooser points ggml at. Nothing is copied or written at runtime, so the
# staged folder works read-only (root-owned /opt installs, read-only mounts).
#
# The default portable build (feature `engine`) is untouched: this script uses
# its own CARGO_TARGET_DIR so CMake never reuses the static build's cache, and
# it exports an explicit TRANSCRIBE_CMAKE_ARGS, overriding the conservative
# portable baseline pinned in .cargo/config.toml (force=false), so nothing in
# that baseline can suppress the CPU variants. The `cpu-variants` feature
# itself sets GGML_BACKEND_DL=ON, GGML_CPU_ALL_VARIANTS=ON and GGML_NATIVE=OFF.
set -euo pipefail
shopt -s nullglob

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../../.." && pwd)"
# shellcheck source=stage-common.sh
. "$script_dir/stage-common.sh"
profile=release
if [ "${1:-}" = "--debug" ]; then
    profile=debug
fi

target_dir="$repo_root/target/cpu-variants"
stage_dir="$target_dir/stage"
mkdir -p "$target_dir"

export CARGO_TARGET_DIR="$target_dir"
export TRANSCRIBE_CMAKE_ARGS="-DGGML_NATIVE=OFF -DTRANSCRIBE_USE_SYSTEM_BLAS=OFF"

echo "building pumice-transcribe (cpu-variants, $profile) in $target_dir"
json_log="$target_dir/cargo-build.json"
if [ "$profile" = release ]; then
    cargo_build_logged "$json_log" 1200 --release -p pumice-transcription \
        --features cpu-variants --locked
else
    cargo_build_logged "$json_log" 1200 -p pumice-transcription --features cpu-variants --locked
fi

bin="$target_dir/$profile/pumice-transcribe"
sys_out="$(transcribe_sys_out "$json_log")"
echo "staging native libraries from $sys_out"

# Validate the staged set before copying: the executable plus an incomplete
# or mismatched library set fails at startup, not here.
for lib in libtranscribe.so libggml.so libggml-base.so libggml-cpu-x64.so; do
    if ! ls "$sys_out"/lib/"$lib"* >/dev/null 2>&1; then
        echo "error: expected library $lib* missing from $sys_out/lib" >&2
        exit 1
    fi
done

rm -rf "$stage_dir"
mkdir -p "$stage_dir"
cp "$bin" "$stage_dir/pumice-transcribe"
# Core libraries beside the binary, resolved through the $ORIGIN rpath.
# -L dereferences the versioned symlinks so the folder is self-contained.
for lib in "$sys_out"/lib/libtranscribe.so* "$sys_out"/lib/libggml.so* \
    "$sys_out"/lib/libggml-base.so*; do
    cp -L "$lib" "$stage_dir/"
done
stage_cpu_modules "$sys_out" "$stage_dir"

echo "staged $stage_dir:"
find "$stage_dir" -mindepth 1 -maxdepth 2 | sort

if ldd "$stage_dir/pumice-transcribe" | grep -q "not found"; then
    echo "error: staged binary does not resolve all its libraries:" >&2
    ldd "$stage_dir/pumice-transcribe" >&2
    exit 1
fi
echo "ldd ok: all libraries resolve"
ldd "$stage_dir/pumice-transcribe"
