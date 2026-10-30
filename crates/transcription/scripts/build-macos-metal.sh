#!/usr/bin/env bash
# Build pumice-transcribe with the engine as shared libraries plus the
# optional Metal module and stage a self-contained bundle. Apple Silicon
# macOS (aarch64-apple-darwin) only; experimental (P14.4), a CI artifact,
# not a release.
#
#   crates/transcription/scripts/build-macos-metal.sh [--target-dir DIR]
#   crates/transcription/scripts/build-macos-metal.sh --self-test
#
# macOS counterpart of build-linux-vulkan.sh (shared steps in
# stage-common.sh): its own CARGO_TARGET_DIR (CMake never reuses another
# build's cache), an explicit TRANSCRIBE_CMAKE_ARGS (wins over the portable
# baseline in .cargo/config.toml, force=false), a deliberate deployment
# target, and staging ONLY from the transcribe-cpp-sys out_dir of the build
# just run. The result lands in <target dir>/stage/:
#
#   pumice-transcribe                     LC_RPATH @executable_path
#   libtranscribe.0.3.dylib,              core libraries under their install
#     libggml.0.dylib,                      names (@rpath/<name>), LC_RPATH
#     libggml-base.0.dylib                  @loader_path
#   cpu/arm64/libggml-cpu.so              the one portable arm64 CPU module
#   gpu/metal/libggml-metal.so            the only Metal importer
#   bundle.json                           manifest: sizes and SHA-256
#
# ggml builds no x86-style CPU variants on arm64: dynamic backends give one
# portable CPU module, `libggml-cpu.so`, which exports no score function, so
# it keeps that exact name (ggml loads it as the plain `cpu` backend). The
# modules are CMake MODULE bundles, `.so` on Apple as ggml's loader expects.
# The engine installs them with LC_RPATH @loader_path; staged one or two
# directories down, that is rewritten to @loader_path/../.., so they resolve
# @rpath/libggml-base.0.dylib from the bundle root and never from their own
# directory. Apple Silicon runs no unsigned code: install_name_tool re-signs
# a linker-signed file it edits (Xcode 16.4 does), and the stager re-signs
# ad hoc (`codesign -s -`) only when the signature no longer verifies. No
# Developer ID signature, no notarization.
#
# The Metal shaders are embedded as source (GGML_METAL_EMBED_LIBRARY, set by
# the transcribe-cpp `metal` feature) and compiled by the OS Metal framework
# on first use: no .metal or .metallib file ships, and users need no Xcode.
#
# bundle.json (src/bundle.rs) has the same format as on Windows and Linux. A
# `metal` build requires it: startup checks the core libraries, the CPU
# module and the GPU module against it before registering any of them.
#
# The audit at the end checks every staged Mach-O file (otool -l): arm64
# only; its dylib dependencies may only be /usr/lib, /System/Library
# frameworks (Metal and MetalKit for the GPU module alone) and the bundled
# core libraries through @rpath; LC_RPATH must be exactly @executable_path
# (executable), @loader_path (core libraries) or @loader_path/../.. (modules);
# a core library's install name must be @rpath/<its staged name>; every
# minimum OS version (LC_BUILD_VERSION minos) must equal the deployment
# target below; and every file must carry a valid signature. Its self-test
# runs first (--self-test runs just that, without a build, on any OS). A
# relocated copy (a path with spaces and non-ASCII characters) is then run
# without DYLD_* variables, and every image dyld loads must come from that
# copy or the OS.
#
# Build-time needs: Xcode command line tools (Apple clang, the macOS SDK,
# otool, install_name_tool, codesign), CMake and jq (CI: GitHub's macOS
# image). It runs on macOS's own bash 3.2: no mapfile, and every possibly
# empty array is expanded as ${a[@]+"${a[@]}"}. End users need macOS 11 or
# newer on Apple Silicon.
set -euo pipefail
shopt -s nullglob

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../../.." && pwd)"
# shellcheck source=stage-common.sh
. "$script_dir/stage-common.sh"

