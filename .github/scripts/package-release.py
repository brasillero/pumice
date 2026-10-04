#!/usr/bin/env python3
"""Build-time archive packaging and extraction smoke checks; no runtime dependency."""

import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import zipfile

TARGETS = (
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-musl",
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
)
ROOT = Path(__file__).resolve().parents[2]


def checksum(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def archive_name(version, target):
    suffix = ".zip" if target.endswith("windows-msvc") else ".tar.gz"
    return f"pumice-{version}-{target}{suffix}"


def package(version, target):
    if target not in TARGETS:
        raise ValueError("unsupported release target")
    windows = target.endswith("windows-msvc")
    executable = "pumice.exe" if windows else "pumice"
    base = f"pumice-{version}-{target}"
    dist = ROOT / "dist"
    dist.mkdir(exist_ok=True)
    archive = dist / archive_name(version, target)
    with tempfile.TemporaryDirectory() as directory:
        staging = Path(directory)
        content = staging / base
        content.mkdir()
        shutil.copy2(ROOT / "target" / target / "release" / executable, content / executable)
        for name in ("LICENSE", "pumice.example.yaml"):
            shutil.copy2(ROOT / name, content / name)
        if windows:
            with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as file:
                for item in sorted(content.iterdir()):
                    file.write(item, f"{base}/{item.name}")
        else:
            with tarfile.open(archive, "w:gz") as file:
                file.add(content, arcname=base)
        extracted = staging / "extracted"
        extracted.mkdir()
        if windows:
            with zipfile.ZipFile(archive) as file:
                file.extractall(extracted)
        else:
            # This archive was just generated from three fixed files above.
            with tarfile.open(archive) as file:
                file.extractall(extracted, filter="data")
        expected = {executable, "LICENSE", "pumice.example.yaml"}
        actual = {item.name for item in (extracted / base).iterdir()}
        if actual != expected:
            raise ValueError("unexpected archive contents")
        binary = extracted / base / executable
        result = subprocess.run(
            [str(binary), "--version"], cwd=extracted, check=True,
            capture_output=True, text=True, timeout=10,
        )
        if result.stdout.strip() != f"pumice {version}":
            raise ValueError("packaged binary version does not match Cargo metadata")
        config = staging / "empty.yaml"
        config.write_text("", encoding="utf-8")
        subprocess.run(
            [str(binary), "check-config", "--config", str(config)],
            cwd=extracted, check=True, timeout=10,
        )
    archive.with_name(archive.name + ".sha256").write_text(
        f"{checksum(archive)}  {archive.name}\n", encoding="utf-8",
    )
    print(f"Packaged and smoke-checked {archive.name}")


def collect(version):
    dist = ROOT / "dist"
    expected = {archive_name(version, target) for target in TARGETS}
    expected_files = expected | {name + ".sha256" for name in expected}
    if {path.name for path in dist.iterdir()} != expected_files:
        raise ValueError("release must contain exactly all four target archives and checksums")
    lines = []
    for name in sorted(expected):
        archive = dist / name
        sidecar = dist / (name + ".sha256")
        line = f"{checksum(archive)}  {name}\n"
        if sidecar.read_text(encoding="utf-8") != line:
            raise ValueError(f"checksum mismatch for {name}")
        lines.append(line)
    (dist / "SHA256SUMS").write_text("".join(lines), encoding="utf-8")
    for name in expected:
        (dist / (name + ".sha256")).unlink()


def main():
    version = os.environ["PACKAGE_VERSION"]
    # Cargo versions are SemVer; reject path separators before using the value.
    if not version or any(char not in "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-+" for char in version):
        raise ValueError("invalid package version")
    if sys.argv[1:] == ["--collect"]:
        collect(version)
    elif not sys.argv[1:]:
        package(version, os.environ["PACKAGE_TARGET"])
    else:
        raise ValueError("usage: package-release.py [--collect]")


if __name__ == "__main__":
    main()
