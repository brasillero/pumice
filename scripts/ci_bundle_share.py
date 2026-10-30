#!/usr/bin/env python3
"""Coordinate same-PR bundle reuse; never reuse a different source or toolchain."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time

from ci_cargo_cache import cache_identity, run


def identity(backend):
    value = cache_identity(
        backend, 'release bundle', dict(os.environ), Path.cwd(), provenance=True,
    )
    value += run(['git', 'rev-parse', 'HEAD^{tree}'])
    return hashlib.sha256(value.encode()).hexdigest()


class ProducerUnavailable(RuntimeError):
    """The optional cross-workflow producer cannot be queried."""


def api(path):
    for attempt in range(3):
        try:
            return json.loads(subprocess.check_output(
                ['gh', 'api', path], text=True, stderr=subprocess.PIPE, timeout=30,
            ))
        except (subprocess.SubprocessError, OSError, json.JSONDecodeError) as error:
            if attempt == 2:
                raise ProducerUnavailable(
                    f'GitHub API unavailable after 3 attempts: {path} '
                    f'({type(error).__name__})'
                ) from error
            time.sleep(attempt + 1)


def coordinate():
    # Sharing is only an optimisation: any failure here means local builds.
    try:
        return locate_producer()
    except Exception as error:
        print(f'::warning::Bundle producer lookup failed ({type(error).__name__}: '
              f'{error}); using local bundle builds.', flush=True)
        return ''


def same_head(run, sha, head_repo):
    # GitHub lists no pull_requests for runs from forks; the head commit and
    # repository identify the PR's runs either way. The bundle identity check
    # still guards what is reused.
    return (run.get('head_sha') == sha
            and (run.get('head_repository') or {}).get('full_name') == head_repo)


def locate_producer():
    started = time.monotonic()
    event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())
    if os.environ['GITHUB_EVENT_NAME'] != 'pull_request':
        return ''
    repo = os.environ['GITHUB_REPOSITORY']
    head = event['pull_request']['head']
    sha, head_repo = head['sha'], head['repo']['full_name']
    # The release-only paths need no transcription run. Give GitHub a bounded
    # discovery window; an absent/failed/cancelled producer means a local build.
    deadline = started + 60
    candidate = None
    while time.monotonic() < deadline:
        runs = api(f'repos/{repo}/actions/workflows/transcription.yml/runs?event=pull_request&head_sha={sha}&per_page=20')['workflow_runs']
        candidate = next((r for r in runs if same_head(r, sha, head_repo)), None)
        if candidate:
            break
        time.sleep(10)
    if not candidate:
        return ''
    rid = candidate['id']
    # 20 minutes in all, inside the workflow step's 22-minute timeout.
    deadline = started + 1200
    wanted = {'shared-bundle-' + b for b in ('linux-vulkan', 'windows-vulkan', 'macos-metal')}
    while time.monotonic() < deadline:
        artifacts = api(f'repos/{repo}/actions/runs/{rid}/artifacts?per_page=100')['artifacts']
        if wanted <= {a['name'] for a in artifacts if not a['expired']}:
            return str(rid)
        state = api(f'repos/{repo}/actions/runs/{rid}')
        if state['status'] == 'completed':
            return ''
        time.sleep(15)
    return ''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['identity', 'coordinate'])
    parser.add_argument('--backend')
    args = parser.parse_args()
    value = identity(args.backend) if args.command == 'identity' else coordinate()
    print(value)
    with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
        output.write(f'value={value}\n')


if __name__ == '__main__':
    main()
