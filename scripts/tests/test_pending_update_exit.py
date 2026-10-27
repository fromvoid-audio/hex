"""Exercise the inherited-pipe/process boundary with disposable processes."""
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[2]
PARENT = r'''
import os, select, signal, subprocess, sys, time
root, result, mode = sys.argv[1:]
read_approval, write_approval = os.pipe()
read_ready, write_ready = os.pipe()
helper = r"""
import importlib.util, os, sys
from pathlib import Path
sys.path.insert(0, sys.argv[1] + '/scripts')
spec = importlib.util.spec_from_file_location('hex_test_build_for_observer', sys.argv[1] + '/scripts/hex-test-build.py')
delivery = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = delivery
spec.loader.exec_module(delivery)
delivery._apply_verified_pending = lambda target, selected: 'approved'
try:
    result = delivery.apply_after_approved_parent_exit(int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4]), Path(sys.argv[7]), '', observer_timeout=1)
except delivery.DeliveryError as e:
    result = 'refused: ' + str(e)
with open(sys.argv[5] + '.tmp', 'w') as output:
    output.write(result)
os.replace(sys.argv[5] + '.tmp', sys.argv[5])
"""
actual_exe = subprocess.check_output(['/bin/ps', '-ww', '-p', str(os.getpid()), '-o', 'comm='], text=True).strip()
if mode == 'wrong_executable':
    actual_exe = '/bin/sh'
target = os.path.join(os.path.dirname(result), 'hex.app')
os.makedirs(os.path.join(target, 'Contents/MacOS'))
os.symlink(actual_exe, os.path.join(target, 'Contents/MacOS/hex'))
subprocess.Popen([sys.executable, '-c', helper, root, str(os.getpid()),
                  str(read_approval), str(write_ready), result, actual_exe, target],
                 pass_fds=(read_approval, write_ready), stdout=subprocess.DEVNULL,
                 stderr=None, close_fds=True)
os.close(read_approval)
os.close(write_ready)
if not select.select([read_ready], [], [], 8)[0]:
    sys.exit(90)
ready = os.read(read_ready, 100)
if ready != b'hex-update-observer-ready-v1\n':
    sys.exit(91)
if mode == 'bad':
    os.write(write_approval, b'wrong\n')
elif mode == 'extra':
    os.write(write_approval, b'hex-update-approved-v1\nextra')
elif mode == 'partial':
    os.write(write_approval, b'hex-update-approved')
elif mode == 'timeout':
    time.sleep(2)
elif mode != 'missing':
    os.write(write_approval, b'hex-update-approved-v1\n')
os.close(write_approval)
if mode == 'signal':
    os.kill(os.getpid(), signal.SIGTERM)
sys.exit(7 if mode == 'failure' else 0)
'''


@unittest.skipUnless(sys.platform == 'darwin', 'requires Darwin process kqueue')
class ParentExitTests(unittest.TestCase):
    def observe(self, mode):
        with tempfile.TemporaryDirectory() as directory:
            result = Path(directory) / 'result'
            parent = subprocess.run([sys.executable, '-c', PARENT, str(ROOT), str(result), mode],
                                    capture_output=True, timeout=15)
            if mode != 'wrong_executable':
                self.assertNotIn(parent.returncode, (90, 91), parent.stderr.decode())
            deadline = time.monotonic() + 8
            while not result.exists() and time.monotonic() < deadline:
                time.sleep(0.01)
            self.assertTrue(result.exists(), 'observer did not return a result')
            return parent.returncode, result.read_text()

    def test_approved_clean_exit(self):
        status, result = self.observe('clean')
        self.assertEqual((status, result), (0, 'approved'))

    def test_refuses_missing_and_invalid_approval(self):
        for mode in ('missing', 'bad', 'partial', 'extra', 'timeout'):
            with self.subTest(mode=mode):
                status, result = self.observe(mode)
                self.assertEqual(status, 0)
                self.assertTrue(result.startswith('refused:'), result)

    def test_refuses_wrong_parent_executable_before_readiness(self):
        status, result = self.observe('wrong_executable')
        self.assertEqual(status, 91)
        self.assertIn('parent executable does not match', result)

    def test_refuses_nonzero_exit_or_signal_after_approval(self):
        for mode in ('failure', 'signal'):
            with self.subTest(mode=mode):
                status, result = self.observe(mode)
                self.assertNotEqual(status, 0)
                self.assertTrue(result.startswith('refused:'), result)


if __name__ == '__main__':
    unittest.main()
