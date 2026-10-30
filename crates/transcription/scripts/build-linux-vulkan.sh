#!/usr/bin/env bash
# Build pumice-transcribe with runtime CPU-variant selection plus the optional
# Vulkan module and stage a self-contained bundle. x86-64 Linux with glibc
# (x86_64-unknown-linux-gnu) only, never musl; experimental (P14), a CI
# artifact, not a release.
#
#   crates/transcription/scripts/build-linux-vulkan.sh [--target-dir DIR]
#   crates/transcription/scripts/build-linux-vulkan.sh --self-test
#
# Linux counterpart of build-windows-vulkan.ps1 and sibling of
# build-cpu-variants.sh (shared steps in stage-common.sh): its own
# CARGO_TARGET_DIR (CMake never reuses another build's cache), an explicit
# TRANSCRIBE_CMAKE_ARGS (wins over the portable baseline in
# .cargo/config.toml, force=false), and staging ONLY from the
# transcribe-cpp-sys out_dir of the build just run. The result lands in
# <target dir>/stage/:
#
#   pumice-transcribe                         $ORIGIN rpath
#   libtranscribe.so.0.3, libggml.so.0,       core libraries under the names
#     libggml-base.so.0                         the loader asks for (SONAMEs)
#   cpu/<variant>/libggml-cpu-<variant>.so    one CPU module per directory
#   gpu/vulkan/libggml-vulkan.so              the only Vulkan importer
#   bundle.json                               manifest: sizes and SHA-256
#
# The modules resolve the core libraries they need (libggml-base.so.0) by
# SONAME from the copy the executable already loaded, so they need no rpath
# of their own into the bundle root. Startup points ggml at exactly one
# cpu/<variant>/ directory and then gpu/vulkan/. Nothing is written at
# runtime. Unlike build-cpu-variants.sh, the unversioned and fully versioned
# core names (link-time names only) are not staged.
#
# bundle.json (src/bundle.rs) has the same format as on Windows. A `vulkan`
# build requires it: startup checks the core libraries, the selected CPU
# module and the GPU module against it before registering any of them.
#
# The audit at the end checks every staged ELF file: its NEEDED entries may
# only be the reviewed glibc/libstdc++/libgcc libraries, the bundled core
# libraries and, for the GPU module alone, libvulkan.so.1; RPATH must be
# absent and RUNPATH absent or exactly $ORIGIN (no build-host path); and no
# symbol version may be newer than the floor below. Its self-test runs first
# (--self-test runs just that, without a build). A relocated copy (a path
# with spaces and non-ASCII characters) is then checked with ldd, without
# LD_LIBRARY_PATH.
#
# Build-time needs: a C/C++ toolchain, CMake, jq, the Vulkan headers and
# loader library, glslc and the SPIRV-Headers CMake package (CI: the LunarG
# Vulkan SDK, see .github/workflows/transcription.yml). End users need only a
# Vulkan loader (libvulkan.so.1) and a driver, and only for the GPU.
set -euo pipefail
shopt -s nullglob

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../../.." && pwd)"
# shellcheck source=stage-common.sh
. "$script_dir/stage-common.sh"

triple=x86_64-unknown-linux-gnu
gpu_module=gpu/vulkan/libggml-vulkan.so
# The runtime floor: Ubuntu 22.04 (glibc 2.35, libstdc++ of GCC 12), the
# oldest distribution CI builds and checks the bundle on. A newer symbol
# version fails the audit.
glibc_floor=2.35
glibcxx_floor=3.4.30
cxxabi_floor=1.3.13
# The OS libraries a staged file may need, reviewed from readelf on
# ubuntu-22.04 (P14.5): glibc's own libraries, the C++ runtime and libgcc.
# Every glibc distribution provides them. A new one fails the audit until
# it is reviewed and added here.
system_libs=(
    libc.so.6 libm.so.6 libdl.so.2 libpthread.so.0 librt.so.1 ld-linux-x86-64.so.2
    libstdc++.so.6 libgcc_s.so.1
)
# The core libraries' SONAMEs; the expected set is checked after the build.
core_libs=(libtranscribe.so.0.3 libggml.so.0 libggml-base.so.0)
# The GNU variant set (engine.rs GNU_TABLE). A different set means the
# engine changed and the Rust chooser must be re-audited.
expected_variants="alderlake cannonlake cascadelake cooperlake haswell icelake ivybridge \
piledriver sandybridge sapphirerapids skylakex sse42 x64 zen4"

