# Shared by build-cpu-variants.sh and build-linux-vulkan.sh; sourced, not
# run. Both build pumice-transcribe with the engine as shared libraries and
# stage one ggml CPU module per cpu/<variant>/ directory; they differ in the
# feature, the target directory and what else they stage.

# cargo_build_logged LOG TIMEOUT CARGO_BUILD_ARGS...
# Runs `cargo build` with JSON messages written to LOG, so the staging step
# can name the exact native build it just ran.
cargo_build_logged() {
    local log="$1" limit="$2"
    shift 2
    timeout "$limit" cargo build "$@" --message-format=json > "$log"
}

# transcribe_sys_out LOG
# Prints the out_dir of transcribe-cpp-sys in the build logged in LOG.
# Stage ONLY that build-script output directory: several cached
# transcribe-cpp-sys-<hash>/out directories can coexist after toolchain or
# dependency changes, and picking the wrong one pairs the fresh executable
# with stale native libraries. The build-script-executed messages of the
# build just run name the exact out_dir Cargo used.
transcribe_sys_out() {
    local sys_out
    sys_out="$(jq -rs '
        [.[] | select(.reason == "build-script-executed")
              | select(.package_id | contains("transcribe-cpp-sys"))]
        | last | .out_dir // empty
    ' "$1")"
    if [ -z "$sys_out" ]; then
        echo "error: no transcribe-cpp-sys build-script output in the current build" >&2
        return 1
    fi
    printf '%s\n' "$sys_out"
}

# stage_cpu_modules SYS_OUT STAGE_DIR
# One CPU module per variant directory: ggml's directory probe scores
# whatever it finds, so each directory must hold exactly its own module and
# nothing a lower-priority scan could pick instead.
stage_cpu_modules() {
    local sys_out="$1" stage_dir="$2" lib name variant dir count
    for lib in "$sys_out"/lib/libggml-cpu-*.so*; do
        name="$(basename "$lib")"
        variant="${name#libggml-cpu-}"
        variant="${variant%%.so*}"
        dir="$stage_dir/cpu/$variant"
        mkdir -p "$dir"
        cp -L "$lib" "$dir/libggml-cpu-$variant.so"
    done

    if [ ! -f "$stage_dir/cpu/x64/libggml-cpu-x64.so" ]; then
        echo "error: x64 CPU module missing from the staged cpu/ directory" >&2
        return 1
    fi
    count="$(find "$stage_dir/cpu" -mindepth 1 -maxdepth 1 -type d | wc -l)"
    if [ "$count" -lt 2 ]; then
        echo "error: expected several cpu/<variant>/ directories, found $count" >&2
        return 1
    fi
}
