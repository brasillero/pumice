"""Test archive validation, npm staging and tarball auditing with fake binaries.

Covers exact membership, modes, hashes, size limits, notices, archive checksums
and unsafe archive members. Real npm packing and installs are also checked by
the test-registry tests and cross-platform release workflow.
"""

import hashlib
import json
import os
import shutil
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

FRONT_TARBALL = f"brasillero-pumice-{VERSION}.tgz"
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
    """Creates the three fake release archives plus SHA256SUMS in root/dist."""
    dist = root / "dist"
    dist.mkdir()
    binary = root / "pumice"
    binary.write_bytes(b"#!/bin/sh\necho pumice %s\n")
    notices = root / "archive-notices.md"
    notices.write_text("# Third-party notices\n")
    for target in package_npm.TARGET_TO_NPM:
        package_release.package(target, binary, dist, notices=notices)
    lines = []
    for archive in sorted(dist.iterdir()):
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        lines.append(f"{digest}  {archive.name}")
    (dist / "SHA256SUMS").write_text("\n".join(lines) + "\n")
    return dist


def make_notices(root: Path) -> Path:
    notices = root / "notices"
    notices.mkdir()
    for key, _exe in package_npm.TARGET_TO_NPM.values():
        (notices / f"{key}.md").write_text(f"# Third-party notices — test {key}\n")
    return notices


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
        self.notices = make_notices(self.work)
        self.fake_npm = write_fake_npm(self.work)
        self.output = self.work / "npm-out"

    def pack(self, dist=None, output=None):
        return package_npm.pack(
            dist or self.dist,
            output or self.output,
            self.notices,
            npm_bin=str(self.fake_npm),
        )

    def assert_pack_fails(self, *fragments):
        with self.assertRaises(SystemExit) as caught:
            self.pack()
        for fragment in fragments:
            self.assertIn(fragment, str(caught.exception))
        self.assertFalse(
            any(self.output.glob("*.tgz")) if self.output.exists() else False,
            "nothing may be packed after a validation failure",
        )

    def test_packs_four_tarballs_and_checksums(self):
        tarballs = self.pack()
        self.assertEqual(
            sorted(path.name for path in tarballs),
            sorted([FRONT_TARBALL, *NATIVE_TARBALLS]),
        )
        checksums = (self.output / "SHA256SUMS").read_text().splitlines()
        self.assertEqual(len(checksums), 4)
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
        self.assertEqual(manifest["name"], "@brasillero/pumice")
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
            self.assertEqual(
                members["package/THIRD_PARTY_NOTICES.md"],
                (self.notices / f"{key}.md").read_bytes(),
            )
            self.assertEqual(
                manifest.get("libc"), None
            )

    def test_native_tarballs_have_only_the_relay_and_notices(self):
        self.pack()
        for key, executable in package_npm.TARGET_TO_NPM.values():
            filename = f"brasillero-pumice-{key}-{VERSION}.tgz"
            with tarfile.open(self.output / filename) as archive:
                modes = {member.name: member.mode & 0o777 for member in archive}
            members = read_tarball(self.output, filename)
            manifest = json.loads(members["package/package.json"])
            self.assertEqual(modes[f"package/bin/{executable}"], 0o755)
            self.assertEqual(set(members), {
                f"package/bin/{executable}", "package/package.json",
                "package/LICENSE", "package/THIRD_PARTY_NOTICES.md",
            })
            self.assertEqual(manifest["files"], [f"bin/{executable}", "THIRD_PARTY_NOTICES.md"])

    def test_size_report_lists_every_package(self):
        self.pack()
        report = (self.output / "PACKAGES.md").read_text()
        for name in [FRONT_TARBALL, *NATIVE_TARBALLS]:
            self.assertIn(f"| {name} |", report)

    def test_size_budget_is_enforced(self):
        original = package_npm.COMPRESSED_BUDGET
        package_npm.COMPRESSED_BUDGET = 100
        self.addCleanup(setattr, package_npm, "COMPRESSED_BUDGET", original)
        with self.assertRaises(SystemExit) as caught:
            self.pack()
        self.assertIn("the budget is 100", str(caught.exception))

    def test_tarball_with_changed_mode_fails_inspection(self):
        package = package_npm.StagedPackage("linux-x64", self.work / "staged")
        package.write("bin/pumice", b"exe", 0o755)
        tarball = self.work / "changed.tgz"
        with tarfile.open(tarball, "w:gz") as archive:
            info = tarfile.TarInfo("package/bin/pumice")
            info.size = 3
            info.mode = 0o644
            import io

            archive.addfile(info, io.BytesIO(b"exe"))
        with self.assertRaises(SystemExit) as caught:
            package_npm.inspect_tarball(tarball, package)
        self.assertIn("has mode 644, expected 755", str(caught.exception))

    def test_tarball_with_extra_member_fails_inspection(self):
        package = package_npm.StagedPackage("linux-x64", self.work / "staged")
        package.write("bin/pumice", b"exe", 0o755)
        tarball = self.work / "extra.tgz"
        with tarfile.open(tarball, "w:gz") as archive:
            archive.add(self.work / "staged" / "bin" / "pumice", arcname="package/bin/pumice")
            archive.add(self.work / "staged" / "bin" / "pumice", arcname="package/bin/extra")
        with self.assertRaises(SystemExit) as caught:
            package_npm.inspect_tarball(tarball, package)
        self.assertIn("unexpected: bin/extra", str(caught.exception))

    def test_notices_must_cover_every_native_package(self):
        (self.notices / "win32-x64.md").unlink()
        self.assert_pack_fails("must contain exactly darwin-arm64.md")

    def test_notices_must_be_generated(self):
        (self.notices / "linux-x64.md").write_text("hand-written\n")
        self.assert_pack_fails("is not a generated notices file")

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

    def test_checksums_must_cover_exactly_the_three_archives(self):
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

    def test_cli_requires_notices(self):
        import contextlib
        import io

        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
            package_npm.main(["--archives-dir", str(self.dist)])

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