contains() {
    local needle="$1" item
    shift
    for item in "$@"; do
        [ "$item" = "$needle" ] && return 0
    done
    return 1
}

# needed_failures FILE NEEDED...
# The failures for one staged file's NEEDED entries (FILE relative to the
# bundle root), one per line; nothing when all are allowed.
needed_failures() {
    local file="$1" lib
    shift
    if [ "$#" -eq 0 ]; then
        # Everything staged links libc at least: an empty list means
        # readelf's output was not parsed.
        echo "$file has no parsed NEEDED entries"
        return
    fi
    for lib in "$@"; do
        if contains "$lib" "${system_libs[@]}" || contains "$lib" "${core_libs[@]}"; then
            continue
        fi
        if [ "$lib" = libvulkan.so.1 ]; then
            [ "$file" = "$gpu_module" ] && continue
            echo "$file needs libvulkan.so.1 (only $gpu_module may)"
        else
            echo "$file needs $lib, which is not on the reviewed allowlist"
        fi
    done
}

# runpath_failures FILE RPATH RUNPATH
runpath_failures() {
    local file="$1" rpath="$2" runpath="$3"
    if [ -n "$rpath" ]; then
        echo "$file has an RPATH ($rpath); only a RUNPATH of \$ORIGIN is allowed"
    fi
    if [ -n "$runpath" ] && [ "$runpath" != '$ORIGIN' ]; then
        echo "$file has RUNPATH $runpath; only \$ORIGIN is allowed"
    fi
}

# version_failures FILE VERSION_NEED...
# Symbol versions (GLIBC_2.34, GLIBCXX_3.4.29, CXXABI_1.3.13, ...) newer
# than the floor.
version_failures() {
    local file="$1" need prefix floor number
    shift
    for need in "$@"; do
        case "$need" in
            GLIBC_[0-9]*) prefix=GLIBC_ floor="$glibc_floor" ;;
            GLIBCXX_[0-9]*) prefix=GLIBCXX_ floor="$glibcxx_floor" ;;
            CXXABI_[0-9]*) prefix=CXXABI_ floor="$cxxabi_floor" ;;
            *) continue ;;
        esac
        number="${need#"$prefix"}"
        if [ "$(printf '%s\n%s\n' "$floor" "$number" | sort -V | tail -n 1)" != "$floor" ]; then
            echo "$file needs $need, newer than the floor $prefix$floor"
        fi
    done
}

