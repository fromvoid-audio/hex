"""Filesystem identification is bounded, read-only and independent of cache custody."""
import importlib.util
from pathlib import Path
import plistlib
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("ci_target_storage", Path(__file__).resolve().parents[1] / "ci_target_storage.py")
storage = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(storage)


class TargetStorageTests(unittest.TestCase):
    def test_macos_identifies_actual_device_for_new_target_without_creating_it(self):
        with tempfile.TemporaryDirectory() as directory:
            ancestor = Path(directory)
            target = ancestor / "future" / "target with spaces"
            for filesystem in ("apfs", "ExFAT", "hfs"):
                responses = [SimpleNamespace(returncode=0, stdout=b"Filesystem blocks used available capacity mounted\n/dev/disk42s1 100 20 80 20% /Volumes/has spaces\n"),
                             SimpleNamespace(returncode=0, stdout=plistlib.dumps({"FilesystemType": filesystem}))]
                with patch.object(storage.sys, "platform", "darwin"), patch.object(storage.subprocess, "run", side_effect=responses) as run:
                    result = storage.inspect(target, {"PATH": "fixture"})
                self.assertEqual(result["filesystem_type"], filesystem.lower())
                self.assertEqual(result["probe_path"], str(ancestor))
                self.assertEqual(result["device"], "/dev/disk42s1")
                self.assertGreaterEqual(result["duration_seconds"], 0)
                self.assertEqual([c.args[0] for c in run.call_args_list], [
                    ["df", "-P", str(ancestor)], ["diskutil", "info", "-plist", "/dev/disk42s1"]])
                self.assertTrue(all(c.kwargs["timeout"] == 5 and c.kwargs["env"] == {"PATH": "fixture"}
                                    for c in run.call_args_list))
                self.assertFalse(target.parent.exists())

    def test_probe_failures_refuse_without_inventing_filesystem(self):
        df = SimpleNamespace(returncode=0, stdout=b"Filesystem blocks\n/dev/disk42s1 100\n")
        bad = [OSError("not installed"), subprocess.TimeoutExpired("df", 5),
               SimpleNamespace(returncode=1, stdout=b""),
               SimpleNamespace(returncode=0, stdout=b"malformed"),
               SimpleNamespace(returncode=0, stdout=b"Filesystem\nnetwork-share 100\n")]
        for response in bad:
            with self.subTest(response=response), patch.object(storage.sys, "platform", "darwin"), patch.object(storage.subprocess, "run", side_effect=[response]):
                with self.assertRaisesRegex(ValueError, "cannot identify macOS"):
                    storage.inspect(Path.cwd(), {})
        for response in (SimpleNamespace(returncode=1, stdout=b""),
                         SimpleNamespace(returncode=0, stdout=b"not a plist"),
                         SimpleNamespace(returncode=0, stdout=b'<?xml version="1.0"?><plist><dict>'),
                         SimpleNamespace(returncode=0, stdout=plistlib.dumps({"Error": True})),
                         SimpleNamespace(returncode=0, stdout=plistlib.dumps({"FilesystemType": 42}))):
            with self.subTest(response=response), patch.object(storage.sys, "platform", "darwin"), patch.object(storage.subprocess, "run", side_effect=[df, response]):
                with self.assertRaisesRegex(ValueError, "cannot identify macOS"):
                    storage.inspect(Path.cwd(), {})

    def test_other_platforms_do_not_probe_or_claim_filesystem_support(self):
        for platform in ("linux", "win32"):
            with patch.object(storage.sys, "platform", platform), patch.object(storage.subprocess, "run") as run:
                result = storage.inspect(Path.cwd(), {})
                run.assert_not_called()
                self.assertEqual(result["status"], "not_checked")
                self.assertNotIn("filesystem_type", result)