triple=aarch64-apple-darwin
gpu_module=gpu/metal/libggml-metal.so
cpu_module=cpu/arm64/libggml-cpu.so
# The oldest macOS the bundle supports: every Mach-O file's minos must be
# exactly this (Rust's own default for this target, and the first macOS on
# Apple Silicon).
deployment_target=11.0
# The core libraries' install names (@rpath/<name>), checked after the build.
core_libs=(libtranscribe.0.3.dylib libggml.0.dylib libggml-base.0.dylib)
# Frameworks only the GPU module may link.
gpu_frameworks=(Metal MetalKit)

contains() {
    local needle="$1" item
    shift
    for item in "$@"; do
        [ "$item" = "$needle" ] && return 0
    done
    return 1
}

# read_lines NAME: reads stdin into the array NAME, one element per line
# (bash 3.2 has no mapfile).
read_lines() {
    local read_lines_line
    eval "$1=()"
    while IFS= read -r read_lines_line; do
        eval "$1+=(\"\$read_lines_line\")"
    done
}

# macho_info FILE
# The load commands the audit reads, one per line: `dep NAME`, `id NAME`,
# `rpath PATH`, `minos VERSION`, `platform N`, `sdk VERSION`.
macho_info() {
    otool -l "$1" | awk '
        function value(field,    v) {
            v = $0
            sub("^ *" field " ", "", v)
            sub(/ \(offset [0-9]+\)$/, "", v)
            return v
        }
        $1 == "cmd" { cmd = $2 }
        $1 == "name" && cmd ~ /^LC_(LOAD|LOAD_WEAK|REEXPORT|LAZY_LOAD|LOAD_UPWARD)_DYLIB$/ {
            print "dep " value("name")
        }
        $1 == "name" && cmd == "LC_ID_DYLIB" { print "id " value("name") }
        $1 == "path" && cmd == "LC_RPATH" { print "rpath " value("path") }
        cmd == "LC_BUILD_VERSION" && ($1 == "minos" || $1 == "platform" || $1 == "sdk") {
            print $1 " " $2
        }
    '
}

# dep_failures FILE DEP...
# The failures for one staged file's dylib dependencies (FILE relative to
# the bundle root), one per line; nothing when all are allowed.
dep_failures() {
    local file="$1" dep framework
    shift
    if [ "$#" -eq 0 ]; then
        # Everything staged links libSystem at least: an empty list means
        # otool's output was not parsed.
        echo "$file has no parsed dylib dependencies"
        return
    fi
    for dep in "$@"; do
        case "$dep" in
            /System/Library/Frameworks/*.framework/*)
                framework="${dep#/System/Library/Frameworks/}"
                framework="${framework%%.framework/*}"
                if contains "$framework" "${gpu_frameworks[@]}" && [ "$file" != "$gpu_module" ]; then
                    echo "$file links $framework.framework (only $gpu_module may)"
                fi
                ;;
            /usr/lib/*) ;;
            @rpath/*)
                if ! contains "${dep#@rpath/}" "${core_libs[@]}"; then
                    echo "$file needs $dep, which is not a bundled core library"
                fi
                ;;
            *) echo "$file needs $dep, which is neither an OS library nor bundled" ;;
        esac
    done
}

# rpath_failures FILE RPATH...
rpath_failures() {
    local file="$1" expected
    shift
    case "$file" in
        pumice-transcribe) expected='@executable_path' ;;
        cpu/*/* | gpu/*/*) expected='@loader_path/../..' ;;
        */*)
            echo "$file is not a known staged location"
            return
            ;;
        *) expected='@loader_path' ;;
    esac
    if [ "$#" -ne 1 ] || [ "$1" != "$expected" ]; then
        echo "$file has LC_RPATH [$*]; expected exactly [$expected]"
    fi
}

# id_failures FILE [ID]
# A core library is identified as @rpath/<its staged name>; the executable
# and the modules (MH_BUNDLE) have no install name.
id_failures() {
    local file="$1" id="${2:-}"
    if contains "$file" "${core_libs[@]}"; then
        [ "$id" = "@rpath/$file" ] || echo "$file has install name [$id]; expected [@rpath/$file]"
    elif [ -n "$id" ]; then
        echo "$file has an install name ($id); only core libraries may"
    fi
}

