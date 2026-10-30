"""Offline checks for the exact-identity bundle reuse gate."""
import io
import os
from pathlib import Path
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
                    native = cargo_cache.cache_identity('bundle-macos-metal', 'release', env, root)
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

class CoordinatorTests(unittest.TestCase):
    def coordinate(self, responses):
        with tempfile.TemporaryDirectory() as tmp:
            event = Path(tmp) / 'event.json'
            event.write_text('{"number":137,"pull_request":{"head":{"sha":"source"}}}')
            with patch.dict(os.environ, {'GITHUB_EVENT_PATH': str(event), 'GITHUB_EVENT_NAME': 'pull_request', 'GITHUB_REPOSITORY': 'owner/repo'}), patch.object(share, 'api', side_effect=responses) as api:
                return share.coordinate(), api.call_args_list

    def test_same_pr_early_uploads_allow_reuse_before_tests_finish(self):
        names = ['shared-bundle-' + b for b in ['linux-vulkan', 'windows-vulkan', 'macos-metal']]
        value, calls = self.coordinate([
            {'workflow_runs': [{'id': 42, 'pull_requests': [{'number': 137}]}]},
            {'artifacts': [{'name': name, 'expired': False} for name in names]},
        ])
        self.assertEqual(value, '42')
        self.assertIn('head_sha=source', calls[0].args[0])
        self.assertIn('/42/artifacts', calls[1].args[0])

    def test_failed_or_cancelled_producer_falls_back_to_local_build(self):
        for conclusion in ['failure', 'cancelled']:
            with self.subTest(conclusion=conclusion):
                value, _ = self.coordinate([
                    {'workflow_runs': [{'id': 42, 'pull_requests': [{'number': 137}]}]},
                    {'artifacts': []},
                    {'status': 'completed', 'conclusion': conclusion},
                ])
                self.assertEqual(value, '')

    def test_api_failure_at_each_query_stage_falls_back_to_local_build(self):
        candidate = {'workflow_runs': [{'id': 42, 'pull_requests': [{'number': 137}]}]}
        for successful_responses in [[], [candidate], [candidate, {'artifacts': []}]]:
            with self.subTest(stage=len(successful_responses)), patch('sys.stderr', new_callable=io.StringIO) as diagnostic:
                value, _ = self.coordinate(successful_responses + [share.ProducerUnavailable('API failed')])
                self.assertEqual(value, '')
                self.assertIn('using local bundle builds', diagnostic.getvalue())

    def test_api_errors_are_bounded_and_main_writes_empty_producer_output(self):
        errors = [subprocess.CalledProcessError(1, ['gh', 'api'], stderr='HTTP 403'),
                  subprocess.CalledProcessError(1, ['gh', 'api'], stderr='HTTP 502'),
                  OSError('network unavailable'),
                  subprocess.TimeoutExpired(['gh', 'api'], 30)]
        for error in errors:
            with self.subTest(error=error), tempfile.TemporaryDirectory() as tmp:
                event = Path(tmp) / 'event.json'
                event.write_text('{"number":137,"pull_request":{"head":{"sha":"source"}}}')
                output = Path(tmp) / 'output'
                env = {'GITHUB_EVENT_PATH': str(event), 'GITHUB_EVENT_NAME': 'pull_request',
                       'GITHUB_REPOSITORY': 'owner/repo', 'GITHUB_OUTPUT': str(output)}
                with patch.dict(os.environ, env), patch('sys.argv', ['ci_bundle_share.py', 'coordinate']), patch.object(share.subprocess, 'check_output', side_effect=error) as request, patch.object(share.time, 'sleep') as sleep, patch('sys.stderr', new_callable=io.StringIO) as diagnostic, patch('sys.stdout', new_callable=io.StringIO):
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


if __name__ == '__main__':
    unittest.main()
