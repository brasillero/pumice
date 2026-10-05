#!/usr/bin/env python3
"""Prepare offline release notes without tags, version changes or publication."""

import argparse
import datetime as dt
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import tomllib

CLI_VERSION = "2.14.2"
ROOT = Path(__file__).resolve().parents[2]
SEMVER = re.compile(
    r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-((?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
    r"(?:\.(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*))?"
    r"(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
)


def version_key(version):
    match = SEMVER.fullmatch(version)
    if not match:
        raise ValueError("release version must be SemVer without a leading v")
    major, minor, patch, prerelease, _ = match.groups()
    identifiers = tuple(
        (0, int(item)) if item.isdigit() else (1, item)
        for item in prerelease.split(".")
    ) if prerelease else ()
    return int(major), int(minor), int(patch), not prerelease, identifiers


def date_value(value):
    if not re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2}", value):
        raise ValueError("release date must be YYYY-MM-DD")
    return dt.date.fromisoformat(value)


def run(command, repo):
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_CLIFF")}
    return subprocess.run(
        command, cwd=repo, env=env, check=True, capture_output=True,
        text=True, encoding="utf-8", timeout=30,
    ).stdout


def history(existing, version):
    headings = list(re.finditer(r"^## .*?$", existing, re.MULTILINE))
    if not headings or headings[0].group() != "## [Unreleased]":
        raise ValueError("CHANGELOG.md must start with an Unreleased section")
    versions = set()
    for heading in headings[1:]:
        match = re.fullmatch(r"## \[(.+)\] - ([0-9]{4}-[0-9]{2}-[0-9]{2})", heading.group())
        if not match:
            raise ValueError("malformed released changelog section")
        prior, prior_date = match.groups()
        if prior in versions or version_key(prior) >= version_key(version):
            raise ValueError("release version must follow every existing released version")
        date_value(prior_date)
        versions.add(prior)
    header = existing[:headings[0].start()]
    tail = existing[headings[1].start():] if len(headings) > 1 else ""
    return header, tail


def prepare(repo, version, release_date, output, cli):
    repo, output = Path(repo).resolve(), Path(output).resolve()
    version_key(version)
    date = date_value(release_date)
    if run(["git", "rev-parse", "--is-shallow-repository"], repo).strip() != "false":
        raise ValueError("release preparation requires full Git history (fetch-depth: 0)")
    try:
        cargo = tomllib.loads((repo / "Cargo.toml").read_text(encoding="utf-8"))
        cargo_version = cargo["package"]["version"]
    except (KeyError, TypeError, tomllib.TOMLDecodeError) as error:
        raise ValueError(f"Cargo.toml is missing a valid package.version: {error}")
    if cargo_version != version:
        raise ValueError("release version must exactly match Cargo.toml")
    tags = run(["git", "tag", "--list"], repo).splitlines()
    if version in tags or "v" + version in tags:
        raise ValueError("release tag already exists; prepare notes before tagging")
    header, tail = history((repo / "CHANGELOG.md").read_bytes().decode("utf-8"), version)
    if output == repo or output.is_relative_to(repo / ".git"):
        raise ValueError("output must be a separate artifact directory")
    for name in ("CHANGELOG.md", "release-notes.md"):
        destination = output / name
        if destination.is_symlink():
            raise ValueError("release artifacts must not overwrite symlinks")
        if destination.resolve().is_relative_to(repo / ".git"):
            raise ValueError("release artifacts must not write into .git")
        if destination.is_relative_to(repo):
            relative = destination.relative_to(repo).as_posix()
            tracked = run(["git", "ls-files", "--", relative], repo).strip()
            if tracked:
                raise ValueError("release artifacts must not overwrite tracked files")
    if run([cli, "--version"], repo).strip() != "git-cliff " + CLI_VERSION:
        raise ValueError("use the pinned git-cliff " + CLI_VERSION)
    common = [cli, "--offline", "--no-exec", "--config", str(repo / "cliff.toml")]
    context = json.loads(run(common + ["--unreleased", "--tag", "v" + version, "--context"], repo))
    if len(context) != 1 or context[0]["version"] != "v" + version:
        raise ValueError("expected exactly one proposed release")
    context[0]["timestamp"] = int(dt.datetime.combine(date, dt.time(), dt.timezone.utc).timestamp())
    with tempfile.TemporaryDirectory() as directory:
        context_path = Path(directory) / "context.json"
        context_path.write_text(json.dumps(context), encoding="utf-8")
        notes = run(common + ["--from-context", str(context_path), "--strip", "all"], repo)
    expected = f"## [{version}] - {release_date}"
    if not notes.startswith(expected + "\n"):
        raise ValueError("rendered release heading does not match version/date")
    notes = notes.rstrip() + "\n\n"
    changelog = header + "## [Unreleased]\n\n" + notes + tail
    if not tail:
        changelog = changelog.rstrip("\n") + "\n"
    output.mkdir(parents=True, exist_ok=True)
    (output / "release-notes.md").write_bytes(notes.encode("utf-8"))
    (output / "CHANGELOG.md").write_bytes(changelog.encode("utf-8"))
    return notes, changelog


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--date", required=True)
    parser.add_argument("--output", type=Path, default=ROOT / "dist" / "release")
    parser.add_argument("--git-cliff", default=os.environ.get("GIT_CLIFF_BINARY", "git-cliff"))
    args = parser.parse_args()
    try:
        prepare(ROOT, args.version, args.date, args.output, args.git_cliff)
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        parser.exit(1, f"release preparation failed: {error}\n")
    print(f"Prepared release artifacts for {args.version} in {args.output}; no tag or release created")


if __name__ == "__main__":
    main()