# minos_failures FILE MINOS...
minos_failures() {
    local file="$1"
    shift
    if [ "$#" -ne 1 ] || [ "$1" != "$deployment_target" ]; then
        echo "$file has minimum macOS [$*]; expected exactly [$deployment_target]"
    fi
}

# Synthetic inputs with known verdicts, checked before anything is built or
# audited. Needs no macOS tools.
self_test() {
    local errors=0 expect
    check() {
        local label="$1" expect="$2" got="$3"
        if [ "$got" != "$expect" ]; then
            echo "error: audit self-test: $label: got [$got], expected [$expect]" >&2
            errors=$((errors + 1))
        fi
    }
    check exe "" "$(dep_failures pumice-transcribe @rpath/libtranscribe.0.3.dylib \
        /System/Library/Frameworks/CoreFoundation.framework/Versions/A/CoreFoundation \
        /usr/lib/libiconv.2.dylib /usr/lib/libSystem.B.dylib)"
    check gpu "" "$(dep_failures "$gpu_module" @rpath/libggml-base.0.dylib \
        /System/Library/Frameworks/Foundation.framework/Versions/C/Foundation \
        /System/Library/Frameworks/Metal.framework/Versions/A/Metal \
        /System/Library/Frameworks/MetalKit.framework/Versions/A/MetalKit \
        /usr/lib/libc++.1.dylib /usr/lib/libSystem.B.dylib)"
    check cpu "" "$(dep_failures "$cpu_module" @rpath/libggml-base.0.dylib \
        /System/Library/Frameworks/Accelerate.framework/Versions/A/Accelerate \
        /usr/lib/libc++.1.dylib /usr/lib/libSystem.B.dylib)"
    check cpu-metal "$cpu_module links Metal.framework (only $gpu_module may)" \
        "$(dep_failures "$cpu_module" /System/Library/Frameworks/Metal.framework/Versions/A/Metal)"
    check core-metalkit "libggml.0.dylib links MetalKit.framework (only $gpu_module may)" \
        "$(dep_failures libggml.0.dylib /usr/lib/libSystem.B.dylib \
            /System/Library/Frameworks/MetalKit.framework/Versions/A/MetalKit)"
    expect="libggml-base.0.dylib needs /opt/homebrew/opt/libomp/lib/libomp.dylib, which is neither an OS library nor bundled
libggml-base.0.dylib needs /Users/runner/work/out/lib/libggml.0.dylib, which is neither an OS library nor bundled
libggml-base.0.dylib needs @rpath/libomp.dylib, which is not a bundled core library
libggml-base.0.dylib needs @loader_path/libggml.0.dylib, which is neither an OS library nor bundled"
    check unknown "$expect" "$(dep_failures libggml-base.0.dylib \
        /opt/homebrew/opt/libomp/lib/libomp.dylib /Users/runner/work/out/lib/libggml.0.dylib \
        @rpath/libomp.dylib @loader_path/libggml.0.dylib /usr/lib/libSystem.B.dylib)"
    check moved-gpu "gpu/other/libggml-metal.so links Metal.framework (only $gpu_module may)" \
        "$(dep_failures gpu/other/libggml-metal.so /System/Library/Frameworks/Metal.framework/Metal)"
    check empty "libtranscribe.0.3.dylib has no parsed dylib dependencies" \
        "$(dep_failures libtranscribe.0.3.dylib)"
    check rpath-exe "" "$(rpath_failures pumice-transcribe @executable_path)"
    check rpath-core "" "$(rpath_failures libggml.0.dylib @loader_path)"
    check rpath-module "" "$(rpath_failures "$gpu_module" @loader_path/../..)"
    check rpath-module-own-dir "$cpu_module has LC_RPATH [@loader_path]; expected exactly [@loader_path/../..]" \
        "$(rpath_failures "$cpu_module" @loader_path)"
    check rpath-build-dir "pumice-transcribe has LC_RPATH [@executable_path /Users/runner/work/out/lib]; expected exactly [@executable_path]" \
        "$(rpath_failures pumice-transcribe @executable_path /Users/runner/work/out/lib)"
    check rpath-none "libggml.0.dylib has LC_RPATH []; expected exactly [@loader_path]" \
        "$(rpath_failures libggml.0.dylib)"
    check id-core "" "$(id_failures libggml-base.0.dylib @rpath/libggml-base.0.dylib)"
    check id-module "" "$(id_failures "$gpu_module")"
    check id-absolute "libggml.0.dylib has install name [/Users/runner/out/lib/libggml.0.dylib]; expected [@rpath/libggml.0.dylib]" \
        "$(id_failures libggml.0.dylib /Users/runner/out/lib/libggml.0.dylib)"
    check id-renamed "libggml.0.dylib has install name [@rpath/libggml.0.1.dylib]; expected [@rpath/libggml.0.dylib]" \
        "$(id_failures libggml.0.dylib @rpath/libggml.0.1.dylib)"
    check id-on-module "$cpu_module has an install name (@rpath/libggml-cpu.so); only core libraries may" \
        "$(id_failures "$cpu_module" @rpath/libggml-cpu.so)"
    check minos-ok "" "$(minos_failures libggml.0.dylib "$deployment_target")"
    check minos-new "libggml.0.dylib has minimum macOS [14.0]; expected exactly [$deployment_target]" \
        "$(minos_failures libggml.0.dylib 14.0)"
    check minos-none "libggml.0.dylib has minimum macOS []; expected exactly [$deployment_target]" \
        "$(minos_failures libggml.0.dylib)"
    if [ "$errors" -ne 0 ]; then
        echo "error: dependency audit self-test failed" >&2
        exit 1
    fi
    echo "dependency audit self-test ok (23 cases)"
}

