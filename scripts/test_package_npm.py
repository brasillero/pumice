"""Tests for scripts/package_npm.py (npm staging and packing helper).

Builds fake release archives with package_release.py, verifies the staged
and packed npm packages with a fake npm (so the tests stay hermetic), and
checks every failure mode: missing or extra archives, checksum problems,
and unsafe archive members. The real npm run happens in CI through the
release workflow and the test-registry install tests.
"""

import hashlib
import json
import os
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import package_npm
import package_release

REPO_ROOT = Path(__file__).resolve().parent.parent
VERSION = package_release.crate_version()

FRONT_TARBALL = f"pumice-{VERSION}.tgz"
NATIVE_TARBALLS = [
    f"brasillero-pumice-{key}-{VERSION}.tgz" for key, _ in package_npm.TARGET_TO_NPM.values()
]

# Minimal `npm pack --ignore-scripts` stand-in: tars the folder into
# <base>-<version>.tgz with a package/ prefix, like npm does.
FAKE_NPM = """#!/usr/bin/env python3
import json
import sys
import tarfile
from pathlib import Path

args = [a for a in sys.argv[1:] if a != "--ignore-scripts"]
if args != ["pack"]:
    sys.exit(f"fake npm only supports pack, got {sys.argv[1:]}")
manifest = json.loads(Path("package.json").read_text())
name = manifest["name"]
base = name[1:].replace("/", "-") if name.startswith("@") else name
filename = f"{base}-{manifest['version']}.tgz"
with tarfile.open(filename, "w:gz") as archive:
    for path in sorted(Path(".").rglob("*")):
        if path.is_file():
            archive.add(path, arcname=f"package/{path.as_posix()}")
print(filename)
"""


def make_release_dir(root: Path) -> Path:
    """Creates the four fake release archives plus SHA256SUMS in root/dist."""
    dist = root / "dist"
    dist.mkdir()
    binary = root / "pumice"
    binary.write_bytes(b"#!/bin/sh\necho pumice %s\n")
    for target in package_npm.TARGET_TO_NPM:
        package_release.package(target, binary, dist)
    lines = []
    for archive in sorted(dist.iterdir()):
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        lines.append(f"{digest}  {archive.name}")
    (dist / "SHA256SUMS").write_text("\n".join(lines) + "\n")
    return dist


def write_fake_npm(root: Path) -> Path:
    path = root / "fake-npm"
    path.write_text(FAKE_NPM)
    path.chmod(0o755)
    return path


def read_tarball(root: Path, filename: str) -> dict[str, bytes]:
    with tarfile.open(root / filename) as archive:
        return {
            member.name: archive.extractfile(member).read()
            for member in archive.getmembers()
            if member.isreg()
        }


class PackageNpmTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.work = Path(self.tmp.name)
        self.dist = make_release_dir(self.work)
        self.fake_npm = write_fake_npm(self.work)
        self.output = self.work / "npm-out"

    def pack(self, dist=None, output=None):
        return package_npm.pack(
            dist or self.dist,
            output or self.output,
            npm_bin=str(self.fake_npm),
        )

    def test_packs_five_tarballs_and_checksums(self):
        tarballs = self.pack()
        self.assertEqual(
            sorted(path.name for path in tarballs),
            sorted([FRONT_TARBALL, *NATIVE_TARBALLS]),
        )
        checksums = (self.output / "SHA256SUMS").read_text().splitlines()
        self.assertEqual(len(checksums), 5)
        for line in checksums:
            digest, name = line.split()
            self.assertEqual(
                digest,
                hashlib.sha256((self.output / name).read_bytes()).hexdigest(),
            )

    def test_front_tarball_manifest_and_contents(self):
        self.pack()
        members = read_tarball(self.output, FRONT_TARBALL)
        manifest = json.loads(members["package/package.json"])
        self.assertEqual(manifest["name"], "pumice")
        self.assertEqual(manifest["version"], VERSION)
        self.assertEqual(manifest["bin"], {"pumice": "bin/pumice.js"})
        self.assertEqual(manifest["engines"], {"node": ">=22"})
        expected_optional = {
            package_npm.native_package_name(key): VERSION
            for key, _ in package_npm.TARGET_TO_NPM.values()
        }
        self.assertEqual(manifest["optionalDependencies"], expected_optional)
        self.assertEqual(
            members["package/bin/pumice.js"],
            (REPO_ROOT / "scripts" / "npm" / "bin" / "pumice.js").read_bytes(),
        )
        self.assertEqual(
            members["package/README.md"],
            (REPO_ROOT / "scripts" / "npm" / "README.front.md").read_bytes(),
        )
        self.assertEqual(
            members["package/LICENSE"],
            (REPO_ROOT / "LICENSE").read_bytes(),
        )

    def test_native_tarballs_have_exact_binary_and_selectors(self):
        self.pack()
        for key, executable in package_npm.TARGET_TO_NPM.values():
            filename = f"brasillero-pumice-{key}-{VERSION}.tgz"
            members = read_tarball(self.output, filename)
            manifest = json.loads(members["package/package.json"])
            npm_os, _, npm_cpu = key.partition("-")
            self.assertEqual(manifest["name"], f"@brasillero/pumice-{key}")
            self.assertEqual(manifest["version"], VERSION)
            self.assertEqual(manifest["os"], [npm_os])
            self.assertEqual(manifest["cpu"], [npm_cpu])
            binary = members[f"package/bin/{executable}"]
            self.assertEqual(binary, b"#!/bin/sh\necho pumice %s\n")

    def test_missing_archive_fails(self):
        (self.dist / package_release.archive_name(VERSION, "aarch64-apple-darwin")).unlink()
        with self.assertRaises(SystemExit) as caught:
            self.pack()
        self.assertIn("missing", str(caught.exception))

    def test_extra_archive_fails(self):
        (self.dist / f"pumice-{VERSION}-i686-pc-windows-msvc.zip").write_bytes(b"junk")
        with self.assertRaises(SystemExit) as caught:
            self.pack()
        self.assertIn("unexpected", str(caught.exception))

    def test_hash_mismatch_fails(self):
        archive = self.dist / package_release.archive_name(VERSION, "x86_64-unknown-linux-musl")
        lines = (self.dist / "SHA256SUMS").read_text().splitlines()
        lines = [
            ("0" * 64 + "  " + archive.name) if line.endswith(archive.name) else line
            for line in lines
        ]
        (self.dist / "SHA256SUMS").write_text("\n".join(lines) + "\n")
        with self.assertRaises(SystemExit) as caught:
            self.pack()
        self.assertIn("SHA-256 mismatch", str(caught.exception))

    def test_checksums_must_cover_exactly_the_four_archives(self):
        lines = (self.dist / "SHA256SUMS").read_text().splitlines()
        (self.dist / "SHA256SUMS").write_text("\n".join(lines[1:]) + "\n")
        with self.assertRaises(SystemExit) as caught:
            self.pack()
        self.assertIn("SHA256SUMS", str(caught.exception))

    def test_missing_checksums_file_fails(self):
        (self.dist / "SHA256SUMS").unlink()
        with self.assertRaises(SystemExit) as caught:
            self.pack()
        self.assertIn("checksum file missing", str(caught.exception))

    def test_malformed_checksum_line_fails(self):
        lines = (self.dist / "SHA256SUMS").read_text().splitlines()
        lines[0] = "not-a-checksum " + lines[0].split()[-1]
        (self.dist / "SHA256SUMS").write_text("\n".join(lines) + "\n")
        with self.assertRaises(SystemExit) as caught:
            self.pack()
        self.assertIn("malformed line 1", str(caught.exception))

    def test_non_hex_checksum_fails(self):
        lines = (self.dist / "SHA256SUMS").read_text().splitlines()
        name = lines[0].split()[-1]
        lines[0] = "z" * 64 + "  " + name
        (self.dist / "SHA256SUMS").write_text("\n".join(lines) + "\n")
        with self.assertRaises(SystemExit) as caught:
            self.pack()
        self.assertIn("malformed line 1", str(caught.exception))

    def test_duplicate_checksum_entry_fails(self):
        lines = (self.dist / "SHA256SUMS").read_text().splitlines()
        (self.dist / "SHA256SUMS").write_text("\n".join([*lines, lines[0]]) + "\n")
        with self.assertRaises(SystemExit) as caught:
            self.pack()
        self.assertIn("duplicate entry", str(caught.exception))

    def test_print_version_does_not_need_archives_dir(self):
        self.assertEqual(package_npm.main(["--print-version"]), 0)

    def test_symlink_executable_member_fails(self):
        import io

        archive = self.dist / package_release.archive_name(
            VERSION, "x86_64-unknown-linux-musl"
        )
        top = f"pumice-{VERSION}-x86_64-unknown-linux-musl"
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w:gz") as handle:
            link = tarfile.TarInfo(f"{top}/pumice")
            link.type = tarfile.SYMTYPE
            link.linkname = "/etc/passwd"
            handle.addfile(link)
        archive.write_bytes(buffer.getvalue())
        self._rewrite_checksum(archive)
        with self.assertRaises(SystemExit) as caught:
            self.pack()
        self.assertIn("symlink", str(caught.exception))

    def _rewrite_checksum(self, archive: Path) -> None:
        lines = (self.dist / "SHA256SUMS").read_text().splitlines()
        lines = [
            f"{hashlib.sha256(archive.read_bytes()).hexdigest()}  {archive.name}"
            if line.endswith(archive.name)
            else line
            for line in lines
        ]
        (self.dist / "SHA256SUMS").write_text("\n".join(lines) + "\n")


if __name__ == "__main__":
    unittest.main()
