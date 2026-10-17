"""Tests for scripts/package_npm.py (npm staging and packing helper).

Builds fake release archives with package_release.py and fake transcription
stages (with minimal ELF files for Linux), verifies the staged and packed
npm packages with a fake npm (so the tests stay hermetic), and checks every
failure mode: missing or extra archives, checksum problems, unsafe archive
members, and bundle trees that break the allowlist, the manifest, the modes
or the SONAME rule. The real npm run happens in CI through the release
workflow and the test-registry install tests.
"""

import hashlib
import json
import os
import shutil
import struct
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


def fake_elf(soname=None, needed=(), padding=b"") -> bytes:
    """A minimal 64-bit little-endian ELF file with a dynamic section."""
    names = ([soname] if soname else []) + list(needed)
    strtab = b"\0"
    offsets = {}
    for name in names:
        offsets[name] = len(strtab)
        strtab += name.encode() + b"\0"
    entries = [(1, offsets[name]) for name in needed]
    if soname:
        entries.append((14, offsets[soname]))
    entries.append((0, 0))
    dynamic = b"".join(struct.pack("<qQ", tag, value) for tag, value in entries)
    strtab_offset = 64
    dynamic_offset = strtab_offset + len(strtab)
    shoff = dynamic_offset + len(dynamic)
    header = struct.pack(
        "<4s5B7xHHIQQQIHHHHHH",
        b"\x7fELF", 2, 1, 1, 0, 0,
        3, 62, 1, 0, 0, shoff, 0, 64, 0, 0, 64, 3, 0,
    )
    sections = struct.pack("<IIQQQQIIQQ", *([0] * 10))
    sections += struct.pack("<IIQQQQIIQQ", 0, 3, 0, 0, strtab_offset, len(strtab), 0, 0, 1, 0)
    sections += struct.pack(
        "<IIQQQQIIQQ", 0, 6, 0, 0, dynamic_offset, len(dynamic), 1, 0, 8, 16
    )
    return header + strtab + dynamic + sections + padding


LINUX_CORE = ["libggml-base.so.0", "libggml.so.0", "libtranscribe.so.0.3"]

# Per platform: (bundle-relative path, role, content) for a fake stage.
def fake_stage_files(key: str) -> list[tuple[str, str, bytes]]:
    if key == "linux-x64":
        needed_core = ["libggml-base.so.0", "libstdc++.so.6", "libc.so.6"]
        return [
            ("pumice-transcribe", "executable", fake_elf(None, ["libtranscribe.so.0.3", "libc.so.6"], b"exe")),
            ("libtranscribe.so.0.3", "core", fake_elf("libtranscribe.so.0.3", ["libggml.so.0", "libggml-base.so.0"])),
            ("libggml.so.0", "core", fake_elf("libggml.so.0", needed_core)),
            ("libggml-base.so.0", "core", fake_elf("libggml-base.so.0", ["libc.so.6"])),
            ("cpu/x64/libggml-cpu-x64.so", "cpu", fake_elf(None, needed_core, b"x64")),
            ("cpu/haswell/libggml-cpu-haswell.so", "cpu", fake_elf(None, needed_core, b"haswell")),
            ("gpu/vulkan/libggml-vulkan.so", "gpu", fake_elf(None, [*needed_core, "libvulkan.so.1"], b"vk")),
        ]
    if key == "win32-x64":
        return [
            ("pumice-transcribe.exe", "executable", b"MZ exe"),
            ("transcribe.dll", "core", b"MZ transcribe"),
            ("ggml.dll", "core", b"MZ ggml"),
            ("ggml-base.dll", "core", b"MZ ggml-base"),
            ("cpu/x64/ggml-cpu-x64.dll", "cpu", b"MZ x64"),
            ("cpu/haswell/ggml-cpu-haswell.dll", "cpu", b"MZ haswell"),
            ("gpu/vulkan/ggml-vulkan.dll", "gpu", b"MZ vulkan"),
        ]
    return [
        ("pumice-transcribe", "executable", b"\xcf\xfa\xed\xfe exe"),
        ("libtranscribe.0.3.dylib", "core", b"\xcf\xfa\xed\xfe transcribe"),
        ("libggml.0.dylib", "core", b"\xcf\xfa\xed\xfe ggml"),
        ("libggml-base.0.dylib", "core", b"\xcf\xfa\xed\xfe base"),
        ("cpu/arm64/libggml-cpu.so", "cpu", b"\xcf\xfa\xed\xfe cpu"),
        ("gpu/metal/libggml-metal.so", "gpu", b"\xcf\xfa\xed\xfe metal"),
    ]


