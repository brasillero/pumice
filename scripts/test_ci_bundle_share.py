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


if __name__ == '__main__':
    unittest.main()
