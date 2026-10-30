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
    value = cache_identity(backend, 'release bundle', dict(os.environ), Path.cwd())
    value += run(['git', 'rev-parse', 'HEAD^{tree}'])
    value += os.environ.get('ImageVersion', '')
    return hashlib.sha256(value.encode()).hexdigest()


def api(path):
    return json.loads(subprocess.check_output(['gh', 'api', path]))


def coordinate():
    event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())
    if os.environ['GITHUB_EVENT_NAME'] != 'pull_request':
        return ''
    repo = os.environ['GITHUB_REPOSITORY']
    sha = event['pull_request']['head']['sha']
    # The release-only paths need no transcription run. Give GitHub a bounded
    # discovery window; an absent/failed/cancelled producer means a local build.
    deadline = time.monotonic() + 60
    candidate = None
    while time.monotonic() < deadline:
        runs = api(f'repos/{repo}/actions/workflows/transcription.yml/runs?event=pull_request&head_sha={sha}&per_page=20')['workflow_runs']
        candidate = next((r for r in runs if any(p['number'] == event['number'] for p in r['pull_requests'])), None)
        if candidate:
            break
        time.sleep(10)
    if not candidate:
        return ''
    rid = candidate['id']
    deadline = time.monotonic() + 1200
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