def write_manifest(stage: Path, key: str, entries: list[dict], **overrides) -> None:
    spec = package_npm.BUNDLES[key]
    manifest = {
        "schema_version": 1,
        "product": "pumice-transcribe",
        "version": VERSION,
        "target": spec["target"],
        "features": list(spec["features"]),
        "engine": "transcribe-cpp-sys 0.3.1",
        "files": entries,
    }
    manifest.update(overrides)
    path = stage / "bundle.json"
    path.write_text(json.dumps(manifest, indent=2) + "\n")
    path.chmod(0o644)


def make_bundles(root: Path) -> Path:
    """One fake, valid transcription stage per BUNDLES key in root/bundles."""
    bundles = root / "bundles"
    for key, spec in package_npm.BUNDLES.items():
        stage = bundles / key
        entries = []
        for rel, role, content in fake_stage_files(key):
            path = stage / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
            mode = 0o755 if role == "executable" else spec["library_mode"]
            path.chmod(mode)
            entries.append(
                {
                    "path": rel,
                    "role": role,
                    "size": len(content),
                    "sha256": hashlib.sha256(content).hexdigest(),
                }
            )
        for directory in [stage, *stage.rglob("*")]:
            if directory.is_dir():
                directory.chmod(0o755)
        write_manifest(stage, key, sorted(entries, key=lambda entry: entry["path"]))
    return bundles


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
        self.bundles = make_bundles(self.work)
        self.notices = make_notices(self.work)
        self.fake_npm = write_fake_npm(self.work)
        self.output = self.work / "npm-out"

    def pack(self, dist=None, output=None):
        return package_npm.pack(
            dist or self.dist,
            output or self.output,
            self.bundles,
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
        self.assertEqual(
            members["package/THIRD_PARTY_NOTICES.md"],
            (REPO_ROOT / "crates" / "formatter" / "THIRD_PARTY_NOTICES.md").read_bytes(),
        )
        self.assertIn(
            b"Copyright (c) 2025 CJ Pais", members["package/THIRD_PARTY_NOTICES.md"]
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
                manifest.get("libc"), ["glibc"] if key == "linux-x64" else None
            )

    def test_native_tarballs_carry_the_intact_bundle(self):
        self.pack()
        for key, executable in package_npm.TARGET_TO_NPM.values():
            filename = f"brasillero-pumice-{key}-{VERSION}.tgz"
            with tarfile.open(self.output / filename) as archive:
                modes = {member.name: member.mode & 0o777 for member in archive}
            members = read_tarball(self.output, filename)
            manifest = json.loads(members["package/package.json"])
            self.assertEqual(modes[f"package/bin/{executable}"], 0o755)
            if key not in package_npm.BUNDLES:
                # darwin-x64 stays core-only (D2).
                self.assertEqual(
                    sorted(members),
                    sorted(
                        f"package/{name}"
                        for name in (
                            f"bin/{executable}",
                            "package.json",
                            "LICENSE",
                            "THIRD_PARTY_NOTICES.md",
                        )
                    ),
                )
                self.assertEqual(manifest["files"], [f"bin/{executable}", "THIRD_PARTY_NOTICES.md"])
                continue
            stage = self.bundles / key
            bundle = json.loads(members["package/bin/bundle.json"])
            self.assertEqual(
                members["package/bin/bundle.json"], (stage / "bundle.json").read_bytes()
            )
            for entry in bundle["files"]:
                payload = members[f"package/bin/{entry['path']}"]
                self.assertEqual(len(payload), entry["size"])
                self.assertEqual(hashlib.sha256(payload).hexdigest(), entry["sha256"])
                expected_mode = (
                    0o755
                    if entry["role"] == "executable"
                    else package_npm.BUNDLES[key]["library_mode"]
                )
                self.assertEqual(modes[f"package/bin/{entry['path']}"], expected_mode)
            self.assertEqual(modes["package/bin/bundle.json"], 0o644)
            listed = {f"bin/{entry['path']}" for entry in bundle["files"]}
            self.assertEqual(
                set(manifest["files"]),
                {f"bin/{executable}", "bin/bundle.json", *listed, "THIRD_PARTY_NOTICES.md"},
            )
            self.assertEqual(
                set(members),
                {f"package/{path}" for path in manifest["files"]}
                | {"package/package.json", "package/LICENSE"},
            )

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

    # --- transcription bundle validation ---------------------------------

    def _manifest(self, key):
        return json.loads((self.bundles / key / "bundle.json").read_text())

    def _rewrite_manifest(self, key, **overrides):
        manifest = self._manifest(key)
        entries = manifest.pop("files")
        manifest.pop("schema_version")
        files = overrides.pop("files", entries)
        write_manifest(self.bundles / key, key, files, **overrides)

    def test_missing_bundle_directory_fails(self):
        shutil.rmtree(self.bundles / "darwin-arm64")
        self.assert_pack_fails("must contain exactly the transcription stages")

    def test_extra_bundle_directory_fails(self):
        (self.bundles / "darwin-x64").mkdir()
        self.assert_pack_fails("must contain exactly the transcription stages")

    def test_symlink_in_bundle_fails(self):
        (self.bundles / "linux-x64" / "libggml.so").symlink_to("libggml.so.0")
        self.assert_pack_fails("symlink libggml.so")

    def test_symlinked_stage_directory_fails(self):
        real = self.work / "real-stage"
        (self.bundles / "linux-x64").rename(real)
        (self.bundles / "linux-x64").symlink_to(real, target_is_directory=True)
        self.assert_pack_fails("not a directory (or a symlink)")

    def test_hard_link_in_bundle_fails(self):
        stage = self.bundles / "win32-x64"
        os.link(stage / "ggml.dll", stage / "cpu" / "x64" / "copy.dll")
        self.assert_pack_fails("hard link")

    def test_unlisted_file_in_bundle_fails(self):
        extra = self.bundles / "win32-x64" / "cpu" / "x64" / "extra.dll"
        extra.write_bytes(b"MZ")
        self.assert_pack_fails("not listed in bundle.json: cpu/x64/extra.dll")

    def test_unexpected_directory_in_bundle_fails(self):
        empty = self.bundles / "darwin-arm64" / "gpu" / "cuda"
        empty.mkdir()
        empty.chmod(0o755)
        self.assert_pack_fails("unexpected directories: gpu/cuda")

    def test_unsafe_name_in_bundle_fails(self):
        (self.bundles / "win32-x64" / "a b.dll").write_bytes(b"MZ")
        self.assert_pack_fails("unsafe name 'a b.dll'")

    def test_listed_file_missing_fails(self):
        (self.bundles / "linux-x64" / "cpu" / "haswell" / "libggml-cpu-haswell.so").unlink()
        self.assert_pack_fails("cpu/haswell/libggml-cpu-haswell.so is listed in bundle.json but missing")

    def test_bundle_hash_mismatch_fails(self):
        path = self.bundles / "win32-x64" / "ggml.dll"
        path.write_bytes(b"MZ gxml")
        self.assert_pack_fails("ggml.dll does not match its SHA-256")

    def test_bundle_size_mismatch_fails(self):
        path = self.bundles / "darwin-arm64" / "cpu" / "arm64" / "libggml-cpu.so"
        path.write_bytes(path.read_bytes() + b"!")
        self.assert_pack_fails("libggml-cpu.so is 9 bytes; bundle.json lists 8")

    def test_bundle_for_another_build_fails(self):
        for field, value in (
            ("version", "0.0.1"),
            ("target", "x86_64-unknown-linux-musl"),
            ("features", ["cpu-variants"]),
            ("product", "pumice"),
        ):
            with self.subTest(field=field):
                self._rewrite_manifest("linux-x64", **{field: value})
                self.assert_pack_fails(f"bundle.json {field} is")
                shutil.rmtree(self.bundles)
                self.bundles = make_bundles(self.work)

    def test_malformed_bundle_manifest_fails(self):
        (self.bundles / "linux-x64" / "bundle.json").write_text("{")
        self.assert_pack_fails("bundle.json is not JSON")

    def test_path_off_the_allowlist_fails(self):
        cases = {
            "win32-x64": ("cpu/x64/evil.dll", "cpu"),
            "linux-x64": ("libtranscribe.so", "core"),
            "darwin-arm64": ("gpu/vulkan/libggml-vulkan.so", "gpu"),
        }
        for key, (rel, role) in cases.items():
            with self.subTest(key=key):
                stage = self.bundles / key
                path = stage / rel
                path.parent.mkdir(parents=True, exist_ok=True)
                path.parent.chmod(0o755)
                path.write_bytes(b"payload")
                path.chmod(package_npm.BUNDLES[key]["library_mode"])
                files = self._manifest(key)["files"] + [
                    {
                        "path": rel,
                        "role": role,
                        "size": 7,
                        "sha256": hashlib.sha256(b"payload").hexdigest(),
                    }
                ]
                self._rewrite_manifest(key, files=files)
                self.assert_pack_fails(f"{rel!r} (role {role!r}) is not on the {key} allowlist")
                shutil.rmtree(self.bundles)
                self.bundles = make_bundles(self.work)

    def test_traversal_path_in_manifest_fails(self):
        files = self._manifest("win32-x64")["files"]
        files[0] = dict(files[0], path="../pumice.exe")
        self._rewrite_manifest("win32-x64", files=files)
        self.assert_pack_fails("is not on the win32-x64 allowlist")

    def test_missing_gpu_module_fails(self):
        stage = self.bundles / "darwin-arm64"
        shutil.rmtree(stage / "gpu")
        files = [entry for entry in self._manifest("darwin-arm64")["files"] if entry["role"] != "gpu"]
        self._rewrite_manifest("darwin-arm64", files=files)
        self.assert_pack_fails("exactly one executable and one GPU module")

    def test_missing_baseline_cpu_module_fails(self):
        stage = self.bundles / "win32-x64"
        shutil.rmtree(stage / "cpu" / "x64")
        files = [
            entry
            for entry in self._manifest("win32-x64")["files"]
            if not entry["path"].startswith("cpu/x64/")
        ]
        self._rewrite_manifest("win32-x64", files=files)
        self.assert_pack_fails("baseline CPU module cpu/x64/ggml-cpu-x64.dll missing")

    def test_posix_modes_are_required(self):
        stage = self.bundles / "linux-x64"
        (stage / "libggml.so.0").chmod(0o755)
        self.assert_pack_fails("libggml.so.0 has mode 755, expected 644")
        (stage / "libggml.so.0").chmod(0o644)
        (stage / "pumice-transcribe").chmod(0o644)
        self.assert_pack_fails("pumice-transcribe has mode 644, expected 755")
        (stage / "pumice-transcribe").chmod(0o755)
        (stage / "cpu").chmod(0o700)
        self.assert_pack_fails("directory cpu has mode 700, expected 755")

    def test_windows_modes_are_not_read_but_normalized(self):
        stage = self.bundles / "win32-x64"
        for path in stage.rglob("*"):
            if path.is_file():
                path.chmod(0o600)
        self.pack()
        filename = f"brasillero-pumice-win32-x64-{VERSION}.tgz"
        with tarfile.open(self.output / filename) as archive:
            modes = {member.name: member.mode & 0o777 for member in archive}
        self.assertEqual(modes["package/bin/pumice-transcribe.exe"], 0o755)
        self.assertEqual(modes["package/bin/ggml.dll"], 0o644)

    def test_linux_core_library_must_be_stored_under_its_soname(self):
        stage = self.bundles / "linux-x64"
        content = fake_elf("libggml.so", ["libc.so.6"])
        (stage / "libggml.so.0").write_bytes(content)
        files = self._manifest("linux-x64")["files"]
        for entry in files:
            if entry["path"] == "libggml.so.0":
                entry.update(size=len(content), sha256=hashlib.sha256(content).hexdigest())
        self._rewrite_manifest("linux-x64", files=files)
        self.assert_pack_fails("libggml.so.0 must be stored under its SONAME ('libggml.so')")

    def test_linux_bundled_needed_name_must_be_shipped(self):
        stage = self.bundles / "linux-x64"
        content = fake_elf(None, ["libtranscribe.so.0", "libc.so.6"], b"exe")
        (stage / "pumice-transcribe").write_bytes(content)
        files = self._manifest("linux-x64")["files"]
        for entry in files:
            if entry["path"] == "pumice-transcribe":
                entry.update(size=len(content), sha256=hashlib.sha256(content).hexdigest())
        self._rewrite_manifest("linux-x64", files=files)
        self.assert_pack_fails("pumice-transcribe needs libtranscribe.so.0")

    def test_linux_non_elf_library_fails(self):
        stage = self.bundles / "linux-x64"
        content = b"not an elf"
        (stage / "libggml.so.0").write_bytes(content)
        files = self._manifest("linux-x64")["files"]
        for entry in files:
            if entry["path"] == "libggml.so.0":
                entry.update(size=len(content), sha256=hashlib.sha256(content).hexdigest())
        self._rewrite_manifest("linux-x64", files=files)
        self.assert_pack_fails("libggml.so.0: not a 64-bit little-endian ELF file")

    def test_elf_reader_on_the_fake(self):
        self.assertEqual(
            package_npm.elf_dynamic(fake_elf("libx.so.1", ["liba.so", "libb.so.2"])),
            ("libx.so.1", ["liba.so", "libb.so.2"]),
        )

    def test_notices_must_cover_every_native_package(self):
        (self.notices / "darwin-x64.md").unlink()
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

    def test_cli_requires_bundles_and_notices(self):
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
