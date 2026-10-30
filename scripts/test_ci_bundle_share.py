"""Offline checks for the exact-identity bundle reuse gate."""
import io
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import ci_bundle_share as share
import ci_cargo_cache as cargo_cache


class IdentityTests(unittest.TestCase):
    def test_native_install_from_old_image_or_cmake_cannot_restore_under_new_identity(self):
        # Keep Rust/compiler/source/dependencies fixed while the runner or CMake
        # changes. An old native install must match neither the new exact key
        # nor its dependency-only restore prefix before a bundle is published.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            env = {'ImageOS': 'macos15', 'RUNNER_OS': 'macOS',
                   'RUNNER_ARCH': 'ARM64', 'ImageVersion': 'image A'}

            def identities(image, cmake):
                env['ImageVersion'] = image
                def command(args):
                    return cmake if args == ['cmake', '--version'] else 'fixed Rust/Cargo'
                with patch.object(cargo_cache, 'run', side_effect=command), patch.object(cargo_cache, 'compiler_identity', return_value='fixed clang/SDK'), patch.object(share, 'run', return_value='fixed source tree'), patch.object(share.Path, 'cwd', return_value=root), patch.dict(os.environ, env, clear=True):
                    native = cargo_cache.cache_identity('bundle-macos-metal', 'release', env, root, provenance=True)
                    key, prefix = cargo_cache.cache_key('bundle-macos-metal', native, 'fixed dependencies', env)
                    bundle = share.identity('macos-metal')
                    return key, prefix, bundle

            old_key, old_prefix, old_bundle = identities('image A', 'cmake A')
            for image, cmake in [('image B', 'cmake A'), ('image A', 'cmake B')]:
                with self.subTest(image=image, cmake=cmake):
                    key, prefix, bundle = identities(image, cmake)
                    self.assertNotEqual(old_key, key)
                    self.assertNotEqual(old_prefix, prefix)
                    self.assertFalse(old_key.startswith(prefix))
                    self.assertNotEqual(old_bundle, bundle)

    def test_non_bundle_cache_survives_image_and_cmake_releases(self):
        # Only caches behind a bundle identity pay for the image revision and
        # CMake; the others must keep their key across weekly image releases.
        env = {'ImageOS': 'ubuntu24', 'RUNNER_OS': 'Linux', 'RUNNER_ARCH': 'X64'}

        def key(image, cmake, provenance):
            def command(args):
                return cmake if args == ['cmake', '--version'] else 'fixed Rust/Cargo'
            env['ImageVersion'] = image
            with tempfile.TemporaryDirectory() as tmp, patch.object(cargo_cache, 'run', side_effect=command), patch.object(cargo_cache, 'compiler_identity', return_value='fixed gcc'):
                identity = cargo_cache.cache_identity('ci', 'debug', env, Path(tmp), provenance)
                return cargo_cache.cache_key('ci', identity, 'fixed dependencies', env)

        self.assertEqual(key('image A', 'cmake A', False), key('image B', 'cmake B', False))
        self.assertNotEqual(key('image A', 'cmake A', True), key('image B', 'cmake A', True))
        self.assertNotEqual(key('image A', 'cmake A', True), key('image A', 'cmake B', True))

    def test_bundle_identity_uses_the_provenance_cache_identity(self):
        with patch.object(share, 'cache_identity', return_value='identity') as identity, patch.object(share, 'run', return_value='tree'):
            share.identity('linux-vulkan')
        self.assertTrue(identity.call_args.kwargs['provenance'])

    def test_source_and_toolchain_must_match(self):
        with patch.object(share, 'cache_identity', return_value='rust A; sdk A'), patch.object(share, 'run', return_value='tree A'):
            original = share.identity('linux-vulkan')
            self.assertEqual(original, share.identity('linux-vulkan'))
            with patch.object(share, 'run', return_value='tree B'):
                self.assertNotEqual(original, share.identity('linux-vulkan'))
            with patch.object(share, 'cache_identity', return_value='rust B; sdk A'):
                self.assertNotEqual(original, share.identity('linux-vulkan'))

    def test_manual_preview_never_waits_for_unrelated_producer(self):
        with tempfile.TemporaryDirectory() as tmp:
            event = Path(tmp) / 'event.json'
            event.write_text('{}')
            with patch.dict(os.environ, {'GITHUB_EVENT_PATH': str(event), 'GITHUB_EVENT_NAME': 'workflow_dispatch'}), patch.object(share, 'api') as api:
                self.assertEqual('', share.coordinate())
                api.assert_not_called()