file_size() {
    stat -f %z "$1"
}

file_sha256() {
    shasum -a 256 "$1" | cut -d ' ' -f 1
}

target_dir="$repo_root/target/macos-metal"
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
export TRANSCRIBE_CMAKE_ARGS="${TRANSCRIBE_CMAKE_ARGS:--DGGML_NATIVE=OFF -DTRANSCRIBE_USE_SYSTEM_BLAS=OFF}"
# rustc, cc and CMake (CMAKE_OSX_DEPLOYMENT_TARGET) all read it.
export MACOSX_DEPLOYMENT_TARGET="$deployment_target"
# Ambient settings that would change or bypass the native build, or the
# relocation check.
unset CMAKE_ARGS TRANSCRIBE_DIR GGML_BACKEND_PATH
unset DYLD_LIBRARY_PATH DYLD_FALLBACK_LIBRARY_PATH DYLD_FRAMEWORK_PATH \
    DYLD_FALLBACK_FRAMEWORK_PATH DYLD_INSERT_LIBRARIES

echo "building pumice-transcribe (metal, release, $triple, macOS $deployment_target) in $target_dir"
json_log="$target_dir/cargo-build.json"
cargo_build_logged "$json_log" 3600 --release -p pumice-transcription --features metal --locked
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
    grep -E '^(GGML_METAL|GGML_METAL_EMBED_LIBRARY|GGML_BACKEND_DL|GGML_CPU_ALL_VARIANTS|GGML_NATIVE|GGML_ACCELERATE|GGML_BLAS|CMAKE_OSX_DEPLOYMENT_TARGET|CMAKE_OSX_ARCHITECTURES|CMAKE_OSX_SYSROOT|CMAKE_CXX_COMPILER|CMAKE_GENERATOR):' \
        "$cmake_cache" | sed 's/^/  /' || true
fi
# The native build's own diagnostics, captured by Cargo: code newer than the
# deployment target that is not behind an availability check would fail at
# run time on older macOS.
build_output="$(dirname "$sys_out")/output"
if [ -f "$build_output" ]; then
    availability="$(grep -c 'is only available on macOS' "$build_output" || true)"
    echo "native build: $availability unguarded-availability warning(s) for macOS $deployment_target"
    grep 'is only available on macOS' "$build_output" | sort -u | head -n 20 | sed 's/^/  /' || true
fi

