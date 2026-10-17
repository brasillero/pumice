#!/usr/bin/env python3
"""Development-only npm packaging helper for Pumice.

Stages and packs five npm packages from the S7.1 release archives and the
transcription bundles built at the same commit (p14.3-npm-bundles):

- ``@brasillero/pumice`` — the front package: a zero-dependency Node.js
  launcher (``bin/pumice.js``) plus LICENSE, THIRD_PARTY_NOTICES.md and
  README. The installed
  command stays ``pumice`` (the unscoped npm name ``pumice`` is rejected
  by the registry's similarity rule).
- ``@brasillero/pumice-{linux-x64,win32-x64,darwin-x64,darwin-arm64}`` —
  native packages. ``bin/`` holds ``pumice`` and, for the platforms in
  ``BUNDLES``, the complete ``pumice-transcribe`` stage next to it (the
  executable, its core libraries, ``cpu/``, ``gpu/`` and ``bundle.json``),
  copied byte for byte: ``pumice`` only looks for ``pumice-transcribe`` as
  its own sibling. ``darwin-x64`` stays core-only (D2). Each package carries
  LICENSE and its generated THIRD_PARTY_NOTICES.md.

``--archives-dir`` must contain exactly the four release archives produced
by ``scripts/package_release.py`` (``pumice-{version}-{target}.zip`` /
``.tar.gz``) plus their ``SHA256SUMS``. Missing archives, extra archives or
hash mismatches fail before anything is staged. Only the expected regular
binary member is extracted from each archive — never a blind ``extractall``,
and symlinks, links and traversal names are rejected.

``--bundles-dir`` must contain exactly one stage directory per ``BUNDLES``
key. Each stage is validated before anything is staged (``load_bundle``):
regular files and directories only (no symlinks, hard links or special
files, which npm would drop or mangle), every path on the platform's
allowlist and listed in ``bundle.json``, every listed file present with its
size and SHA-256, ``bundle.json`` written for this version, target and
feature, POSIX modes as staged (0755 directories and executables, 0644
data), and on Linux core libraries stored as regular files under their
SONAME with every bundled DT_NEEDED name present. ``--notices-dir`` must
hold ``<platform>.md`` for the four native packages
(``scripts/third_party_notices.py``).

The version is read from ``Cargo.toml``; package names are centralized in
this module. Each staged folder is packed with ``npm pack --ignore-scripts``
(no lifecycle scripts, no download step); every tarball is then read back
and must hold exactly the staged files, modes and bundle hashes, within the
size budget. SHA-256 checksums of the tarballs are written to ``SHA256SUMS``
and their sizes to ``PACKAGES.md`` in the output directory. Nothing is
published.

Used by the release-preview workflow (.github/workflows/release.yml) and by
the local tests in scripts/test_package_npm.py. Stdlib only.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import stat
import struct
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import package_release

REPO_ROOT = Path(__file__).resolve().parent.parent
NPM_DIR = Path(__file__).resolve().parent / "npm"

# Source repository, linked from the published package pages.
REPO_URL = "https://github.com/brasillero/pumice"

# Centralized package naming: the front package is "@brasillero/pumice"
# and every native package lives under the @brasillero scope as
# "pumice-<os>-<arch>". The installed command (npm "bin" key) stays "pumice".
FRONT_PACKAGE_NAME = "@brasillero/pumice"
FRONT_BIN_NAME = "pumice"
NATIVE_SCOPE = "@brasillero"

# Keep in sync with package_release.SUPPORTED_TARGETS and the release.yml
# build matrix. Values are ("<npm os>-<npm arch>", executable name inside the
# release archive). The static Linux musl binary runs on glibc and musl.
TARGET_TO_NPM = {
    "x86_64-unknown-linux-musl": ("linux-x64", "pumice"),
    "x86_64-pc-windows-msvc": ("win32-x64", "pumice.exe"),
    "x86_64-apple-darwin": ("darwin-x64", "pumice"),
    "aarch64-apple-darwin": ("darwin-arm64", "pumice"),
}

EXECUTABLE_MODE = 0o755
DATA_MODE = 0o644
DIR_MODE = 0o755

# Per native package, from the npm-pack report: the npm registry's own limit
# is not documented; p14.3-npm-bundles keeps every package well below it.
COMPRESSED_BUDGET = 50_000_000
UNPACKED_BUDGET = 150_000_000

# The transcription bundles shipped in the native packages
# (p14.3-npm-bundles D1–D3), keyed by npm platform key. Each is the stage of
# the transcription build for ``target`` with ``features`` (the build actions
# in .github/actions/ and the scripts in crates/transcription/scripts/).
# ``patterns`` is the allowlist, per bundle.json role, of bundle-relative
# paths (full matches); ``library_mode`` is the mode a native library is
# staged and packed with. ``posix_modes`` says whether the input tree carries
# meaningful modes (the Windows stage comes from NTFS and does not).
# darwin-x64 has no bundle yet (D2, backlog #2).
BUNDLES = {
    "win32-x64": {
        "target": "x86_64-pc-windows-msvc",
        "features": ("vulkan",),
        "executable": "pumice-transcribe.exe",
        "patterns": {
            "core": r"(transcribe|ggml|ggml-base)\.dll",
            "cpu": r"cpu/(?P<variant>[a-z0-9]+)/ggml-cpu-(?P=variant)\.dll",
            "gpu": r"gpu/vulkan/ggml-vulkan\.dll",
        },
        "baseline_cpu": "cpu/x64/ggml-cpu-x64.dll",
        "library_mode": DATA_MODE,
        "posix_modes": False,
        "elf": False,
    },
    "linux-x64": {
        "target": "x86_64-unknown-linux-gnu",
        "features": ("vulkan",),
        "executable": "pumice-transcribe",
        "patterns": {
            "core": r"lib(transcribe|ggml|ggml-base)\.so\.[0-9]+(\.[0-9]+)*",
            "cpu": r"cpu/(?P<variant>[a-z0-9]+)/libggml-cpu-(?P=variant)\.so",
            "gpu": r"gpu/vulkan/libggml-vulkan\.so",
        },
        "baseline_cpu": "cpu/x64/libggml-cpu-x64.so",
        "library_mode": DATA_MODE,
        "posix_modes": True,
        "elf": True,
    },
    "darwin-arm64": {
        "target": "aarch64-apple-darwin",
        "features": ("metal",),
        "executable": "pumice-transcribe",
        "patterns": {
            "core": r"lib(transcribe|ggml|ggml-base)\.[0-9]+(\.[0-9]+)*\.dylib",
            "cpu": r"cpu/arm64/libggml-cpu\.so",
            "gpu": r"gpu/metal/libggml-metal\.so",
        },
        "baseline_cpu": "cpu/arm64/libggml-cpu.so",
        # The Mach-O libraries are staged executable (build-macos-metal.sh).
        "library_mode": EXECUTABLE_MODE,
        "posix_modes": True,
        "elf": False,
    },
}
CORE_LIBRARIES = frozenset({"transcribe", "ggml", "ggml-base"})
BUNDLE_MANIFEST = "bundle.json"
BUNDLE_PRODUCT = "pumice-transcribe"
SAFE_COMPONENT = re.compile(r"[A-Za-z0-9][A-Za-z0-9._+-]*")

LAUNCHER_SOURCE = NPM_DIR / "bin" / "pumice.js"
FRONT_README_SOURCE = NPM_DIR / "README.front.md"
LICENSE_SOURCE = REPO_ROOT / "LICENSE"
# License notices for third-party content embedded in the binary (the
# Handy-derived built-in formatter prompt). Shipped in every package: the
# front package documents the install and the native packages carry the
# executable itself.
NOTICES_SOURCE = REPO_ROOT / "crates" / "formatter" / "THIRD_PARTY_NOTICES.md"
NOTICES_NAME = "THIRD_PARTY_NOTICES.md"


def native_package_name(platform_key: str) -> str:
    """``linux-x64`` -> ``@brasillero/pumice-linux-x64``."""
    short_name = FRONT_PACKAGE_NAME.rsplit("/", 1)[-1]
    return f"{NATIVE_SCOPE}/{short_name}-{platform_key}"


def _repo_fields() -> dict:
    return {
        "repository": {
            "type": "git",
            "url": f"git+{REPO_URL}.git",
        },
        "homepage": REPO_URL,
        "bugs": {"url": f"{REPO_URL}/issues"},
    }


def front_manifest(version: str) -> dict:
    return {
        "name": FRONT_PACKAGE_NAME,
        "version": version,
        **_repo_fields(),
        "description": (
            "Polish your dictation with the AI subscriptions you already "
            "have. A small local tool that connects dictation apps such as "
            "Handy to the AI assistants you already pay for, through their "
            "official command-line apps."
        ),
        "license": "MIT",
        "type": "commonjs",
        "bin": {FRONT_BIN_NAME: "bin/pumice.js"},
        "engines": {"node": ">=22"},
        "files": ["bin/pumice.js", "README.md", "LICENSE", NOTICES_NAME],
        # Exact-version pins to the matching native packages; managers
        # install only the one matching the current os/cpu.
        "optionalDependencies": {
            native_package_name(platform_key): version
            for platform_key, _exe in TARGET_TO_NPM.values()
        },
    }


def native_manifest(
    version: str,
    platform_key: str,
    executable: str,
    bundle_paths: list[str] | None = None,
) -> dict:
    npm_os, _, npm_cpu = platform_key.partition("-")
    contents = (
        "executable and transcription bundle" if bundle_paths else "executable"
    )
    manifest = {
        "name": native_package_name(platform_key),
        "version": version,
        **_repo_fields(),
        "description": (
            f"Pumice native {contents} for {npm_os}-{npm_cpu} "
            "(installed through the pumice package; not meant for direct use)."
        ),
        "license": "MIT",
        "os": [npm_os],
        "cpu": [npm_cpu],
    }
    # The Linux bundle needs glibc 2.35 or newer; npm skips the package on
    # musl systems instead of installing a bundle that cannot start (D1).
    if platform_key == "linux-x64":
        manifest["libc"] = ["glibc"]
    manifest["files"] = [
        f"bin/{executable}",
        *(f"bin/{path}" for path in bundle_paths or []),
        NOTICES_NAME,
    ]
    return manifest


def verify_archives(archives_dir: Path, version: str) -> dict[str, Path]:
    """Checks the four archives and their SHA256SUMS; returns target -> path.

    Fails on a missing archive, an unexpected archive file, a checksum file
    that does not cover exactly the four archives, or any hash mismatch.
    """
    expected = {
        package_release.archive_name(version, target): target
        for target in TARGET_TO_NPM
    }
    checksums_file = archives_dir / "SHA256SUMS"
    if not checksums_file.is_file():
        raise SystemExit(f"error: checksum file missing: {checksums_file}")

    recorded: dict[str, str] = {}
    for lineno, line in enumerate(checksums_file.read_text().splitlines(), start=1):
        if not line.strip():
            continue
        parts = line.split()
        digest = parts[0] if parts else ""
        name = parts[1].lstrip("*") if len(parts) == 2 else ""
        if (
            len(parts) != 2
            or len(digest) != 64
            or any(char not in "0123456789abcdefABCDEF" for char in digest)
            or not name
        ):
            raise SystemExit(
                f"error: malformed line {lineno} in {checksums_file}: "
                f"{line!r} (expected '<sha256>  <archive>')"
            )
        if name in recorded:
            raise SystemExit(
                f"error: duplicate entry for {name} in {checksums_file}"
            )
        recorded[name] = digest.lower()

    archives = {
        path.name: path
        for path in archives_dir.iterdir()
        if path.is_file() and path.name != "SHA256SUMS"
    }
    missing = sorted(set(expected) - set(archives))
    extra = sorted(set(archives) - set(expected))
    if missing or extra:
        details = []
        if missing:
            details.append("missing: " + ", ".join(missing))
        if extra:
            details.append("unexpected: " + ", ".join(extra))
        raise SystemExit(
            f"error: {archives_dir} must contain exactly the four release "
            "archives (" + ", ".join(sorted(expected)) + "); " + "; ".join(details)
        )
    if set(recorded) != set(expected):
        raise SystemExit(
            "error: SHA256SUMS must list exactly the four release archives; "
            f"found: {sorted(recorded)}"
        )

    resolved: dict[str, Path] = {}
    for name, target in expected.items():
        path = archives[name]
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        if digest != recorded[name]:
            raise SystemExit(
                f"error: SHA-256 mismatch for {name}: "
                f"SHA256SUMS says {recorded[name]}, computed {digest}"
            )
        resolved[target] = path
    return resolved


def extract_binary(archive: Path, target: str, version: str) -> bytes:
    """Returns the executable bytes from a release archive.

    Only the single expected regular file ``pumice-{version}-{target}/<exe>``
    is read. Symlinks, hard links, device nodes and names that would escape
    the top directory are rejected, and nothing is written to disk.
    """
    platform_key, executable = TARGET_TO_NPM[target]
    top = f"pumice-{version}-{target}"
    member_name = f"{top}/{executable}"
    if ".." in Path(member_name).parts:
        raise SystemExit(f"error: unsafe member name in {archive.name}")

    def reject(kind: str) -> SystemExit:
        return SystemExit(
            f"error: {archive.name}: unexpected {kind} member {member_name!r}; "
            "refusing to package"
        )

    if package_release.is_windows(target):
        with zipfile.ZipFile(archive) as handle:
            infos = {info.filename: info for info in handle.infolist()}
            if member_name not in infos:
                raise reject("missing")
            info = infos[member_name]
            mode = info.external_attr >> 16
            if mode & 0o120000 == 0o120000:
                raise reject("symlink")
            if info.is_dir():
                raise reject("directory")
            if mode & 0o170000 not in (0, 0o100000):
                raise reject("non-regular")
            payload = handle.read(member_name)
    else:
        with tarfile.open(archive) as handle:
            members = {member.name: member for member in handle.getmembers()}
            if member_name not in members:
                raise reject("missing")
            member = members[member_name]
            if member.issym():
                raise reject("symlink")
            if member.islnk():
                raise reject("hard link")
            if not member.isreg():
                raise reject("non-regular")
            source = handle.extractfile(member)
            if source is None:
                raise reject("unreadable")
            payload = source.read()
    if not payload:
        raise SystemExit(f"error: {archive.name}: executable {member_name} is empty")
    return payload


def elf_dynamic(data: bytes) -> tuple[str | None, list[str]]:
    """(DT_SONAME, [DT_NEEDED]) of a 64-bit little-endian ELF file.

    Reads the section headers, which stripped release files keep. Raises
    ValueError for anything else.
    """
    if data[:4] != b"\x7fELF" or data[4:6] != b"\x02\x01":
        raise ValueError("not a 64-bit little-endian ELF file")
    shoff, = struct.unpack_from("<Q", data, 0x28)
    shentsize, shnum = struct.unpack_from("<HH", data, 0x3A)
    if not shoff or shentsize != 64 or shoff + shnum * 64 > len(data):
        raise ValueError("no usable ELF section headers")
    sections = [
        struct.unpack_from("<IIQQQQIIQQ", data, shoff + index * 64)
        for index in range(shnum)
    ]
    soname, needed = None, []
    for section in sections:
        if section[1] != 6:  # SHT_DYNAMIC
            continue
        strtab = sections[section[6]]
        offset, size = section[4], section[5]

        def string(value: int) -> str:
            start = strtab[4] + value
            end = data.index(b"\0", start)
            return data[start:end].decode("utf-8", errors="replace")

        for entry in range(offset, offset + size, 16):
            tag, value = struct.unpack_from("<qQ", data, entry)
            if tag == 0:  # DT_NULL
                break
            if tag == 1:  # DT_NEEDED
                needed.append(string(value))
            elif tag == 14:  # DT_SONAME
                soname = string(value)
    return soname, needed


def _reject(stage: Path, message: str) -> SystemExit:
    return SystemExit(f"error: transcription bundle {stage}: {message}; refusing to package")


def load_bundle(stage: Path, platform_key: str, version: str) -> list[dict]:
    """Validates one transcription stage; returns its files in path order.

    Each entry is {"path", "source", "mode", "size", "sha256"}, with
    ``bundle.json`` itself included. Nothing is written.
    """
    spec = BUNDLES[platform_key]
    root_info = stage.lstat() if stage.exists() or stage.is_symlink() else None
    if root_info is None or not stat.S_ISDIR(root_info.st_mode):
        raise _reject(stage, "not a directory (or a symlink)")

    files: dict[str, Path] = {}
    dirs: set[str] = set()
    pending = [stage]
    while pending:
        directory = pending.pop()
        for entry in sorted(directory.iterdir()):
            rel = entry.relative_to(stage).as_posix()
            if any(not SAFE_COMPONENT.fullmatch(part) for part in rel.split("/")):
                raise _reject(stage, f"unsafe name {rel!r}")
            info = entry.lstat()
            if stat.S_ISLNK(info.st_mode):
                raise _reject(stage, f"symlink {rel} (npm drops symlinks)")
            if stat.S_ISDIR(info.st_mode):
                if spec["posix_modes"] and stat.S_IMODE(info.st_mode) != DIR_MODE:
                    raise _reject(
                        stage,
                        f"directory {rel} has mode {stat.S_IMODE(info.st_mode):o}, "
                        f"expected {DIR_MODE:o}",
                    )
                dirs.add(rel)
                pending.append(entry)
            elif stat.S_ISREG(info.st_mode):
                if info.st_nlink != 1:
                    raise _reject(stage, f"hard link {rel}")
                files[rel] = entry
            else:
                raise _reject(stage, f"special file {rel}")

    if BUNDLE_MANIFEST not in files:
        raise _reject(stage, f"{BUNDLE_MANIFEST} missing")
    try:
        manifest = json.loads(files[BUNDLE_MANIFEST].read_text())
    except (ValueError, UnicodeDecodeError) as error:
        raise _reject(stage, f"{BUNDLE_MANIFEST} is not JSON ({error})") from None
    expected_identity = {
        "schema_version": 1,
        "product": BUNDLE_PRODUCT,
        "version": version,
        "target": spec["target"],
        "features": list(spec["features"]),
    }
    if not isinstance(manifest, dict):
        raise _reject(stage, f"{BUNDLE_MANIFEST} is not an object")
    for field, value in expected_identity.items():
        if manifest.get(field) != value:
            raise _reject(
                stage,
                f"{BUNDLE_MANIFEST} {field} is {manifest.get(field)!r}, expected {value!r}",
            )
    listed = manifest.get("files")
    if not isinstance(listed, list) or not listed:
        raise _reject(stage, f"{BUNDLE_MANIFEST} lists no files")

    patterns = {role: re.compile(pattern) for role, pattern in spec["patterns"].items()}
    entries: dict[str, dict] = {}
    core_names: list[str] = []
    for item in listed:
        if not isinstance(item, dict) or set(item) != {"path", "role", "size", "sha256"}:
            raise _reject(stage, f"malformed {BUNDLE_MANIFEST} entry {item!r}")
        path, role = item["path"], item["role"]
        if not isinstance(path, str) or path in entries:
            raise _reject(stage, f"duplicate or invalid path {path!r} in {BUNDLE_MANIFEST}")
        if role == "executable":
            allowed = path == spec["executable"]
        elif role in patterns:
            match = patterns[role].fullmatch(path)
            allowed = match is not None
            if allowed and role == "core":
                core_names.append(match.group(1))
        else:
            allowed = False
        if not allowed:
            raise _reject(stage, f"{path!r} (role {role!r}) is not on the {platform_key} allowlist")
        if path not in files:
            raise _reject(stage, f"{path} is listed in {BUNDLE_MANIFEST} but missing")
        entries[path] = item

    unlisted = sorted(set(files) - set(entries) - {BUNDLE_MANIFEST})
    if unlisted:
        raise _reject(stage, f"files not listed in {BUNDLE_MANIFEST}: {', '.join(unlisted)}")
    expected_dirs = {
        "/".join(path.split("/")[:depth])
        for path in entries
        for depth in range(1, path.count("/") + 1)
    }
    if dirs != expected_dirs:
        raise _reject(
            stage, f"unexpected directories: {', '.join(sorted(dirs - expected_dirs))}"
        )
    roles = [item["role"] for item in entries.values()]
    if roles.count("executable") != 1 or roles.count("gpu") != 1:
        raise _reject(stage, "expected exactly one executable and one GPU module")
    if sorted(core_names) != sorted(CORE_LIBRARIES):
        raise _reject(stage, f"core libraries {sorted(core_names)}, expected {sorted(CORE_LIBRARIES)}")
    if spec["baseline_cpu"] not in entries:
        raise _reject(stage, f"baseline CPU module {spec['baseline_cpu']} missing")

    result = []
    elf_needed: dict[str, list[str]] = {}
    for path in sorted(files):
        source = files[path]
        role = entries[path]["role"] if path in entries else None
        mode = (
            DATA_MODE
            if role is None
            else EXECUTABLE_MODE
            if role == "executable"
            else spec["library_mode"]
        )
        actual_mode = stat.S_IMODE(source.lstat().st_mode)
        if spec["posix_modes"] and actual_mode != mode:
            raise _reject(stage, f"{path} has mode {actual_mode:o}, expected {mode:o}")
        payload = source.read_bytes()
        digest = hashlib.sha256(payload).hexdigest()
        if role is not None:
            if len(payload) != entries[path]["size"]:
                raise _reject(
                    stage,
                    f"{path} is {len(payload)} bytes; {BUNDLE_MANIFEST} lists {entries[path]['size']}",
                )
            if digest != entries[path]["sha256"]:
                raise _reject(stage, f"{path} does not match its SHA-256 in {BUNDLE_MANIFEST}")
            if spec["elf"]:
                try:
                    soname, needed = elf_dynamic(payload)
                except ValueError as error:
                    raise _reject(stage, f"{path}: {error}") from None
                if role == "core" and soname != path:
                    raise _reject(
                        stage,
                        f"{path} must be stored under its SONAME ({soname!r}); "
                        "npm drops the symlinks a library install would use",
                    )
                elf_needed[path] = needed
        result.append(
            {
                "path": path,
                "source": source,
                "mode": mode,
                "size": len(payload),
                "sha256": digest,
            }
        )
    core_pattern = patterns["core"]
    for path, needed in elf_needed.items():
        for name in needed:
            if core_pattern.fullmatch(name) and name not in entries:
                raise _reject(stage, f"{path} needs {name}, which the bundle does not ship")
    return result


def load_bundles(bundles_dir: Path, version: str) -> dict[str, list[dict]]:
    """Validates ``bundles_dir/<platform>/`` for exactly the BUNDLES keys."""
    if not bundles_dir.is_dir():
        raise SystemExit(f"error: bundles directory missing: {bundles_dir}")
    present = {path.name for path in bundles_dir.iterdir()}
    if present != set(BUNDLES):
        raise SystemExit(
            f"error: {bundles_dir} must contain exactly the transcription stages "
            f"{', '.join(sorted(BUNDLES))}; found: {', '.join(sorted(present)) or 'nothing'}"
        )
    return {
        key: load_bundle(bundles_dir / key, key, version) for key in sorted(BUNDLES)
    }


def load_notices(notices_dir: Path) -> dict[str, str]:
    """``notices_dir/<platform>.md`` for exactly the four native packages."""
    expected = {f"{key}.md": key for key, _exe in TARGET_TO_NPM.values()}
    present = (
        {path.name for path in notices_dir.iterdir()} if notices_dir.is_dir() else set()
    )
    if present != set(expected):
        raise SystemExit(
            f"error: {notices_dir} must contain exactly {', '.join(sorted(expected))} "
            "(scripts/third_party_notices.py)"
        )
    notices = {}
    for name, key in expected.items():
        text = (notices_dir / name).read_text()
        if not text.startswith("# Third-party notices"):
            raise SystemExit(f"error: {notices_dir / name} is not a generated notices file")
        notices[key] = text
    return notices


def _write(path: Path, content: str | bytes, mode: int) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if isinstance(content, str):
        path.write_text(content)
    else:
        path.write_bytes(content)
    path.chmod(mode)


class StagedPackage:
    """One staged package folder and what its tarball must hold."""

    def __init__(self, key: str, folder: Path):
        self.key = key
        self.folder = folder
        # package-relative path -> (mode, sha256)
        self.expected: dict[str, tuple[int, str]] = {}

    def write(self, rel: str, content: str | bytes, mode: int) -> None:
        payload = content.encode() if isinstance(content, str) else content
        _write(self.folder / rel, payload, mode)
        self.expected[rel] = (mode, hashlib.sha256(payload).hexdigest())

    def copy(self, rel: str, item: dict) -> None:
        destination = self.folder / rel
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(item["source"], destination)
        destination.chmod(item["mode"])
        self.expected[rel] = (item["mode"], item["sha256"])


def stage_packages(
    version: str,
    binaries: dict[str, bytes],
    stage: Path,
    bundles: dict[str, list[dict]],
    notices: dict[str, str],
) -> list[StagedPackage]:
    """Writes the five staged package folders; returns them front first."""
    front = StagedPackage("front", stage / "front")
    front.write("bin/pumice.js", LAUNCHER_SOURCE.read_bytes(), EXECUTABLE_MODE)
    front.write(
        "package.json", json.dumps(front_manifest(version), indent=2) + "\n", DATA_MODE
    )
    front.write("README.md", FRONT_README_SOURCE.read_text(), DATA_MODE)
    front.write("LICENSE", LICENSE_SOURCE.read_text(), DATA_MODE)
    front.write(NOTICES_NAME, NOTICES_SOURCE.read_text(), DATA_MODE)

    packages = [front]
    for target, (platform_key, executable) in TARGET_TO_NPM.items():
        package = StagedPackage(platform_key, stage / platform_key)
        bundle = bundles.get(platform_key, [])
        if any(item["path"] == executable for item in bundle):
            raise SystemExit(f"error: the {platform_key} bundle would overwrite bin/{executable}")
        package.write(f"bin/{executable}", binaries[target], EXECUTABLE_MODE)
        for item in bundle:
            package.copy(f"bin/{item['path']}", item)
        manifest = native_manifest(
            version, platform_key, executable, [item["path"] for item in bundle]
        )
        package.write("package.json", json.dumps(manifest, indent=2) + "\n", DATA_MODE)
        package.write("LICENSE", LICENSE_SOURCE.read_text(), DATA_MODE)
        package.write(NOTICES_NAME, notices[platform_key], DATA_MODE)
        packages.append(package)
    return packages


def inspect_tarball(tarball: Path, package: StagedPackage) -> dict:
    """Checks a packed tarball against what was staged; returns its sizes.

    Every member must be a regular file under ``package/`` that was staged,
    with the staged mode and content, and nothing staged may be missing.
    Native packages must stay within the size budget.
    """
    seen: dict[str, tuple[int, str]] = {}
    unpacked = 0
    with tarfile.open(tarball) as archive:
        for member in archive:
            name = member.name
            if not member.isreg() or not name.startswith("package/"):
                raise SystemExit(f"error: {tarball.name}: unexpected member {name!r}")
            rel = name.removeprefix("package/")
            if rel in seen or ".." in rel.split("/"):
                raise SystemExit(f"error: {tarball.name}: duplicate or unsafe member {name!r}")
            source = archive.extractfile(member)
            digest = hashlib.sha256()
            while chunk := source.read(1 << 20):
                digest.update(chunk)
            seen[rel] = (stat.S_IMODE(member.mode), digest.hexdigest())
            unpacked += member.size
    missing = sorted(set(package.expected) - set(seen))
    extra = sorted(set(seen) - set(package.expected))
    if missing or extra:
        raise SystemExit(
            f"error: {tarball.name} does not hold exactly the staged files "
            f"(missing: {', '.join(missing) or 'none'}; unexpected: {', '.join(extra) or 'none'})"
        )
    for rel, (mode, digest) in package.expected.items():
        if seen[rel][0] != mode:
            raise SystemExit(
                f"error: {tarball.name}: {rel} has mode {seen[rel][0]:o}, expected {mode:o}"
            )
        if seen[rel][1] != digest:
            raise SystemExit(f"error: {tarball.name}: {rel} differs from the staged file")
    compressed = tarball.stat().st_size
    if package.key != "front" and (
        compressed > COMPRESSED_BUDGET or unpacked > UNPACKED_BUDGET
    ):
        raise SystemExit(
            f"error: {tarball.name} is {compressed} bytes packed / {unpacked} unpacked; "
            f"the budget is {COMPRESSED_BUDGET} / {UNPACKED_BUDGET}"
        )
    return {
        "package": tarball.name,
        "files": len(seen),
        "compressed": compressed,
        "unpacked": unpacked,
    }


def size_report(rows: list[dict]) -> str:
    lines = [
        "| Package | Files | Packed bytes | Unpacked bytes |",
        "| --- | ---: | ---: | ---: |",
    ]
    for row in rows:
        lines.append(
            f"| {row['package']} | {row['files']} | {row['compressed']} | {row['unpacked']} |"
        )
    lines.append(
        f"\nBudget per native package: {COMPRESSED_BUDGET} packed, {UNPACKED_BUDGET} unpacked."
    )
    return "\n".join(lines) + "\n"


def resolve_npm_command(npm_bin: str) -> list[str]:
    """Builds an argv for invoking npm without shell strings.

    When ``npm_bin`` names a Windows ``.cmd``/``.bat``/``.exe`` shim (or PATH
    resolves it to one), prefer running ``npm-cli.js`` through Node directly:
    it avoids CreateProcess quoting surprises and never spawns an
    uncontrolled shell. Falls back to the resolved executable otherwise.
    """
    resolved = shutil.which(npm_bin) or npm_bin
    if resolved.lower().endswith((".cmd", ".bat", ".exe")):
        npm_cli = (
            Path(resolved).parent / "node_modules" / "npm" / "bin" / "npm-cli.js"
        )
        node = shutil.which("node")
        if node and npm_cli.is_file():
            return [node, str(npm_cli)]
    return [resolved]


def pack_folder(folder: Path, npm_bin: str, output_dir: Path) -> Path:
    """Runs ``npm pack --ignore-scripts`` and moves the tarball to output."""
    result = subprocess.run(
        [*resolve_npm_command(npm_bin), "pack", "--ignore-scripts"],
        cwd=folder,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise SystemExit(
            f"error: npm pack failed in {folder}:\n{result.stdout}{result.stderr}"
        )
    produced = [line for line in result.stdout.splitlines() if line.endswith(".tgz")]
    if len(produced) != 1:
        raise SystemExit(
            f"error: npm pack in {folder} produced {len(produced)} tarballs; "
            f"output was:\n{result.stdout}"
        )
    tarball = folder / produced[-1].strip()
    if not tarball.is_file():
        raise SystemExit(f"error: npm pack did not create {tarball}")
    destination = output_dir / tarball.name
    shutil.move(str(tarball), destination)
    return destination


def pack(
    archives_dir: Path,
    output_dir: Path,
    bundles_dir: Path,
    notices_dir: Path,
    npm_bin: str = "npm",
    repo_root: Path = REPO_ROOT,
) -> list[Path]:
    """Stages and packs the five npm packages; returns the tarball paths."""
    version = package_release.crate_version(repo_root)
    archives = verify_archives(archives_dir, version)
    binaries = {
        target: extract_binary(archive, target, version)
        for target, archive in archives.items()
    }
    bundles = load_bundles(bundles_dir, version)
    notices = load_notices(notices_dir)

    output_dir.mkdir(parents=True, exist_ok=True)
    rows = []
    with tempfile.TemporaryDirectory(prefix="pumice-npm-stage-") as tmp:
        packages = stage_packages(version, binaries, Path(tmp), bundles, notices)
        tarballs = []
        for package in packages:
            tarball = pack_folder(package.folder, npm_bin, output_dir)
            rows.append(inspect_tarball(tarball, package))
            tarballs.append(tarball)

    lines = [
        f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}"
        for path in tarballs
    ]
    (output_dir / "SHA256SUMS").write_text("\n".join(lines) + "\n")
    (output_dir / "PACKAGES.md").write_text(size_report(rows))
    return tarballs


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--archives-dir",
        type=Path,
        help="directory with the four release archives and SHA256SUMS "
        "(required unless --print-version)",
    )
    parser.add_argument(
        "--bundles-dir",
        type=Path,
        help="directory with one transcription stage per bundled platform "
        f"({', '.join(sorted(BUNDLES))}); required unless --print-version",
    )
    parser.add_argument(
        "--notices-dir",
        type=Path,
        help="directory with <platform>.md notices for the four native "
        "packages (scripts/third_party_notices.py); required unless --print-version",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path("dist/npm"),
        help="where to write the tarballs and SHA256SUMS (default: ./dist/npm)",
    )
    parser.add_argument(
        "--npm",
        default="npm",
        help="npm executable used for packing (default: npm)",
    )
    parser.add_argument(
        "--print-version",
        action="store_true",
        help="print the crate version from Cargo.toml and exit",
    )
    args = parser.parse_args(argv)

    if args.print_version:
        print(package_release.crate_version())
        return 0
    if args.archives_dir is None or args.bundles_dir is None or args.notices_dir is None:
        parser.error(
            "--archives-dir, --bundles-dir and --notices-dir are required "
            "(or use --print-version)"
        )
    for path in pack(
        args.archives_dir, args.output_dir, args.bundles_dir, args.notices_dir, args.npm
    ):
        print(path)
    print((args.output_dir / "PACKAGES.md").read_text(), end="")
    return 0


if __name__ == "__main__":
    sys.exit(main())
