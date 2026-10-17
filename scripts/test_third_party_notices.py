"""Tests for scripts/third_party_notices.py (combined notices generator).

A fake ``cargo`` answers ``cargo metadata`` with a small crate graph whose
sources live in a temporary directory, so the tests never touch the real
registry: the walk keeps normal dependencies only, workspace members and
dev/build dependencies stay out, identical texts print once, a crate
without a license file gets its declared license's standard text, and the
native components follow the platform's bundle (Vulkan, Metal, musl, or
none for the core-only Intel Mac package), every package carries the pinned
Rust standard library notices for its targets (a fake ``rustc`` reports the
pinned version; another version fails), and MPL crates get links to their
exact source. The real run happens in CI.
"""

import contextlib
import io
import json
import re
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
ROOT = Path(__file__).resolve().parent.parent

import third_party_notices

FAKE_CARGO = """#!/usr/bin/env python3
import json
import sys
from pathlib import Path

args = sys.argv[1:]
assert args[:5] == ["metadata", "--format-version", "1", "--locked", "--filter-platform"], args
target = args[5]
features = args[7] if len(args) > 7 else ""
graph = json.loads(Path(__file__).with_name("graph.json").read_text())
log = Path(__file__).with_name("calls.log")
with log.open("a") as handle:
    handle.write(f"{target} {features}\\n")
print(json.dumps(graph))
"""

FAKE_RUSTC = """#!/bin/sh
echo "rustc $(cat "$(dirname "$0")/rustc-version") (b940084d7 2026-09-28)"
"""

MIT_ALICE = "MIT License\n\nCopyright (c) Alice\n\nPermission is hereby granted...\n"


class NoticesTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.crates = self.root / "crates"
        self.packages = []
        self.nodes = []
        self.add("pumice", deps=[("serde", None), ("tempfile", "dev"), ("cc", "build"), ("pumice-relay", None)])
        self.add("pumice-relay", deps=[("serde", None)])
        self.add(
            "pumice-transcription",
            deps=[("transcribe-cpp-sys", None), ("opusic-sys", None), ("ebml", None), ("serde", None)],
        )
        self.add("serde", files={"LICENSE-MIT": MIT_ALICE, "LICENSE-APACHE": "Apache text\n"}, deps=[("serde_derive", None)])
        self.add("serde_derive", files={"LICENSE-MIT": MIT_ALICE, "LICENSE-APACHE": "Apache text\n"})
        self.add("tempfile", files={"LICENSE": "dev only\n"})
        self.add("cc", files={"LICENSE": "build only\n"})
        self.add("ebml", files={}, license="MIT", authors=["Austin Blake"])
        self.add(
            "transcribe-cpp-sys",
            files={
                "LICENSE": "transcribe.cpp MIT\n",
                "ggml/LICENSE": "ggml MIT ```fenced```\n",
                "src/third_party/miniz/LICENSE": "miniz\n",
                "ggml/src/ggml-cpu/llamafile/sgemm.cpp": "// Copyright 2024 Mozilla Foundation\n//\n// Permission granted.\n\nint x;\n",
            },
        )
        self.add("opusic-sys", files={"LICENSE": "opusic\n", "opus/COPYING": "Opus BSD\n"})
        self.write_graph()
        self.cargo = self.root / "fake-cargo"
        self.cargo.write_text(FAKE_CARGO)
        self.cargo.chmod(0o755)
        self.rustc = self.root / "fake-rustc"
        self.rustc.write_text(FAKE_RUSTC)
        self.rustc.chmod(0o755)
        self.set_rustc_version(third_party_notices.RUST_VERSION)

    def set_rustc_version(self, version):
        (self.root / "rustc-version").write_text(version)

    def add(self, name, files=None, deps=(), license="MIT OR Apache-2.0", authors=(), **fields):
        directory = self.crates / name
        directory.mkdir(parents=True)
        (directory / "Cargo.toml").write_text(f'[package]\nname = "{name}"\n')
        for rel, text in (files or {}).items():
            (directory / rel).parent.mkdir(parents=True, exist_ok=True)
            (directory / rel).write_text(text)
        package_id = f"{name} 1.0.0"
        self.packages.append(
            {
                "id": package_id,
                "name": name,
                "version": "1.0.0",
                "license": license,
                "license_file": None,
                "authors": list(authors),
                "manifest_path": str(directory / "Cargo.toml"),
                **fields,
            }
        )
        self.nodes.append(
            {
                "id": package_id,
                "deps": [
                    {"pkg": f"{dep} 1.0.0", "dep_kinds": [{"kind": kind, "target": None}]}
                    for dep, kind in deps
                ],
            }
        )

    def write_graph(self):
        (self.root / "graph.json").write_text(
            json.dumps({"packages": self.packages, "resolve": {"nodes": self.nodes}})
        )

    def generate(self, platform_key, core_only=False):
        return third_party_notices.generate(
            platform_key, core_only, cargo=str(self.cargo), rustc=str(self.rustc)
        )

    def calls(self):
        return (self.root / "calls.log").read_text().splitlines()

    def test_walk_keeps_normal_dependencies_only(self):
        text = self.generate("linux-x64")
        self.assertIn("| serde | 1.0.0 |", text)
        self.assertIn("| serde_derive | 1.0.0 |", text)
        self.assertNotIn("| tempfile |", text)
        self.assertNotIn("| cc |", text)
        self.assertNotIn("| pumice-relay |", text)
        self.assertNotIn("dev only", text)
        self.assertNotIn("build only", text)

    def test_shipped_in_names_each_binary(self):
        text = self.generate("linux-x64")
        self.assertIn("| serde | 1.0.0 | MIT OR Apache-2.0 | `pumice`, `pumice-transcribe` |", text)
        self.assertIn("| opusic-sys | 1.0.0 | MIT OR Apache-2.0 | `pumice-transcribe` |", text)

    def test_metadata_runs_per_target_and_feature(self):
        self.generate("linux-x64")
        self.assertEqual(
            self.calls(),
            ["x86_64-unknown-linux-musl ", "x86_64-unknown-linux-gnu pumice-transcription/vulkan"],
        )

    def test_identical_texts_print_once(self):
        text = self.generate("win32-x64")
        self.assertEqual(text.count("Copyright (c) Alice"), 1)
        self.assertIn("Used by: serde 1.0.0, serde_derive 1.0.0.", text)

    def test_crate_without_license_file_gets_standard_text(self):
        text = self.generate("linux-x64")
        self.assertIn("Copyright (c) Austin Blake", text)
        self.assertIn("Crates without a published license file", text)
        self.assertIn("ebml 1.0.0", text)

    def test_unknown_license_without_file_fails(self):
        self.packages[[p["name"] for p in self.packages].index("ebml")]["license"] = "WTFPL"
        self.write_graph()
        with self.assertRaises(SystemExit) as caught:
            self.generate("linux-x64")
        self.assertIn("ebml 1.0.0 ships no license file", str(caught.exception))

    def test_native_components_follow_the_bundle(self):
        linux = self.generate("linux-x64")
        for name in ("musl libc", "transcribe.cpp", "ggml", "llamafile sgemm", "miniz", "libopus", "Vulkan-Headers", "SPIRV-Headers"):
            self.assertIn(name, linux)
        self.assertIn("Copyright 2024 Mozilla Foundation\n\nPermission granted.", linux)
        self.assertIn("Opus BSD", linux)

        mac = self.generate("darwin-arm64")
        self.assertIn("transcribe.cpp", mac)
        self.assertNotIn("Vulkan-Headers", mac)
        self.assertNotIn("**musl libc", mac)

        windows = self.generate("win32-x64")
        self.assertIn("Vulkan-Headers", windows)
        self.assertNotIn("**musl libc", windows)

    def test_intel_mac_is_core_only(self):
        text = self.generate("darwin-x64")
        self.assertIn("`pumice` (x86_64-apple-darwin)", text)
        self.assertNotIn("pumice-transcribe", text)
        self.assertNotIn("transcribe.cpp", text)
        self.assertIn("Rust standard library", text)
        self.assertEqual(self.calls(), ["x86_64-apple-darwin "])

    def test_core_only_skips_the_bundle(self):
        text = self.generate("linux-x64", core_only=True)
        self.assertNotIn("libopus", text)
        self.assertIn("musl libc", text)
        self.assertEqual(self.calls(), ["x86_64-unknown-linux-musl "])

    def test_formatter_notices_are_included(self):
        self.assertIn("Copyright (c) 2025 CJ Pais", self.generate("darwin-x64"))

    def test_fences_outlast_backticks_in_texts(self):
        text = self.generate("linux-x64")
        self.assertIn("````text\nggml MIT ```fenced```\n````", text)

    def test_output_is_a_generated_notices_file(self):
        out = self.root / "out"
        third_party_notices.main(
            ["--platform", "darwin-x64", "--output-dir", str(out), "--cargo", str(self.cargo), "--rustc", str(self.rustc)]
        )
        self.assertTrue((out / "darwin-x64.md").read_text().startswith("# Third-party notices"))

    def test_every_package_carries_the_rust_std_notices(self):
        version = third_party_notices.RUST_VERSION
        for key, core_only in (("darwin-x64", False), ("linux-x64", True), ("win32-x64", True)):
            text = self.generate(key, core_only)
            self.assertIn(f"**Rust standard library {version}", text)
            self.assertIn("Copyright © 1991-2024 Unicode, Inc.", text)
            self.assertIn("Copyright (c) 2019 The Crossbeam Project Developers", text)
            self.assertIn(f"**compiler-builtins 0.1.160 and its libm (Rust {version}", text)
            self.assertIn("Copyright (c) 2018 Jorge Aparicio", text)
            self.assertIn("Copyright (c) 2016 Amanieu d'Antras", text)  # hashbrown

    def test_rust_std_crates_follow_the_target(self):
        # std links gimli, libc and friends everywhere but windows-msvc.
        linux = self.generate("linux-x64", core_only=True)
        self.assertIn("**gimli 0.34.0 (a dependency of the Rust", linux)
        self.assertIn("**libc 0.2.189 (a dependency of the Rust", linux)
        windows = self.generate("win32-x64")
        self.assertNotIn("**gimli 0.34.0", windows)
        self.assertNotIn("**libc 0.2.189", windows)
        self.assertIn("**hashbrown 0.17.1 (a dependency of the Rust", windows)

    def test_pinned_rust_std_texts_exist(self):
        for entry in third_party_notices.rust_std_entries():
            for source in entry["texts"]:
                if source["kind"] == "repo":
                    self.assertTrue((third_party_notices.TEXTS_DIR / source["path"]).is_file(), source)

    def test_other_rustc_version_fails(self):
        self.set_rustc_version("1.100.0")
        with self.assertRaises(SystemExit) as caught:
            self.generate("linux-x64")
        self.assertIn(
            f"pinned for rustc {third_party_notices.RUST_VERSION} "
            f"(scripts/notices/rust-{third_party_notices.RUST_VERSION}/) but the "
            "toolchain is rustc 1.100.0",
            str(caught.exception),
        )
        self.assertIn("scripts/notices/rust-1.100.0/", str(caught.exception))
        self.assertIn('set RUST_VERSION = "1.100.0" in scripts/third_party_notices.py', str(caught.exception))

    def test_print_rust_version_needs_no_toolchain(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.assertEqual(third_party_notices.main(["--print-rust-version"]), 0)
        self.assertEqual(out.getvalue(), f"{third_party_notices.RUST_VERSION}\n")

    def test_release_builds_install_the_pinned_toolchain(self):
        # D4: every Rust install in the release preview, direct or through a
        # bundle action, takes the version from --print-rust-version.
        release = (ROOT / ".github/workflows/release.yml").read_text()
        pinned = "${{ needs.package-tests.outputs.rust-version }}"
        self.assertIn("third_party_notices.py --print-rust-version", release)
        installs = re.findall(r"uses: dtolnay/rust-toolchain@(\S+)\n\s+with:\n\s+toolchain: (.+)", release)
        self.assertEqual(len(installs), release.count("dtolnay/rust-toolchain@"))
        self.assertTrue(installs)
        for ref, toolchain in installs:
            self.assertEqual((ref, toolchain.strip()), ("master", pinned))
        actions = re.findall(r"uses: \./\.github/actions/\S+\n\s+with:\n\s+rust-toolchain: (.+)", release)
        self.assertEqual(len(actions), release.count("uses: ./.github/actions/"))
        self.assertEqual({a.strip() for a in actions}, {pinned})

    def add_mpl_crate(self, name, license="MPL-2.0", vcs=True, **fields):
        files = {"LICENSE": "Mozilla Public License Version 2.0\n"}
        if vcs:
            files[".cargo_vcs_info.json"] = json.dumps(
                {"git": {"sha1": "ee35874b571a35a9a6e15d3bc9a3aaf8f11fbeee"}, "path_in_vcs": name}
            )
        self.add(name, files=files, license=license, **fields)
        transcription = self.nodes[[p["name"] for p in self.packages].index("pumice-transcription")]
        transcription["deps"].append({"pkg": f"{name} 1.0.0", "dep_kinds": [{"kind": None, "target": None}]})
        self.write_graph()

    def test_mpl_crates_link_to_their_exact_source(self):
        self.add_mpl_crate(
            "symphonia-core",
            source=third_party_notices.CRATES_IO,
            repository="https://github.com/pdeljanov/Symphonia",
        )
        self.add_mpl_crate(
            "dual-mpl",
            license="MIT OR MPL-2.0",
            vcs=False,
            source=third_party_notices.CRATES_IO,
        )
        text = self.generate("linux-x64")
        self.assertIn("## Source code of MPL-licensed crates", text)
        self.assertIn("Pumice uses them unmodified, exactly as published", text)
        self.assertIn(
            "- symphonia-core 1.0.0: <https://crates.io/api/v1/crates/symphonia-core/1.0.0/download>, "
            "<https://github.com/pdeljanov/Symphonia/tree/ee35874b571a35a9a6e15d3bc9a3aaf8f11fbeee/symphonia-core>",
            text,
        )
        self.assertIn("- dual-mpl 1.0.0: <https://crates.io/api/v1/crates/dual-mpl/1.0.0/download>\n", text)
        self.assertNotIn("- serde 1.0.0:", text)

    def test_no_mpl_section_without_mpl_crates(self):
        self.assertNotIn("Source code of MPL-licensed crates", self.generate("linux-x64"))

    def test_mpl_crate_without_a_source_fails(self):
        self.add_mpl_crate("vendored-mpl", vcs=False, source=None)
        with self.assertRaises(SystemExit) as caught:
            self.generate("linux-x64")
        self.assertIn("vendored-mpl 1.0.0 is MPL-licensed but has no crates.io source", str(caught.exception))


if __name__ == "__main__":
    unittest.main()