# Classify every library the native install produced; an unknown one means
# the native build changed and is not staged silently.
lib_dir="$sys_out/lib"
echo "native runtime files in $lib_dir:"
for file in "$lib_dir"/*.dylib "$lib_dir"/*.so; do
    name="$(basename "$file")"
    printf '  %s%s\n' "$name" "$([ -L "$file" ] && echo " -> $(readlink "$file")")"
    case "$name" in
        libtranscribe*.dylib | libggml.*dylib | libggml-base*.dylib | libggml-cpu.so | libggml-metal.so) ;;
        *)
            echo "error: unexpected library $name in $lib_dir" >&2
            exit 1
            ;;
    esac
done
for name in "${core_libs[@]}" libggml-cpu.so libggml-metal.so; do
    if [ ! -f "$lib_dir/$name" ]; then
        echo "error: expected library $name missing from $lib_dir" >&2
        exit 1
    fi
done
# Embedded shaders: no metallib or kernel sources installed, and the module
# carries the embedded source's symbols.
leftovers="$(find "$sys_out/lib" "$sys_out/bin" \( -name '*.metallib' -o -name '*.metal' \) 2> /dev/null || true)"
if [ -n "$leftovers" ]; then
    echo "error: Metal library files installed (the shaders must be embedded): $leftovers" >&2
    exit 1
fi
embedded="$(nm -U "$lib_dir/libggml-metal.so" | grep -c '_ggml_metallib_.*_start$' || true)"
if [ "$embedded" -eq 0 ]; then
    echo "error: libggml-metal.so carries no embedded Metal source" >&2
    exit 1
fi
echo "libggml-metal.so embeds $embedded Metal source unit(s)"

rm -rf "$stage_dir"
mkdir -p "$stage_dir"
cp "$exe" "$stage_dir/pumice-transcribe"
# Core libraries beside the binary, under their install names (the names
# the executable and the modules ask for through @rpath).
for name in "${core_libs[@]}"; do
    cp -L "$lib_dir/$name" "$stage_dir/$name"
done
for module in "$cpu_module" "$gpu_module"; do
    mkdir -p "$stage_dir/$(dirname "$module")"
    cp -L "$lib_dir/$(basename "$module")" "$stage_dir/$module"
    chmod u+w "$stage_dir/$module"
    echo "$module: $(codesign -dv "$stage_dir/$module" 2>&1 | grep -E '^(Signature|CodeDirectory)' | xargs)"
    install_name_tool -rpath @loader_path @loader_path/../.. "$stage_dir/$module"
    if codesign --verify --strict "$stage_dir/$module" 2> /dev/null; then
        echo "  signature still valid after install_name_tool"
    else
        echo "  install_name_tool invalidated the signature; re-signing ad hoc"
        codesign --force --sign - "$stage_dir/$module"
    fi
done

# Staged layout and sizes.
read_lines files < <(cd "$stage_dir" && find . -type f | sed 's|^\./||' | LC_ALL=C sort)
total=0
echo "staged $stage_dir:"
for rel in "${files[@]}"; do
    size="$(file_size "$stage_dir/$rel")"
    total=$((total + size))
    printf '  %12d  %s\n' "$size" "$rel"
done
printf '  %12d  total (%d files)\n' "$total" "${#files[@]}"

# bundle.json: identity from cargo metadata (the versions this lockfile
# builds with the same features and target), then every staged file.
metadata="$target_dir/cargo-metadata.json"
cargo metadata --format-version 1 --locked --filter-platform "$triple" \
    --features pumice-transcription/metal > "$metadata"
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
        --argjson size "$(file_size "$stage_dir/$rel")" \
        --arg sha256 "$(file_sha256 "$stage_dir/$rel")" \
        '{path: $path, role: $role, size: $size, sha256: $sha256}' >> "$entries"
done
jq -s --arg version "$product_version" --arg target "$triple" \
    --arg engine "transcribe-cpp-sys $engine_version" '{
        schema_version: 1,
        product: "pumice-transcribe",
        version: $version,
        target: $target,
        features: ["metal"],
        engine: $engine,
        files: .
    }' "$entries" > "$stage_dir/bundle.json"
rm -f "$entries"
echo "wrote bundle.json: pumice-transcribe $product_version ($triple), transcribe-cpp-sys" \
    "$engine_version, ${#files[@]} files"

# Mach-O audit with otool against the rules above.
failures=""
echo "Mach-O audit (otool -l, lipo, codesign):"
for rel in "${files[@]}"; do
    path="$stage_dir/$rel"
    info="$(macho_info "$path")"
    read_lines deps < <(sed -n 's/^dep //p' <<< "$info")
    read_lines rpaths < <(sed -n 's/^rpath //p' <<< "$info")
    read_lines minos < <(sed -n 's/^minos //p' <<< "$info")
    id="$(sed -n 's/^id //p' <<< "$info")"
    sdk="$(sed -n 's/^sdk //p' <<< "$info" | xargs)"
    archs="$(lipo -archs "$path")"
    signature="$(codesign -dv "$path" 2>&1 | sed -n 's/^CodeDirectory .*flags=0x[0-9a-f]*(\([^)]*\)).*/\1/p')"
    echo "  $rel: arch $archs; minos ${minos[*]+"${minos[*]}"}, sdk ${sdk:-none};" \
        "rpath [${rpaths[*]+"${rpaths[*]}"}]; signature [${signature:-none}]"
    for dep in ${deps[@]+"${deps[@]}"}; do
        echo "    $dep"
    done
    while IFS= read -r failure; do
        failures="$failures$failure"$'\n'
    done < <(
        [ "$archs" = arm64 ] || echo "$rel has architectures [$archs]; expected [arm64]"
        dep_failures "$rel" ${deps[@]+"${deps[@]}"}
        rpath_failures "$rel" ${rpaths[@]+"${rpaths[@]}"}
        id_failures "$rel" "$id"
        minos_failures "$rel" ${minos[@]+"${minos[@]}"}
        codesign --verify --strict "$path" 2> /dev/null || echo "$rel has no valid code signature"
    )
