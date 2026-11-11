#!/usr/bin/env python3
"""Pack the Pumice launcher and three platform binaries for npm.

Release archives and SHA256SUMS are validated before staging. Only the exact
regular executable is extracted; links and traversal paths are rejected.
Each native package carries generated notices from --notices-dir. npm pack
runs without lifecycle scripts, and the resulting tarballs are checked for
exact file membership, permissions, hashes and size limits. Nothing is published.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import stat
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
    "aarch64-apple-darwin": ("darwin-arm64", "pumice"),
}

EXECUTABLE_MODE = 0o755
DATA_MODE = 0o644
DIR_MODE = 0o755

# Per native package, from the npm-pack report: the npm registry's own limit
# is not documented; keep each package well below common registry limits.
COMPRESSED_BUDGET = 50_000_000
UNPACKED_BUDGET = 150_000_000

LAUNCHER_SOURCE = NPM_DIR / "bin" / "pumice.js"
FRONT_README_SOURCE = NPM_DIR / "README.front.md"
LICENSE_SOURCE = REPO_ROOT / "LICENSE"
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
        "files": ["bin/pumice.js", "README.md", "LICENSE"],
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
) -> dict:
    npm_os, _, npm_cpu = platform_key.partition("-")
    manifest = {
        "name": native_package_name(platform_key),
        "version": version,
        **_repo_fields(),
        "description": (
            f"Pumice executable for {npm_os}-{npm_cpu} "
            "(installed through the pumice package; not meant for direct use)."
        ),
        "license": "MIT",
        "os": [npm_os],
        "cpu": [npm_cpu],
    }
    manifest["files"] = [f"bin/{executable}", NOTICES_NAME]
    return manifest


def verify_archives(archives_dir: Path, version: str) -> dict[str, Path]:
    """Checks the three archives and their SHA256SUMS; returns target -> path.

    Fails on a missing archive, an unexpected archive file, a checksum file
    that does not cover exactly the three archives, or any hash mismatch.
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
            f"error: {archives_dir} must contain exactly the three release "
            "archives (" + ", ".join(sorted(expected)) + "); " + "; ".join(details)
        )
    if set(recorded) != set(expected):
        raise SystemExit(
            "error: SHA256SUMS must list exactly the three release archives; "
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


def load_notices(notices_dir: Path) -> dict[str, str]:
    """``notices_dir/<platform>.md`` for exactly the three native packages."""
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


def stage_packages(
    version: str,
    binaries: dict[str, bytes],
    stage: Path,
    notices: dict[str, str],
) -> list[StagedPackage]:
    """Writes the four staged package folders; returns them front first."""
    front = StagedPackage("front", stage / "front")
    front.write("bin/pumice.js", LAUNCHER_SOURCE.read_bytes(), EXECUTABLE_MODE)
    front.write(
        "package.json", json.dumps(front_manifest(version), indent=2) + "\n", DATA_MODE
    )
    front.write("README.md", FRONT_README_SOURCE.read_text(), DATA_MODE)
    front.write("LICENSE", LICENSE_SOURCE.read_text(), DATA_MODE)

    packages = [front]
    for target, (platform_key, executable) in TARGET_TO_NPM.items():
        package = StagedPackage(platform_key, stage / platform_key)
        package.write(f"bin/{executable}", binaries[target], EXECUTABLE_MODE)
        manifest = native_manifest(version, platform_key, executable)
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
    notices_dir: Path,
    npm_bin: str = "npm",
    repo_root: Path = REPO_ROOT,
) -> list[Path]:
    """Stages and packs the four npm packages; returns the tarball paths."""
    version = package_release.crate_version(repo_root)
    archives = verify_archives(archives_dir, version)
    binaries = {
        target: extract_binary(archive, target, version)
        for target, archive in archives.items()
    }
    notices = load_notices(notices_dir)

    output_dir.mkdir(parents=True, exist_ok=True)
    rows = []
    with tempfile.TemporaryDirectory(prefix="pumice-npm-stage-") as tmp:
        packages = stage_packages(version, binaries, Path(tmp), notices)
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
        help="directory with the three release archives and SHA256SUMS "
        "(required unless --print-version)",
    )
    parser.add_argument(
        "--notices-dir",
        type=Path,
        help="directory with <platform>.md notices for the three native "
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
    if args.archives_dir is None or args.notices_dir is None:
        parser.error(
            "--archives-dir and --notices-dir are required "
            "(or use --print-version)"
        )
    for path in pack(
        args.archives_dir, args.output_dir, args.notices_dir, args.npm
    ):
        print(path)
    print((args.output_dir / "PACKAGES.md").read_text(), end="")
    return 0


if __name__ == "__main__":
    sys.exit(main())
