#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
: "${MUSL_TOOLCHAIN:?Set MUSL_TOOLCHAIN to the extracted x86_64-unknown-linux-musl directory}"
export PATH="$MUSL_TOOLCHAIN/bin:$PATH"
export CC_x86_64_unknown_linux_musl=x86_64-unknown-linux-musl-gcc
export CXX_x86_64_unknown_linux_musl=x86_64-unknown-linux-musl-g++
export AR_x86_64_unknown_linux_musl=x86_64-unknown-linux-musl-ar
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER="$PWD/ci/musl-linker.py"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS='-C target-feature=+crt-static -C link-self-contained=yes'
export TRANSCRIBE_CMAKE_ARGS='-DGGML_NATIVE=OFF -DTRANSCRIBE_USE_SYSTEM_BLAS=OFF -DGGML_SSE42=ON -DGGML_AVX=ON -DGGML_AVX2=ON -DGGML_F16C=ON -DGGML_FMA=ON -DGGML_BMI2=ON'
cargo build --release --locked --target x86_64-unknown-linux-musl