done
if [ -n "$failures" ]; then
    printf '%s' "$failures" | sed 's/^/error: /' >&2
    echo "error: Mach-O audit failed" >&2
    exit 1
fi
echo "Mach-O audit ok: arm64 only, only OS libraries and bundled core libraries, no build-host" \
    "paths, minos $deployment_target, valid signatures; only $gpu_module links Metal"

# The relocated bundle runs without DYLD_* variables, and every image dyld
# loads (the GPU module included: auto scans it) comes from the copy or the
# OS. DYLD_PRINT_LIBRARIES only reports; it changes no lookup.
moved="$target_dir/relocation check ç"
rm -rf "$moved"
cp -R "$stage_dir" "$moved"
loaded="$target_dir/relocation-dyld.txt"
if ! (cd /tmp && DYLD_PRINT_LIBRARIES=1 "$moved/pumice-transcribe" backends --backend auto \
    --json > "$target_dir/relocation-backends.json" 2> "$loaded"); then
    grep -v '^dyld\[' "$loaded" >&2 || true
    echo "error: the relocated bundle did not run" >&2
    exit 1
fi
jq -c .result "$target_dir/relocation-backends.json" | sed 's/^/  backends: /'
# dyld4 prints `dyld[PID]: <UUID> PATH` per image (older dyld: no UUID),
# among other lines such as `move delayed to loaded: NAME`.
read_lines images < <(sed -n 's|^dyld\[[0-9]*\]: \(<[^>]*> \)\{0,1\}\(/.*\)$|\2|p' "$loaded" | sort -u)
echo "images dyld loaded (relocated, backend auto): ${#images[@]}"
outside=""
for image in ${images[@]+"${images[@]}"}; do
    case "$image" in
        "$moved"/*) echo "  bundle: ${image#"$moved"/}" ;;
        /usr/lib/* | /System/*) ;;
        *) outside="$outside  $image"$'\n' ;;
    esac
done
for rel in "${core_libs[@]}" "$cpu_module" "$gpu_module"; do
    if ! contains "$moved/$rel" ${images[@]+"${images[@]}"}; then
        echo "error: $rel was not loaded from the relocated bundle; dyld reported, for example:" >&2
        grep -F "$(basename "$rel")" "$loaded" | head -n 5 >&2 || true
        exit 1
    fi
done
if [ -n "$outside" ]; then
    printf 'error: loaded from outside the bundle and the OS:\n%s' "$outside" >&2
    exit 1
fi
rm -rf "$moved"
echo "relocation ok: the copy loads its own libraries and modules, and otherwise only OS images"
