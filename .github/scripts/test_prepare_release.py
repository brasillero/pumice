#!/usr/bin/env python3
"""Fixture-based tests for prepare-release.py using temporary local Git repos.

The pinned, offline git-cliff binary is located through the GIT_CLIFF_BINARY
environment variable. No test touches the real Pumice repository, creates tags
there or calls any AI service.
"""

import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPT = REPO_ROOT / ".github" / "scripts" / "prepare-release.py"
CLIFF_TOML = REPO_ROOT / "cliff.toml"
GIT_CLIFF = os.environ.get("GIT_CLIFF_BINARY", "")

spec = importlib.util.spec_from_file_location("prepare_release", SCRIPT)
prepare_release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare_release)

IDENTITY = {
    "GIT_AUTHOR_NAME": "Pumice Test",
    "GIT_AUTHOR_EMAIL": "pumice-test@example.invalid",
    "GIT_COMMITTER_NAME": "Pumice Test",
    "GIT_COMMITTER_EMAIL": "pumice-test@example.invalid",
}

HEADER = (
    "# Changelog\n"
    "\n"
    "Changes are grouped using [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).\n"
    "Release versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).\n"
    "\n"
)

CARGO_0_1_0 = '[package]\nname = "pumice"\nversion = "0.1.0"\n'


def cargo_with(version):
    return f'[package]\nname = "pumice"\nversion = "{version}"\n'

BASE_CHANGELOG = HEADER + "## [Unreleased]\n\n### Added\n\n- stale preview entry\n\n"

RELEASED_0_1_0_TAIL = (
    "## [0.1.0] - 2021-06-01\n"
    "\n"
    "### Added\n"
    "\n"
    "- first release (#1)\n"
    "\n"
)


def git(repo, *args, dates=None):
    env = dict(os.environ)
    env.update(IDENTITY)
    if dates:
        env.update(dates)
    return subprocess.run(
        ["git", *args], cwd=repo, env=env, check=True,
        capture_output=True, text=True,
    ).stdout


class Fixture:
    """A throwaway Git repository with Pumice release metadata."""

    _created = 0

    def __init__(self, root, *, cargo=CARGO_0_1_0, changelog=BASE_CHANGELOG):
        Fixture._created += 1
        self.path = root / f"repo-{Fixture._created}"
        self.path.mkdir()
        git(self.path, "init", "-b", "main")
        git(self.path, "config", "user.name", IDENTITY["GIT_AUTHOR_NAME"])
        git(self.path, "config", "user.email", IDENTITY["GIT_AUTHOR_EMAIL"])
        (self.path / "Cargo.toml").write_text(cargo, encoding="utf-8")
        shutil.copyfile(CLIFF_TOML, self.path / "cliff.toml")
        (self.path / "CHANGELOG.md").write_text(changelog, encoding="utf-8")
        self._counter = 0

    def commit(self, subject, body=None, tag=None):
        self._counter += 1
        with (self.path / "work.txt").open("a", encoding="utf-8") as file:
            file.write(f"{subject}\n")
        git(self.path, "add", ".")
        command = ["commit", "-m", subject] + (["-m", body] if body else [])
        stamp = f"2021-01-{min(self._counter, 28):02d}T12:00:00+00:00"
        git(self.path, *command, dates={
            "GIT_AUTHOR_DATE": stamp, "GIT_COMMITTER_DATE": stamp,
        })
        if tag:
            git(self.path, "tag", tag)

    def prepare(self, version, date, output):
        return prepare_release.prepare(
            self.path, version, date, output, GIT_CLIFF,
        )

    def tracked_files(self):
        return git(self.path, "ls-files")

    def tags(self):
        return git(self.path, "tag", "--list")

    def status(self):
        return git(self.path, "status", "--porcelain")