# Synthetic inputs with known verdicts, checked before anything is built or
# audited.
self_test() {
    local errors=0 got expect
    check() {
        local label="$1" expect="$2" got="$3"
        if [ "$got" != "$expect" ]; then
            echo "error: audit self-test: $label: got [$got], expected [$expect]" >&2
            errors=$((errors + 1))
        fi
    }
    check exe "" "$(needed_failures pumice-transcribe \
        libtranscribe.so.0.3 libgcc_s.so.1 libm.so.6 libc.so.6)"
    check gpu "" "$(needed_failures "$gpu_module" \
        libggml-base.so.0 libvulkan.so.1 libstdc++.so.6 libm.so.6 libgcc_s.so.1 libc.so.6)"
    check cpu "" "$(needed_failures cpu/x64/libggml-cpu-x64.so \
        libggml-base.so.0 libstdc++.so.6 libc.so.6)"
    check cpu-vulkan "cpu/x64/libggml-cpu-x64.so needs libvulkan.so.1 (only $gpu_module may)" \
        "$(needed_failures cpu/x64/libggml-cpu-x64.so libvulkan.so.1 libc.so.6)"
    check core-vulkan "libggml.so.0 needs libvulkan.so.1 (only $gpu_module may)" \
        "$(needed_failures libggml.so.0 libggml-base.so.0 libvulkan.so.1)"
    expect="libggml-base.so.0 needs libgomp.so.1, which is not on the reviewed allowlist
libggml-base.so.0 needs libvulkan.so, which is not on the reviewed allowlist"
    check unknown "$expect" \
        "$(needed_failures libggml-base.so.0 libgomp.so.1 libc.so.6 libvulkan.so)"
    check moved-gpu "gpu/other/libggml-vulkan.so needs libvulkan.so.1 (only $gpu_module may)" \
        "$(needed_failures gpu/other/libggml-vulkan.so libvulkan.so.1)"
    check empty "libtranscribe.so.0.3 has no parsed NEEDED entries" \
        "$(needed_failures libtranscribe.so.0.3)"
    check runpath-origin "" "$(runpath_failures libggml.so.0 "" '$ORIGIN')"
    check runpath-none "" "$(runpath_failures pumice-transcribe "" "")"
    check runpath-host "libggml.so.0 has RUNPATH \$ORIGIN:/home/runner/out/lib; only \$ORIGIN is allowed" \
        "$(runpath_failures libggml.so.0 "" '$ORIGIN:/home/runner/out/lib')"
    check rpath "libggml.so.0 has an RPATH (/opt/x); only a RUNPATH of \$ORIGIN is allowed" \
        "$(runpath_failures libggml.so.0 /opt/x "")"
    check versions-ok "" "$(version_failures libggml.so.0 \
        GLIBC_2.2.5 GLIBC_2.35 GLIBCXX_3.4.30 GLIBCXX_3.4.9 CXXABI_1.3.13 GCC_3.0)"
    expect="libggml.so.0 needs GLIBC_2.36, newer than the floor GLIBC_2.35
libggml.so.0 needs GLIBCXX_3.4.31, newer than the floor GLIBCXX_3.4.30
libggml.so.0 needs CXXABI_1.3.14, newer than the floor CXXABI_1.3.13"
    check versions-new "$expect" "$(version_failures libggml.so.0 \
        GLIBC_2.36 GLIBCXX_3.4.31 CXXABI_1.3.14 GLIBC_2.3.4)"
    if [ "$errors" -ne 0 ]; then
        echo "error: dependency audit self-test failed" >&2
        exit 1
    fi
    echo "dependency audit self-test ok (14 cases)"
}

target_dir="$repo_root/target/linux-vulkan"
case "${1:-}" in
    --self-test)
        self_test
        exit 0
        ;;
    --target-dir)
        target_dir="${2:?--target-dir needs a directory}"
        ;;
    "") ;;
    *)
        echo "usage: $0 [--target-dir DIR] | --self-test" >&2
        exit 2
        ;;
esac
self_test

host="$(rustc -vV | sed -n 's/^host: //p')"
if [ "$host" != "$triple" ]; then
    echo "error: this bundle is built on and for $triple only (host is $host)" >&2
    exit 1
fi

mkdir -p "$target_dir"
target_dir="$(cd "$target_dir" && pwd)"
stage_dir="$target_dir/stage"

export CARGO_TARGET_DIR="$target_dir"
# Keep compiler launchers while enforcing the portable bundle baseline.
export TRANSCRIBE_CMAKE_ARGS="${TRANSCRIBE_CMAKE_ARGS:-} -DGGML_NATIVE=OFF -DTRANSCRIBE_USE_SYSTEM_BLAS=OFF"
# Ambient settings that would change or bypass the native build.
unset CMAKE_ARGS TRANSCRIBE_DIR GGML_BACKEND_PATH