EVENT = '{"number":137,"pull_request":{"head":{"sha":"source","repo":{"full_name":"owner/repo"}}}}'
SAME_PR = {'id': 42, 'head_sha': 'source', 'head_repository': {'full_name': 'owner/repo'},
           'pull_requests': [{'number': 137}]}


class CoordinatorTests(unittest.TestCase):
    def coordinate(self, responses, event=EVENT):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'event.json'
            path.write_text(event)
            event = path
            with patch.dict(os.environ, {'GITHUB_EVENT_PATH': str(event), 'GITHUB_EVENT_NAME': 'pull_request', 'GITHUB_REPOSITORY': 'owner/repo'}), patch.object(share, 'api', side_effect=responses) as api:
                return share.coordinate(), api.call_args_list

    def test_same_pr_early_uploads_allow_reuse_before_tests_finish(self):
        names = ['shared-bundle-' + b for b in ['linux-vulkan', 'windows-vulkan', 'macos-metal']]
        value, calls = self.coordinate([
            {'workflow_runs': [SAME_PR]},
            {'artifacts': [{'name': name, 'expired': False} for name in names]},
        ])
        self.assertEqual(value, '42')
        self.assertIn('head_sha=source', calls[0].args[0])
        self.assertIn('/42/artifacts', calls[1].args[0])

    def test_fork_pr_runs_match_by_head_commit_and_repository(self):
        # GitHub lists no pull_requests for runs from forks.
        names = ['shared-bundle-' + b for b in ['linux-vulkan', 'windows-vulkan', 'macos-metal']]
        fork = '{"number":137,"pull_request":{"head":{"sha":"source","repo":{"full_name":"fork/repo"}}}}'
        other_fork = {'id': 41, 'head_sha': 'source', 'head_repository': {'full_name': 'other/repo'}, 'pull_requests': []}
        deleted_fork = {'id': 40, 'head_sha': 'source', 'head_repository': None, 'pull_requests': []}
        producer = {'id': 42, 'head_sha': 'source', 'head_repository': {'full_name': 'fork/repo'}, 'pull_requests': []}
        value, _ = self.coordinate([
            {'workflow_runs': [other_fork, deleted_fork, producer]},
            {'artifacts': [{'name': name, 'expired': False} for name in names]},
        ], event=fork)
        self.assertEqual(value, '42')

    def test_failed_or_cancelled_producer_falls_back_to_local_build(self):
        for conclusion in ['failure', 'cancelled']:
            with self.subTest(conclusion=conclusion):
                value, _ = self.coordinate([
                    {'workflow_runs': [SAME_PR]},
                    {'artifacts': []},
                    {'status': 'completed', 'conclusion': conclusion},
                ])
                self.assertEqual(value, '')

    def test_api_failure_at_each_query_stage_falls_back_to_local_build(self):
        candidate = {'workflow_runs': [SAME_PR]}
        for successful_responses in [[], [candidate], [candidate, {'artifacts': []}]]:
            with self.subTest(stage=len(successful_responses)), patch('sys.stdout', new_callable=io.StringIO) as diagnostic:
                value, _ = self.coordinate(successful_responses + [share.ProducerUnavailable('API failed')])
                self.assertEqual(value, '')
                self.assertIn('::warning::', diagnostic.getvalue())
                self.assertIn('using local bundle builds', diagnostic.getvalue())

    def test_unexpected_error_falls_back_to_local_build(self):
        for error in [KeyError('workflow_runs'), TypeError('bad payload'), ValueError('bad JSON')]:
            with self.subTest(error=error), patch('sys.stdout', new_callable=io.StringIO) as diagnostic:
                value, _ = self.coordinate([error])
                self.assertEqual(value, '')
                self.assertIn('::warning::', diagnostic.getvalue())
                self.assertIn(type(error).__name__, diagnostic.getvalue())

    def test_api_errors_are_bounded_and_main_writes_empty_producer_output(self):
        errors = [subprocess.CalledProcessError(1, ['gh', 'api'], stderr='HTTP 403'),
                  subprocess.CalledProcessError(1, ['gh', 'api'], stderr='HTTP 502'),
                  OSError('network unavailable'),
                  subprocess.TimeoutExpired(['gh', 'api'], 30)]
        for error in errors:
            with self.subTest(error=error), tempfile.TemporaryDirectory() as tmp:
                event = Path(tmp) / 'event.json'
                event.write_text(EVENT)
                output = Path(tmp) / 'output'
                env = {'GITHUB_EVENT_PATH': str(event), 'GITHUB_EVENT_NAME': 'pull_request',
                       'GITHUB_REPOSITORY': 'owner/repo', 'GITHUB_OUTPUT': str(output)}
                with patch.dict(os.environ, env), patch('sys.argv', ['ci_bundle_share.py', 'coordinate']), patch.object(share.subprocess, 'check_output', side_effect=error) as request, patch.object(share.time, 'sleep') as sleep, patch('sys.stdout', new_callable=io.StringIO) as diagnostic:
                    share.main()
                self.assertEqual('value=\n', output.read_text())
                self.assertEqual(3, request.call_count)
                self.assertEqual([1, 2], [c.args[0] for c in sleep.call_args_list])
                self.assertTrue(all(c.kwargs['timeout'] == 30 for c in request.call_args_list))
                self.assertIn('after 3 attempts', diagnostic.getvalue())
                self.assertIn('using local bundle builds', diagnostic.getvalue())

    def test_transient_api_failure_retries_then_succeeds(self):
        with patch.object(share.subprocess, 'check_output', side_effect=[OSError('network'), '{"artifacts": []}']) as request, patch.object(share.time, 'sleep') as sleep:
            self.assertEqual({'artifacts': []}, share.api('artifacts'))
            self.assertEqual(2, request.call_count)
            sleep.assert_called_once_with(1)


