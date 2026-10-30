#!/usr/bin/env python3
"""CI helper for the Cargo build caches (ci-speed#1).

Swatinem/rust-cache cannot read this workspace: it runs ``cargo metadata
--all-features``, which fails because pumice-transcription names
transcribe-cpp-sys twice (the ``vulkan`` and ``metal`` aliases in
crates/transcription/Cargo.toml), and the action then cleans every dependency
artifact out of the cache. The workflows cache their target directories with
actions/cache and this helper instead (.github/actions/cargo-cache and
.github/actions/cargo-cache-save):

    ci_cargo_cache.py key --name NAME --build WHAT [--provenance]
        Prints the cache key of one job's build and its restore prefix, and
        writes both to $GITHUB_OUTPUT when set. The key covers the job, the
        runner image and architecture, rustc, cargo, the native C/C++
        compiler, what the job builds (profiles, features, targets), the
        environment that reaches the build scripts (CMake arguments, SDK pins,
        deployment target, Rust flags), .cargo/config.toml, the [profile]
        sections of the root Cargo.toml and the external packages of
        Cargo.lock. The restore prefix drops only the lockfile, so
        a dependency change still starts from the previous build.
        --provenance adds the runner image revision and CMake, for the caches
        whose output is published under a bundle identity (the three bundle
        actions); other caches survive weekly image releases.

    ci_cargo_cache.py prune TARGET_DIR...
        Removes what a later run never reuses, before the cache is saved: the
        workspace's own artifacts (a fresh checkout rebuilds them), incremental
        data, final executables and staged bundles, and the CMake build trees
        of native dependencies (their installed output stays; CMakeCache.txt
        stays for the bundle scripts' build-settings report).

Stdlib only; runs on the hosted Linux, macOS and Windows runners (Python 3.9
or newer).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import stat
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# Bump to start every cache over (v0 was Swatinem/rust-cache's namespace).
NAMESPACE = "cargo-v1"

# Environment the build scripts read (cmake-rs, cc-rs, transcribe-cpp-sys,
# the bundle scripts) or that changes rustc's output.
KEY_ENV = (
    "CARGO_ENCODED_RUSTFLAGS",
    "CC",
    "CFLAGS",
    "CMAKE_ARGS",
    "CMAKE_GENERATOR",
    "CXX",
    "CXXFLAGS",
    "MACOSX_DEPLOYMENT_TARGET",
    "RUSTFLAGS",
    "TRANSCRIBE_CMAKE_ARGS",
    "VULKAN_SDK_SHA256",
    "VULKAN_SDK_VERSION",
)

# Cargo's per-unit hash, as in deps/libfoo-<hash>.rlib and
# .fingerprint/foo-<hash>.
UNIT_DIR = re.compile(r"^(?P<package>.+)-(?P<hash>[0-9a-f]{16})$")
UNIT_FILE = re.compile(r"-(?P<hash>[0-9a-f]{16})(?:\.|$)")

PROFILE_KEEP = frozenset({"build", "deps", ".fingerprint"})
TARGET_KEEP = frozenset({"CACHEDIR.TAG", ".rustc_info.json"})


def run(args: list[str], cwd: Path | None = None) -> str:
    return subprocess.run(
        args, cwd=cwd, check=True, capture_output=True, text=True
    ).stdout.strip()


def short_hash(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()[:16]


def lockfile_dependencies(lockfile: Path) -> str:
    """The external [[package]] entries of Cargo.lock, without the workspace's
    own (their versions change with every release, not the dependencies)."""
    # Windows checkouts may convert the lockfile to CRLF.
    text = lockfile.read_text(encoding="utf-8").replace("\r\n", "\n")
    blocks = text.split("\n\n")
    return "\n\n".join(
        block.strip() for block in blocks if "\nsource = " in block
    )


def compiler_identity() -> str:
    """The C/C++ compiler the -sys crates build with."""
    system = platform.system()
    if system == "Windows":
        vswhere = (
            Path(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)"))
            / "Microsoft Visual Studio"
            / "Installer"
            / "vswhere.exe"
        )
        install = run(
            [
                str(vswhere),
                "-latest",
                "-products",
                "*",
                "-requires",
                "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                "-property",
                "installationPath",
            ]
        ).splitlines()[0]
        tools = Path(install) / "VC/Auxiliary/Build/Microsoft.VCToolsVersion.default.txt"
        return f"msvc {tools.read_text(encoding='utf-8').strip()}"
    if system == "Darwin":
        clang = run(["clang", "--version"]).splitlines()[0]
        sdk = run(["xcrun", "--sdk", "macosx", "--show-sdk-version"])
        return f"{clang}; macOS SDK {sdk}"
    return run(["cc", "--version"]).splitlines()[0]


def profile_sections(manifest: str) -> str:
    """The [profile.*] tables of a Cargo manifest, without comments or blank
    lines. They change every artifact, but no other part of the key sees
    them, and an exact hit is never saved again (cache entries are
    immutable). Include any table with an escaped header conservatively:
    TOML escapes can spell `profile` too, and an extra cache miss is safer
    than restoring stale artifacts. No TOML parser is needed on the runners."""
    keep, lines = False, []
    for line in manifest.splitlines():
        stripped = line.strip()
        if stripped.startswith("[["):
            keep = False
        elif stripped.startswith("["):
            # TOML allows `[ profile.dev ]` and `["profile".dev]`.
            first = stripped[1:].split("]", 1)[0].split(".", 1)[0]
            keep = first.strip().strip("\"'") == "profile" or "\\" in stripped
        if keep and stripped and not stripped.startswith("#"):
            lines.append(stripped)
    return "\n".join(lines)


def cache_identity(
    name: str, build: str, env: dict[str, str], root: Path, provenance: bool = False
) -> str:
    """Everything but the lockfile that makes a cached build reusable.

    With provenance, also the runner image revision and CMake: native installs
    survive pruning, so a cache whose output is published under a bundle
    identity (ci_bundle_share.py, which derives that identity from this
    function) must not restore, even by prefix, an installation another image
    or CMake built."""
    config = root / ".cargo" / "config.toml"
    manifest = root / "Cargo.toml"
    lines = [
        f"name={name}",
        f"build={build}",
        f"image={env.get('ImageOS', '')} {env.get('RUNNER_OS', '')} {env.get('RUNNER_ARCH', '')}",
    ]
    if provenance:
        lines += [
            f"image_revision={env.get('ImageVersion', '')}",
            run(["cmake", "--version"]),
        ]
    lines += [
        run(["rustc", "-vV"]),
        run(["cargo", "-V"]),
        f"compiler={compiler_identity()}",
        f"config={short_hash(config.read_text(encoding='utf-8')) if config.exists() else '-'}",
        f"profiles={short_hash(profile_sections(manifest.read_text(encoding='utf-8'))) if manifest.exists() else '-'}",
    ]
    lines += [f"{var}={env[var]}" for var in KEY_ENV if env.get(var)]
    return "\n".join(lines)


def cache_key(name: str, identity: str, dependencies: str, env: dict[str, str]) -> tuple[str, str]:
    image = env.get("ImageOS") or env.get("RUNNER_OS", "local")
    arch = env.get("RUNNER_ARCH", platform.machine())
    prefix = f"{NAMESPACE}-{name}-{image}-{arch}-{short_hash(identity)}-"
    return prefix + short_hash(dependencies), prefix


def workspace_packages(root: Path) -> set[str]:
    meta = json.loads(
        run(["cargo", "metadata", "--no-deps", "--format-version", "1"], cwd=root)
    )
    return {package["name"] for package in meta["packages"]}


def long_path(path: Path) -> str:
    # Native build trees nest past Windows' MAX_PATH; the \\?\ form has no
    # limit.
    target = os.path.abspath(path)
    if os.name == "nt" and not target.startswith("\\\\?\\"):
        target = "\\\\?\\" + target
    return target


def remove(path: Path) -> None:
    target = long_path(path)
    if path.is_symlink() or not path.is_dir():
        if os.name == "nt" and not path.is_symlink():
            os.chmod(target, stat.S_IWRITE)
        os.unlink(target)
    elif os.name != "nt":
        shutil.rmtree(target)
    elif sys.version_info >= (3, 12):
        shutil.rmtree(target, onexc=_writable_retry)
    else:
        shutil.rmtree(target, onerror=_writable_retry)


def _writable_retry(func, failed, _error) -> None:
    # Windows refuses to delete read-only files (some -sys outputs).
    os.chmod(failed, stat.S_IWRITE)
    func(failed)


def size_of(path: Path) -> int:
    total = 0
    for folder, _dirs, files in os.walk(long_path(path)):
        for name in files:
            try:
                total += os.lstat(os.path.join(folder, name)).st_size
            except OSError:
                pass
    return total


def is_profile(path: Path) -> bool:
    return (path / ".fingerprint").is_dir()


def is_target_dir(path: Path) -> bool:
    return (path / "CACHEDIR.TAG").exists() or (path / ".rustc_info.json").exists()


def prune_profile(profile: Path, packages: set[str]) -> None:
    for entry in list(profile.iterdir()):
        if entry.name not in PROFILE_KEEP:
            remove(entry)

    # The workspace's units, by Cargo's unit hash: their fingerprints and
    # build-script directories carry the package name, their outputs in deps/
    # only the target name (which can match a dependency's, e.g. `opus`).
    hashes = set()
    for folder in (profile / ".fingerprint", profile / "build"):
        if not folder.is_dir():
            continue
        for entry in list(folder.iterdir()):
            match = UNIT_DIR.match(entry.name)
            if match and match["package"] in packages:
                hashes.add(match["hash"])
                remove(entry)
    deps = profile / "deps"
    if deps.is_dir():
        for entry in list(deps.iterdir()):
            match = UNIT_FILE.search(entry.name)
            if match and match["hash"] in hashes:
                remove(entry)

    # cmake-rs builds in OUT_DIR/build and installs into OUT_DIR; a fresh
    # build script never reads the build tree again.
    build = profile / "build"
    if build.is_dir():
        for tree in build.glob("*/out/build"):
            if (tree / "CMakeCache.txt").is_file():
                for entry in list(tree.iterdir()):
                    if entry.name != "CMakeCache.txt":
                        remove(entry)


def prune_target(target: Path, packages: set[str]) -> None:
    """Prunes one target directory: its profiles, its --target <triple>
    directories, and nested target directories (target/cpu-variants)."""
    for entry in list(target.iterdir()):
        if entry.name in TARGET_KEEP:
            continue
        if entry.is_dir() and not entry.is_symlink():
            if is_target_dir(entry):
                prune_target(entry, packages)
                continue
            if is_profile(entry):
                prune_profile(entry, packages)
                continue
            profiles = [child for child in entry.iterdir() if is_profile(child)]
            if profiles:
                for child in list(entry.iterdir()):
                    if child in profiles:
                        prune_profile(child, packages)
                    else:
                        remove(child)
                continue
        remove(entry)


def command_key(args: argparse.Namespace) -> int:
    env = dict(os.environ)
    identity = cache_identity(args.name, args.build, env, REPO_ROOT, args.provenance)
    dependencies = lockfile_dependencies(REPO_ROOT / "Cargo.lock")
    key, restore = cache_key(args.name, identity, dependencies, env)
    print("cache identity:")
    print("  " + identity.replace("\n", "\n  "))
    print(f"key: {key}")
    print(f"restore prefix: {restore}")
    output = env.get("GITHUB_OUTPUT")
    if output:
        with open(output, "a", encoding="utf-8") as handle:
            handle.write(f"key={key}\nrestore-key={restore}\n")
    return 0


def command_prune(args: argparse.Namespace) -> int:
    packages = workspace_packages(REPO_ROOT)
    for target in args.target_dirs:
        if not target.is_dir():
            print(f"{target}: missing, nothing to prune")
            continue
        before = size_of(target)
        prune_target(target, packages)
        after = size_of(target)
        print(f"{target}: {before / 2**20:.0f} MiB -> {after / 2**20:.0f} MiB")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    commands = parser.add_subparsers(dest="command", required=True)
    key = commands.add_parser("key", help="print the cache key of one job's build")
    key.add_argument("--name", required=True, help="the job's cache name")
    key.add_argument(
        "--build", required=True, help="what the job builds: profiles, features, targets"
    )
    key.add_argument(
        "--provenance",
        action="store_true",
        help="add the runner image revision and CMake (caches behind a bundle identity)",
    )
    key.set_defaults(func=command_key)
    prune = commands.add_parser("prune", help="drop what a warm build never reuses")
    prune.add_argument("target_dirs", nargs="+", type=Path, metavar="TARGET_DIR")
    prune.set_defaults(func=command_prune)
    args = parser.parse_args(argv)
    if args.command == "key" and not re.fullmatch(r"[A-Za-z0-9._-]+", args.name):
        parser.error("--name may only contain letters, digits, '.', '_' and '-'")
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