echo "building pumice-transcribe (vulkan, release, $triple) in $target_dir"
json_log="$target_dir/cargo-build.json"
cargo_build_logged "$json_log" 3600 --release -p pumice-transcription --features vulkan --locked
sys_out="$(transcribe_sys_out "$json_log")"
exe="$(jq -rs '
    [.[] | select(.reason == "compiler-artifact" and .target.name == "pumice-transcribe")
          | .executable // empty]
    | last // empty
' "$json_log")"
if [ -z "$exe" ]; then
    echo "error: no pumice-transcribe executable in the current build" >&2
    exit 1
fi
echo "staging native libraries from $sys_out"

cmake_cache="$sys_out/build/CMakeCache.txt"
if [ -f "$cmake_cache" ]; then
    echo "native build settings (CMakeCache.txt):"
    grep -E '^(GGML_VULKAN|GGML_BACKEND_DL|GGML_CPU_ALL_VARIANTS|GGML_NATIVE|Vulkan_LIBRARY|Vulkan_INCLUDE_DIR|Vulkan_GLSLC_EXECUTABLE|SPIRV-Headers_DIR|CMAKE_CXX_COMPILER|CMAKE_GENERATOR):' \
        "$cmake_cache" | sed 's/^/  /' || true
fi

# Classify every shared object the native install produced; an unknown one
# means the native build changed and is not staged silently.
lib_dir="$sys_out/lib"
echo "native runtime files in $lib_dir:"
variants=()
for file in "$lib_dir"/*.so*; do
    name="$(basename "$file")"
    printf '  %s%s\n' "$name" "$([ -L "$file" ] && echo " -> $(readlink "$file")")"
    case "$name" in
        libtranscribe.so* | libggml.so* | libggml-base.so* | libggml-vulkan.so) ;;
        libggml-cpu-*.so)
            variant="${name#libggml-cpu-}"
            variants+=("${variant%.so}")
            ;;
        *)
            echo "error: unexpected library $name in $lib_dir" >&2
            exit 1
            ;;
    esac
done
for name in "${core_libs[@]}" libggml-vulkan.so; do
    if [ ! -f "$lib_dir/$name" ]; then
        echo "error: expected library $name missing from $lib_dir" >&2
        exit 1
    fi
done
built_variants="$(printf '%s\n' "${variants[@]}" | LC_ALL=C sort | xargs)"
if [ "$built_variants" != "$expected_variants" ]; then
    echo "error: CPU variant set changed: built [$built_variants], expected [$expected_variants]" >&2
    exit 1
fi

rm -rf "$stage_dir"
mkdir -p "$stage_dir"
cp "$exe" "$stage_dir/pumice-transcribe"
# Core libraries beside the binary, under their SONAMEs (the names the
# executable and the modules ask for), resolved through the $ORIGIN rpath.
for name in "${core_libs[@]}"; do
    cp -L "$lib_dir/$name" "$stage_dir/$name"
    soname="$(readelf -d "$stage_dir/$name" | sed -n 's/.*(SONAME).*\[\(.*\)\]$/\1/p')"
    if [ "$soname" != "$name" ]; then
        echo "error: $name has SONAME [$soname]; core_libs must list SONAMEs" >&2
        exit 1
    fi
done
stage_cpu_modules "$sys_out" "$stage_dir"
mkdir -p "$stage_dir/gpu/vulkan"
cp -L "$lib_dir/libggml-vulkan.so" "$stage_dir/$gpu_module"

# Staged layout and sizes.
mapfile -t files < <(cd "$stage_dir" && find . -type f -printf '%P\n' | LC_ALL=C sort)
total=0
echo "staged $stage_dir:"
for rel in "${files[@]}"; do
    size="$(stat -c %s "$stage_dir/$rel")"
    total=$((total + size))
    printf '  %12d  %s\n' "$size" "$rel"
done
printf '  %12d  total (%d files)\n' "$total" "${#files[@]}"

# bundle.json: identity from cargo metadata (the versions this lockfile
# builds with the same features and target; without the feature the engine
# is not in the resolve), then every staged file.
metadata="$target_dir/cargo-metadata.json"
cargo metadata --format-version 1 --locked --filter-platform "$triple" \
    --features pumice-transcription/vulkan > "$metadata"
package_version() {
    jq -er --arg name "$1" '
        [.packages[] | select(.name == $name)]
        | if length == 1 then .[0].version
          else error("cargo metadata lists \(length) \($name) packages, expected 1") end
    ' "$metadata"
}
product_version="$(package_version pumice-transcription)"
engine_version="$(package_version transcribe-cpp-sys)"
entries="$target_dir/bundle-entries.jsonl"
: > "$entries"
for rel in "${files[@]}"; do
    case "$rel" in
        pumice-transcribe) role=executable ;;
        cpu/*) role=cpu ;;
        gpu/*) role=gpu ;;
        */*)
            echo "error: unclassified staged file $rel" >&2
            exit 1
            ;;
        *)
            if ! contains "$rel" "${core_libs[@]}"; then
                echo "error: unclassified staged file $rel" >&2
                exit 1
            fi
            role=core
            ;;
    esac
    jq -nc --arg path "$rel" --arg role "$role" \
        --argjson size "$(stat -c %s "$stage_dir/$rel")" \
        --arg sha256 "$(sha256sum "$stage_dir/$rel" | cut -d ' ' -f 1)" \
        '{path: $path, role: $role, size: $size, sha256: $sha256}' >> "$entries"