def release_jobs():
    """release.yml's jobs as {name: text}; stdlib only, so no YAML parser."""
    text = (Path(__file__).resolve().parent.parent / '.github/workflows/release.yml').read_text()
    jobs = text.split('\njobs:\n', 1)[1]
    parts = re.split(r'^  ([a-z0-9-]+):\n', jobs, flags=re.M)
    return dict(zip(parts[1::2], parts[2::2]))


class WorkflowFallbackTests(unittest.TestCase):
    """A coordinator failure of any kind must mean local builds, not skipped bundles."""

    def test_coordinator_failure_cannot_fail_the_run(self):
        job = release_jobs()['bundle-producer']
        self.assertIn('\n    timeout-minutes: 25\n    continue-on-error: true\n', job)
        step = job.split('- name: Locate same-PR bundle producer', 1)[1]
        self.assertIn('continue-on-error: true', step)
        self.assertIn('timeout-minutes: 22', step)

    def test_bundle_jobs_run_whatever_the_coordinator_result(self):
        jobs = release_jobs()
        for name in ['transcription-windows-vulkan', 'transcription-linux-vulkan', 'transcription-macos-metal']:
            with self.subTest(job=name):
                self.assertIn('needs: [package-tests, bundle-producer]', jobs[name])
                self.assertIn("if: ${{ !cancelled() && needs.package-tests.result == 'success' }}", jobs[name])

    def test_npm_jobs_still_need_every_bundle(self):
        jobs = release_jobs()
        pack = jobs['npm-pack']
        for need in ['package-tests', 'checksums', 'transcription-windows-vulkan',
                     'transcription-linux-vulkan', 'transcription-macos-metal']:
            self.assertIn(f'      - {need}\n', pack)
        # The default success() would skip it after a failed coordinator.
        for result in ['failure', 'cancelled', 'skipped']:
            self.assertIn(f"!contains(needs.*.result, '{result}')", pack)
        for name in ['npm-smoke', 'npm-smoke-managers']:
            with self.subTest(job=name):
                self.assertIn("if: ${{ !cancelled() && needs.npm-pack.result == 'success' }}", jobs[name])


if __name__ == '__main__':
    unittest.main()
