"""Offline checks for the exact-identity bundle reuse gate."""
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import ci_bundle_share as share


class IdentityTests(unittest.TestCase):
    def test_source_toolchain_and_runner_must_all_match(self):
        with patch.object(share, 'cache_identity', return_value='rust A; sdk A'), patch.object(share, 'run', return_value='tree A'), patch.dict(os.environ, {'ImageVersion': 'image A'}):
            original = share.identity('linux-vulkan')
            self.assertEqual(original, share.identity('linux-vulkan'))
            with patch.object(share, 'run', return_value='tree B'):
                self.assertNotEqual(original, share.identity('linux-vulkan'))
            with patch.object(share, 'cache_identity', return_value='rust B; sdk A'):
                self.assertNotEqual(original, share.identity('linux-vulkan'))
            with patch.dict(os.environ, {'ImageVersion': 'image B'}):
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


if __name__ == '__main__':
    unittest.main()