@unittest.skipUnless(GIT_CLIFF, "set GIT_CLIFF_BINARY to the pinned git-cliff path")
class PrepareReleaseTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = Path(self._tmp.name)

    def output(self, name="out"):
        return self.root / name

    def make_fixture(self, **kwargs):
        return Fixture(self.root, **kwargs)

    def assert_rejected(self, fixture, version, date, *, output=None):
        output = output or self.output()
        before = (fixture.tracked_files(), fixture.tags(), fixture.status())
        with self.assertRaises((ValueError, OSError, subprocess.SubprocessError)):
            fixture.prepare(version, date, output)
        self.assertFalse(output.exists(), "failed run must not create output")
        self.assertEqual(
            before,
            (fixture.tracked_files(), fixture.tags(), fixture.status()),
            "failed run must not touch the repository",
        )

    # -- grouping and rendering -------------------------------------------

    def test_conventional_grouping_and_skipped_types(self):
        fixture = self.make_fixture()
        fixture.commit("feat: add alpha")
        fixture.commit("fix: correct beta")
        fixture.commit("refactor: tidy gamma")
        fixture.commit("perf: speed up delta")
        fixture.commit("revert: undo epsilon")
        fixture.commit("docs: rewrite the guide")
        fixture.commit("chore: bump dependencies")
        fixture.commit("test: cover zeta")
        fixture.commit("ci: add workflow")
        fixture.commit("style: format eta")
        fixture.commit("random nonconventional message")

        notes, changelog = fixture.prepare("0.1.0", "2021-02-01", self.output())

        self.assertTrue(notes.startswith("## [0.1.0] - 2021-02-01\n"), notes)
        self.assertIn("### Added\n- add alpha\n", notes)
        self.assertIn("### Fixed\n- correct beta\n", notes)
        self.assertIn("### Changed\n", notes)
        for entry in ("tidy gamma", "speed up delta", "undo epsilon"):
            self.assertIn(f"- {entry}\n", notes)
        for skipped in (
            "rewrite the guide", "bump dependencies", "cover zeta",
            "add workflow", "format eta", "nonconventional",
        ):
            self.assertNotIn(skipped, notes)
        for empty_group in ("### Deprecated", "### Removed", "### Security"):
            self.assertNotIn(empty_group, notes)
        self.assertIn("## [Unreleased]\n\n## [0.1.0]", changelog)

    def test_breaking_commits_are_preserved(self):
        fixture = self.make_fixture()
        fixture.commit("feat!: rework the api")
        fixture.commit("fix: patch theta", body="BREAKING CHANGE: drops old config")

        notes, _ = fixture.prepare("0.1.0", "2021-02-01", self.output())

        self.assertIn("**BREAKING:** rework the api\n", notes)
        self.assertIn("**BREAKING:** patch theta: drops old config\n", notes)

    def test_prerelease_and_build_metadata_versions(self):
        fixture = self.make_fixture(cargo='[package]\nname = "pumice"\nversion = "0.1.0-rc.1"\n')
        fixture.commit("feat: add alpha")
        notes, _ = fixture.prepare("0.1.0-rc.1", "2021-02-01", self.output())
        self.assertTrue(notes.startswith("## [0.1.0-rc.1] - 2021-02-01\n"), notes)

    # -- prior releases ----------------------------------------------------

    def test_prior_tag_excludes_old_commits(self):
        fixture = self.make_fixture(cargo=cargo_with("0.2.0"))
        fixture.commit("feat: first feature", tag="v0.1.0")
        fixture.commit("docs: refresh readme")
        fixture.commit("feat: second feature")

        notes, _ = fixture.prepare("0.2.0", "2021-07-04", self.output())

        self.assertNotIn("first feature", notes)
        self.assertNotIn("refresh readme", notes)
        self.assertIn("second feature", notes)
        self.assertTrue(notes.startswith("## [0.2.0] - 2021-07-04\n"), notes)

    def test_released_tail_preserved_and_notes_inserted_verbatim(self):
        changelog = HEADER + "## [Unreleased]\n\n- stale preview entry\n\n" + RELEASED_0_1_0_TAIL
        fixture = self.make_fixture(cargo=cargo_with("0.2.0"), changelog=changelog)
        fixture.commit("feat: first feature", tag="v0.1.0")
        fixture.commit("feat: second feature")

        notes, rendered = fixture.prepare("0.2.0", "2021-07-04", self.output())

        expected = HEADER + "## [Unreleased]\n\n" + notes + RELEASED_0_1_0_TAIL
        self.assertEqual(rendered, expected)
        self.assertTrue(rendered.endswith(RELEASED_0_1_0_TAIL))
        self.assertNotIn("stale preview entry", rendered)
        notes_file = (self.output() / "release-notes.md").read_text(encoding="utf-8")
        self.assertEqual(notes_file, notes)

    def test_first_release_has_no_tail(self):
        fixture = self.make_fixture()
        fixture.commit("feat: add alpha")
        notes, changelog = fixture.prepare("0.1.0", "2021-02-01", self.output())
        self.assertEqual(
            changelog,
            HEADER + "## [Unreleased]\n\n" + notes.rstrip("\n") + "\n",
        )
        self.assertTrue(changelog.endswith("\n"))
        self.assertFalse(changelog.endswith("\n\n"))

    # -- determinism and repo immutability ---------------------------------

    def test_same_inputs_give_identical_bytes(self):
        fixture = self.make_fixture()
        fixture.commit("feat: add alpha")
        fixture.commit("fix: correct beta")
        first_notes, first_changelog = fixture.prepare("0.1.0", "2021-02-01", self.output("one"))
        second_notes, second_changelog = fixture.prepare("0.1.0", "2021-02-01", self.output("two"))
        self.assertEqual(first_notes, second_notes)
        self.assertEqual(first_changelog, second_changelog)

    def test_success_leaves_repository_untouched(self):
        fixture = self.make_fixture()
        fixture.commit("feat: add alpha")
        before = (fixture.tracked_files(), fixture.tags())
        cargo_before = (fixture.path / "Cargo.toml").read_bytes()
        changelog_before = (fixture.path / "CHANGELOG.md").read_bytes()
        fixture.prepare("0.1.0", "2021-02-01", self.output())
        self.assertEqual(before, (fixture.tracked_files(), fixture.tags()))
        self.assertEqual(cargo_before, (fixture.path / "Cargo.toml").read_bytes())
        self.assertEqual(changelog_before, (fixture.path / "CHANGELOG.md").read_bytes())
        self.assertEqual("", fixture.status())
        self.assertTrue((self.output() / "CHANGELOG.md").is_file())
        self.assertTrue((self.output() / "release-notes.md").is_file())

    # -- rejection paths (all before any output) ---------------------------

    def test_invalid_semver_rejected(self):
        fixture = self.make_fixture()
        fixture.commit("feat: add alpha")
        for bad in ("v0.1.0", "0.1", "0.01.0", "0.1.0 ", "0.1.0-beta..1"):
            with self.subTest(version=bad):
                self.assert_rejected(fixture, bad, "2021-02-01")

    def test_invalid_date_rejected(self):
        fixture = self.make_fixture()
        fixture.commit("feat: add alpha")
        for bad in ("2021-13-01", "2021-00-10", "2021/02/01", "21-02-01", "2021-02-30"):
            with self.subTest(date=bad):
                self.assert_rejected(fixture, "0.1.0", bad)

    def test_cargo_mismatch_rejected(self):
        fixture = self.make_fixture()
        fixture.commit("feat: add alpha")
        self.assert_rejected(fixture, "9.9.9", "2021-02-01")

    def test_release_matching_released_heading_or_below_history_rejected(self):
        cases = [
            # Version already present as a released section (and its tag).
            (cargo_with("0.1.0"), HEADER + "## [Unreleased]\n\n" + RELEASED_0_1_0_TAIL, "0.1.0"),
            # Version lower than an existing released section.
            (
                cargo_with("0.2.0"),
                HEADER + "## [Unreleased]\n\n"
                        "## [0.3.0] - 2021-06-01\n\n### Added\n\n- newer (#2)\n\n"
                        + RELEASED_0_1_0_TAIL,
                "0.2.0",
            ),
        ]
        for cargo, changelog, version in cases:
            with self.subTest(version=version):
                fixture = self.make_fixture(cargo=cargo, changelog=changelog)
                fixture.commit("feat: first feature")
                self.assert_rejected(fixture, version, "2021-07-04")

    def test_prerelease_after_final_release_rejected(self):
        tail = (
            "## [0.2.0] - 2021-06-01\n\n### Added\n\n- final (#2)\n\n"
            "## [0.1.0] - 2021-05-01\n\n### Added\n\n- initial (#1)\n\n"
        )
        fixture = self.make_fixture(cargo=cargo_with("0.2.0-beta.1"), changelog=HEADER + "## [Unreleased]\n\n" + tail)
        fixture.commit("feat: first feature")
        self.assert_rejected(fixture, "0.2.0-beta.1", "2021-07-04")

    def test_final_release_after_prerelease_allowed(self):
        tail = "## [0.2.0-beta.1] - 2021-06-01\n\n### Added\n\n- beta (#2)\n\n"
        fixture = self.make_fixture(cargo=cargo_with("0.2.0"), changelog=HEADER + "## [Unreleased]\n\n" + tail)
        fixture.commit("feat: first feature")
        notes, _ = fixture.prepare("0.2.0", "2021-07-04", self.output())
        self.assertTrue(notes.startswith("## [0.2.0] - 2021-07-04\n"), notes)

    def test_malformed_or_duplicate_headings_rejected(self):
        cases = [
            HEADER + "## [Unreleased]\n\n## 0.1.0 - 2021-06-01\n\n",
            HEADER + "## [Unreleased]\n\n## [0.1.0]\n\n",
            HEADER + "## [Unreleased]\n\n## [0.1.0] - 2021-06-01\n\n"
                    "## [0.1.0] - 2021-07-01\n\n",
            HEADER + "## [Unreleased]\n\n## [0.1.0] - 06/01/2021\n\n",
            HEADER + "## [0.1.0] - 2021-06-01\n\n",
        ]
        for changelog in cases:
            with self.subTest(changelog=changelog.splitlines()[-1]):
                fixture = self.make_fixture(changelog=changelog)
                fixture.commit("feat: first feature")
                self.assert_rejected(fixture, "0.1.0", "2021-02-01")

    def test_existing_tag_rejected(self):
        fixture = self.make_fixture()
        fixture.commit("feat: first feature", tag="v0.1.0")
        fixture.commit("feat: second feature")
        self.assert_rejected(fixture, "0.1.0", "2021-07-04")

    def test_shallow_repository_rejected(self):
        fixture = self.make_fixture()
        fixture.commit("feat: first feature")
        shallow = self.root / "shallow"
        git(self.root, "clone", "--depth", "1", f"file://{fixture.path}", str(shallow))
        for name in ("Cargo.toml", "cliff.toml", "CHANGELOG.md"):
            shutil.copyfile(fixture.path / name, shallow / name)
        before = git(shallow, "rev-parse", "--is-shallow-repository").strip()
        self.assertEqual(before, "true")
        with self.assertRaises(ValueError):
            prepare_release.prepare(shallow, "0.1.0", "2021-02-01", self.output(), GIT_CLIFF)
        self.assertFalse(self.output().exists())

    def test_wrong_git_cliff_version_rejected(self):
        fixture = self.make_fixture()
        fixture.commit("feat: add alpha")
        fake = self.root / "fake-git-cliff"
        fake.write_text("#!/bin/sh\necho 'git-cliff 9.9.9'\n", encoding="utf-8")
        fake.chmod(0o755)
        with self.assertRaises(ValueError):
            prepare_release.prepare(
                fixture.path, "0.1.0", "2021-02-01", self.output(), str(fake),
            )
        self.assertFalse(self.output().exists())

    def test_output_inside_git_directory_rejected(self):
        fixture = self.make_fixture()
        fixture.commit("feat: add alpha")
        self.assert_rejected(
            fixture, "0.1.0", "2021-02-01", output=fixture.path / ".git" / "out",
        )

    def test_symlinked_destinations_rejected(self):
        cases = [
            ("release-notes.md", "tracked Cargo.toml", lambda fx: fx.path / "Cargo.toml"),
            ("CHANGELOG.md", ".git target", lambda fx: fx.path / ".git" / "HEAD"),
            ("CHANGELOG.md", "dangling", lambda fx: fx.path / "missing-target"),
        ]
        for name, label, target in cases:
            with self.subTest(name=name, target=label):
                fixture = self.make_fixture()
                fixture.commit("feat: add alpha")
                output = self.output(f"{name}-{label}")
                output.mkdir()
                (output / name).symlink_to(target(fixture))
                tracked_before = {
                    path: (fixture.path / path).read_bytes()
                    for path in fixture.tracked_files().splitlines()
                }
                with self.assertRaises(ValueError):
                    fixture.prepare("0.1.0", "2021-02-01", output)
                for path, content in tracked_before.items():
                    self.assertEqual(content, (fixture.path / path).read_bytes())
                self.assertEqual(
                    sorted(path.name for path in output.iterdir()), [name],
                    "failed run must not write any artifact",
                )
                self.assertEqual("", fixture.status())

    # -- CLI harness --------------------------------------------------------

    def test_cli_success_and_failure(self):
        fixture = self.make_fixture(cargo=cargo_with("0.2.0"))
        fixture.commit("feat: first feature", tag="v0.1.0")
        fixture.commit("feat: second feature")
        # The CLI resolves its repo from __file__, so run a copy inside the
        # fixture; 0.2.0 differs from the real repository's Cargo version and
        # fails there if the copy ever stops being used.
        scripts = fixture.path / ".github" / "scripts"
        scripts.mkdir(parents=True)
        script = scripts / "prepare-release.py"
        shutil.copyfile(SCRIPT, script)
        env = dict(os.environ, GIT_CLIFF_BINARY=GIT_CLIFF)
        ok = subprocess.run(
            [sys.executable, str(script), "--version", "0.2.0", "--date", "2021-07-04",
             "--output", str(self.output())],
            cwd=fixture.path, env=env, capture_output=True, text=True,
        )
        self.assertEqual(ok.returncode, 0, ok.stderr)
        self.assertIn("no tag or release created", ok.stdout)
        self.assertTrue((self.output() / "release-notes.md").is_file())
        self.assertTrue((self.output() / "CHANGELOG.md").is_file())
        bad = subprocess.run(
            [sys.executable, str(script), "--version", "0.2.0", "--date", "not-a-date",
             "--output", str(self.output("bad-out"))],
            cwd=fixture.path, env=env, capture_output=True, text=True,
        )
        self.assertEqual(bad.returncode, 1)
        self.assertIn("release preparation failed", bad.stderr)
        self.assertFalse(self.output("bad-out").exists())


if __name__ == "__main__":
    unittest.main()
