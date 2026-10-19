import sys, unittest, os
from pathlib import Path
from unittest.mock import patch
sys.path.insert(0, '/workspace/hex/scripts/tests')
import test_launch_restart as t
class SupportedConfiguredPath(t.PreflightTests):
    def test_configured_override_reaches_verified_preflight(self):
        self.prepare_fixture()
        config = self.f.repo / 'scripts/hex-test-build.json'
        config.parent.mkdir(parents=True, exist_ok=True)
        config.write_text('{"app_path":"artifacts/hex.app"}')
        original = self.owner.read_bytes()
        with patch.dict(os.environ, {'HEX_DEV_APP_PATH': str(self.app)}):
            self.assertEqual(t.launch.configured_app(self.f.repo), self.app)
            try:
                self.prepare()
            finally:
                self.factory.assert_not_called()
                self.assertEqual(self.owner.read_bytes(), original)
                self.assertIsNone(self.runner.target_lock)
                print('REFUSAL: no controller construction, owner unchanged, no target lock', flush=True)
if __name__ == '__main__':
    suite = unittest.TestSuite([SupportedConfiguredPath('test_configured_override_reaches_verified_preflight')])
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    raise SystemExit(not result.wasSuccessful())
