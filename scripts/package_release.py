#!/usr/bin/env python3
"""Development-only release packaging helper for Pumice.

Builds ``pumice-{version}-{target}.zip`` (Windows) or
``pumice-{version}-{target}.tar.gz`` (Unix) with a single top-level directory
containing the release executable, LICENSE, pumice.example.yaml and INSTALL.md.
Only the explicitly listed files are packaged — never the repository or the
target directory, and never the test-only fake CLI.

Used by the release-preview workflow (.github/workflows/release.yml) and by
the local tests in scripts/test_package_release.py. Stdlib only.
"""

from __future__ import annotations

import argparse
import sys
import tarfile
import tomllib
import zipfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# Keep in sync with the build matrix in .github/workflows/release.yml.
SUPPORTED_TARGETS = frozenset(
    {
        "x86_64-pc-windows-msvc",
        "x86_64-unknown-linux-musl",
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
    }
)

# Files copied verbatim into every archive, relative to the repo root.
# INSTALL.md lives next to this script and is the archive install note.
PACKAGE_FILES = ("LICENSE", "pumice.example.yaml", "INSTALL.md")

EXECUTABLE_MODE = 0o755
DATA_MODE = 0o644


def crate_version(repo_root: Path = REPO_ROOT) -> str:
    """Reads [package] version from the workspace Cargo.toml."""
    manifest = repo_root / "Cargo.toml"
    with manifest.open("rb") as handle:
        version = tomllib.load(handle).get("package", {}).get("version")
    if not isinstance(version, str) or not version:
        raise SystemExit(f"error: no [package] version found in {manifest}")
    return version


def is_windows(target: str) -> bool:
    return target.endswith("-windows-msvc")


def executable_name(target: str) -> str:
    return "pumice.exe" if is_windows(target) else "pumice"


def archive_name(version: str, target: str) -> str:
    top = f"pumice-{version}-{target}"
    return f"{top}.zip" if is_windows(target) else f"{top}.tar.gz"


def resolve_binary(binary: Path, target: str) -> Path:
    """Accepts the executable path with or without the .exe suffix."""
    if binary.is_file():
        return binary
    if is_windows(target) and binary.suffix != ".exe":
        candidate = binary.with_name(binary.name + ".exe")
        if candidate.is_file():
            return candidate
    raise SystemExit(
        f"error: release binary not found: {binary}. "
        "Build it first (cargo build --release --locked --bin pumice "
        "--target <target>)."
    )


def package(
    target: str,
    binary: Path,
    output_dir: Path,
    repo_root: Path = REPO_ROOT,
) -> Path:
    """Builds the release archive for `target`; returns the archive path."""
    if target not in SUPPORTED_TARGETS:
        supported = "\n  ".join(sorted(SUPPORTED_TARGETS))
        raise SystemExit(
            f"error: unsupported target '{target}'\nsupported targets:\n  {supported}"
        )
    binary = resolve_binary(binary, target)
    version = crate_version(repo_root)
    top = f"pumice-{version}-{target}"

    members: list[tuple[Path, str, int]] = [
        (binary, executable_name(target), EXECUTABLE_MODE)
    ]
    for name in PACKAGE_FILES:
        source = repo_root / name
        if name == "INSTALL.md":
            source = repo_root / "scripts" / "INSTALL.md"
        if not source.is_file():
            raise SystemExit(f"error: packaged file missing: {source}")
        members.append((source, name, DATA_MODE))

    output_dir.mkdir(parents=True, exist_ok=True)
    archive = output_dir / archive_name(version, target)
    if is_windows(target):
        _write_zip(archive, top, members)
    else:
        _write_tarball(archive, top, members)
    return archive


def _write_zip(archive: Path, top: str, members) -> None:
    with zipfile.ZipFile(archive, "w") as handle:
        for source, arcname, mode in members:
            info = zipfile.ZipInfo.from_file(source, arcname=f"{top}/{arcname}")
            info.compress_type = zipfile.ZIP_DEFLATED
            # Unix mode in the high 16 bits so extracted files keep
            # their permissions on macOS/Linux unzip.
            info.external_attr = (mode & 0xFFFF) << 16
            with source.open("rb") as content:
                handle.writestr(info, content.read())


def _write_tarball(archive: Path, top: str, members) -> None:
    def normalize(member, mode):
        member.mode = mode
        member.uid = member.gid = 0
        member.uname = member.gname = ""
        return member

    with tarfile.open(archive, "w:gz") as handle:
        for source, arcname, mode in members:
            handle.add(
                source,
                arcname=f"{top}/{arcname}",
                filter=lambda member, mode=mode: normalize(member, mode),
            )


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--target", help="Rust target triple to package for")
    parser.add_argument("--binary", type=Path, help="path to the release executable")
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path("dist"),
        help="where to write the archive (default: ./dist)",
    )
    parser.add_argument(
        "--print-version",
        action="store_true",
        help="print the crate version from Cargo.toml and exit",
    )
    args = parser.parse_args(argv)

    if args.print_version:
        print(crate_version())
        return 0
    if not args.target or not args.binary:
        parser.error("--target and --binary are required")
    print(package(args.target, args.binary, args.output_dir))
    return 0


if __name__ == "__main__":
    sys.exit(main())
