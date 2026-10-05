"""Tests for scripts/package_release.py (development-only packaging helper)."""

import sys
import tarfile
import tempfile
import tomllib
import unittest
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import package_release

REPO_ROOT = Path(__file__).resolve().parent.parent

LINUX_TARGET = "x86_64-unknown-linux-musl"
WINDOWS_TARGET = "x86_64-pc-windows-msvc"
EXPECTED_MEMBERS = ("pumice", "LICENSE", "pumice.example.yaml", "INSTALL.md")


class PackageReleaseTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.work = Path(self.tmp.name)
        self.binary = self.work / "pumice"
        self.binary.write_bytes(b"#!/bin/sh\necho fake binary\n")

    def _expected_top(self, target):
        version = package_release.crate_version()
        return f"pumice-{version}-{target}"

    def test_tarball_membership_and_modes(self):
        archive = package_release.package(LINUX_TARGET, self.binary, self.work / "dist")
        top = self._expected_top(LINUX_TARGET)
        self.assertEqual(archive.name, f"{top}.tar.gz")

        with tarfile.open(archive) as handle:
            members = {member.name: member for member in handle.getmembers()}
            payload = handle.extractfile(f"{top}/pumice").read()

        self.assertEqual(
            sorted(members), sorted(f"{top}/{name}" for name in EXPECTED_MEMBERS)
        )
        self.assertEqual(members[f"{top}/pumice"].mode, 0o755)
        for name in ("LICENSE", "pumice.example.yaml", "INSTALL.md"):
            self.assertEqual(members[f"{top}/{name}"].mode, 0o644)
        self.assertEqual(payload, self.binary.read_bytes())

    def test_zip_membership_and_executable_mode(self):
        archive = package_release.package(WINDOWS_TARGET, self.binary, self.work / "dist")
        top = self._expected_top(WINDOWS_TARGET)
        self.assertEqual(archive.name, f"{top}.zip")

        with zipfile.ZipFile(archive) as handle:
            expected = [f"{top}/{name}" for name in EXPECTED_MEMBERS]
            expected.remove(f"{top}/pumice")
            expected.append(f"{top}/pumice.exe")
            self.assertEqual(sorted(handle.namelist()), sorted(expected))
            executable = handle.getinfo(f"{top}/pumice.exe")
            data = handle.getinfo(f"{top}/LICENSE")
            payload = handle.read(f"{top}/pumice.exe")

        # Unix mode in the high 16 bits is retained for unzip on Unix-likes.
        self.assertEqual(executable.external_attr >> 16, 0o755)
        self.assertEqual(data.external_attr >> 16, 0o644)
        self.assertEqual(payload, self.binary.read_bytes())

    def test_missing_binary_fails_clearly(self):
        with self.assertRaises(SystemExit) as caught:
            package_release.package(
                LINUX_TARGET, self.work / "does-not-exist", self.work / "dist"
            )
        self.assertIn("release binary not found", str(caught.exception))

    def test_missing_binary_with_exe_suffix_fails_clearly(self):
        with self.assertRaises(SystemExit) as caught:
            package_release.package(
                WINDOWS_TARGET, self.work / "pumice.exe", self.work / "dist"
            )
        self.assertIn("release binary not found", str(caught.exception))

    def test_unsupported_target_fails_clearly(self):
        with self.assertRaises(SystemExit) as caught:
            package_release.package(
                "i686-unknown-linux-gnu", self.binary, self.work / "dist"
            )
        message = str(caught.exception)
        self.assertIn("unsupported target 'i686-unknown-linux-gnu'", message)
        for target in package_release.SUPPORTED_TARGETS:
            self.assertIn(target, message)

    def test_every_supported_target_has_archive_name(self):
        for target in package_release.SUPPORTED_TARGETS:
            name = package_release.archive_name("1.2.3", target)
            if package_release.is_windows(target):
                self.assertTrue(name.endswith(".zip"), name)
            else:
                self.assertTrue(name.endswith(".tar.gz"), name)
            self.assertTrue(name.startswith("pumice-1.2.3-"), name)

    def test_version_matches_cargo_toml(self):
        manifest = tomllib.loads((REPO_ROOT / "Cargo.toml").read_text())
        self.assertEqual(
            package_release.crate_version(), manifest["package"]["version"]
        )


if __name__ == "__main__":
    unittest.main()