done
jq -s --arg version "$product_version" --arg target "$triple" \
    --arg engine "transcribe-cpp-sys $engine_version" '{
        schema_version: 1,
        product: "pumice-transcribe",
        version: $version,
        target: $target,
        features: ["vulkan"],
        engine: $engine,
        files: .
    }' "$entries" > "$stage_dir/bundle.json"
rm -f "$entries"
echo "wrote bundle.json: pumice-transcribe $product_version ($triple), transcribe-cpp-sys" \
    "$engine_version, ${#files[@]} files"

# Dependency audit with readelf against the reviewed allowlist and floor.
failures=()
echo "dependency audit (readelf -d, readelf -V):"
for rel in "${files[@]}"; do
    path="$stage_dir/$rel"
    dynamic="$(readelf -d -W "$path")"
    mapfile -t needed < <(sed -n 's/.*(NEEDED).*\[\(.*\)\]$/\1/p' <<< "$dynamic")
    rpath="$(sed -n 's/.*(RPATH).*\[\(.*\)\]$/\1/p' <<< "$dynamic")"
    runpath="$(sed -n 's/.*(RUNPATH).*\[\(.*\)\]$/\1/p' <<< "$dynamic")"
    mapfile -t versions < <(readelf -V -W "$path" |
        sed -n 's/.*Name: \(\(GLIBC\|GLIBCXX\|CXXABI\)_[0-9][0-9.]*\).*/\1/p' | sort -uV)
    newest="$(printf '%s\n' "${versions[@]}" | grep -E '^GLIBC_' | sort -V | tail -n 1 || true)"
    newest_cxx="$(printf '%s\n' "${versions[@]}" | grep -E '^GLIBCXX_' | sort -V | tail -n 1 || true)"
    echo "  $rel: ${needed[*]}; runpath [${runpath}]; newest ${newest:-none} ${newest_cxx}"
    mapfile -t -O "${#failures[@]}" failures < <(
        needed_failures "$rel" "${needed[@]}"
        runpath_failures "$rel" "$rpath" "$runpath"
        version_failures "$rel" "${versions[@]}"
    )
done
if [ "${#failures[@]}" -gt 0 ]; then
    printf 'error: %s\n' "${failures[@]}" >&2
    echo "error: dependency audit failed" >&2
    exit 1
fi
echo "dependency audit ok: only reviewed OS libraries, no build-host paths, glibc <= $glibc_floor," \
    "only $gpu_module needs libvulkan.so.1"

# The relocated bundle resolves its core libraries from its own directory,
# without LD_LIBRARY_PATH; the GPU module's only other dependency is the
# system Vulkan loader.
moved="$target_dir/relocation check ç"
rm -rf "$moved"
cp -a "$stage_dir" "$moved"
resolved="$(env -u LD_LIBRARY_PATH ldd "$moved/pumice-transcribe")"
echo "ldd pumice-transcribe (relocated):"
sed 's/^/  /' <<< "$resolved"
if grep -q "not found" <<< "$resolved"; then
    echo "error: the relocated binary does not resolve all its libraries" >&2
    exit 1
fi
for name in "${core_libs[@]}"; do
    if ! grep -qF "$name => $moved/$name " <<< "$resolved"; then
        echo "error: $name does not resolve from the relocated bundle" >&2
        exit 1
    fi
done
echo "ldd $gpu_module (relocated; libggml-base.so.0 comes from the process at runtime):"
env -u LD_LIBRARY_PATH ldd "$moved/$gpu_module" | sed 's/^/  /' || true
rm -rf "$moved"
echo "ldd ok: the relocated bundle resolves its core libraries from its own directory"
