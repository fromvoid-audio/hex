"""Tests for the fixed helper request and one-shot publication boundary."""
import importlib.util
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch

PATH = Path(__file__).resolve().parents[1] / "hex-update-entry.py"
SPEC = importlib.util.spec_from_file_location("hex_update_entry_test", PATH)
entry = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(entry)

class Uncertain(Exception):
    pass

class FollowupFailed(Exception):
    pass


class EntryTests(unittest.TestCase):
    def test_interpreter_policy_binds_running_file_and_rejects_changed_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            resources = Path(directory)
            executable = resources / "python"
            executable.write_bytes(b"interpreter fixture")
            metadata = executable.stat()
            policy = {"version": 1, "interpreter": str(executable),
                      "interpreter_sha256": hashlib.sha256(executable.read_bytes()).hexdigest(),
                      "interpreter_device": metadata.st_dev, "interpreter_inode": metadata.st_ino}
            policy_path = resources / "policy.json"
            runtime = SimpleNamespace(assert_plain_path=Mock(),
                                      sha=lambda path: hashlib.sha256(path.read_bytes()).hexdigest())
            with patch.object(entry.subprocess, "run", return_value=SimpleNamespace(stdout=str(executable) + "\n")):
                policy_path.write_text(json.dumps(policy))
                entry.verify_interpreter(resources, runtime)
                for change in ({"version": True}, {"interpreter_inode": 0},
                               {"interpreter_inode": metadata.st_ino + 1},
                               {"interpreter_sha256": "0" * 64}):
                    with self.subTest(change=change):
                        policy_path.write_text(json.dumps({**policy, **change}))
                        with self.assertRaises(RuntimeError):
                            entry.verify_interpreter(resources, runtime)

    def test_selection_does_not_create_missing_installer_lock(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runtime = SimpleNamespace(assert_plain_path=Mock(), verify=Mock())
            with self.assertRaisesRegex(RuntimeError, "no update custody"):
                entry.verify_selection(root / "hex.app", "a" * 64, runtime)
            self.assertFalse((root / ".hex-test-build.lock").exists())
            runtime.verify.assert_not_called()

    def test_request_does_not_consume_the_later_approval(self):
        request = {"parent_pid": 123, "target": "/tmp/hex.app", "selected_receipt": "a" * 64,
                   "action": "install"}
        read_fd, write_fd = os.pipe()
        try:
            os.write(write_fd, json.dumps(request).encode() + b"\nhex-update-approved-v1\n")
            os.close(write_fd)
            write_fd = -1
            with patch.object(entry.sys, "stdin", SimpleNamespace(fileno=lambda: read_fd)):
                self.assertEqual(entry.read_request(), request)
            self.assertEqual(os.read(read_fd, 100), b"hex-update-approved-v1\n")
        finally:
            os.close(read_fd)
            if write_fd >= 0:
                os.close(write_fd)

    def test_request_rejects_extra_fields_and_nonfixed_actions(self):
        base = {"parent_pid": 123, "target": "/tmp/hex.app", "selected_receipt": "a" * 64,
                "action": "install"}
        for invalid in ({**base, "command": "anything"}, {**base, "action": "run"},
                        {**base, "target": "relative/hex.app"}, {**base, "parent_pid": True}):
            with self.subTest(invalid=invalid):
                read_fd, write_fd = os.pipe()
                try:
                    os.write(write_fd, json.dumps(invalid).encode() + b"\n")
                    os.close(write_fd)
                    write_fd = -1
                    with patch.object(entry.sys, "stdin", SimpleNamespace(fileno=lambda: read_fd)):
                        with self.assertRaisesRegex(RuntimeError, "invalid update request"):
                            entry.read_request()
                finally:
                    os.close(read_fd)
                    if write_fd >= 0:
                        os.close(write_fd)

    def invoke(self, action, *, observe_error=None, apply_error=None, launch_error=None):
        request = {"parent_pid": 123, "target": "/tmp/hex.app", "selected_receipt": "a" * 64,
                   "action": action}
        runtime = SimpleNamespace(_apply_verified_pending=Mock(side_effect=apply_error),
                                  PublicationUncertain=Uncertain, PublicationFollowupFailed=FollowupFailed)
        observer = SimpleNamespace(observe_parent_exit=Mock(side_effect=observe_error))
        spec = SimpleNamespace(loader=SimpleNamespace(exec_module=Mock()))
        status = Mock()
        with patch.multiple(entry, read_request=Mock(return_value=request),
                            runtime_module=Mock(return_value=runtime), verify_custody=Mock(),
                            verify_interpreter=Mock(), verify_selection=Mock(), write_result=status), \
                patch.object(entry.importlib.util, "spec_from_file_location", return_value=spec), \
                patch.object(entry.importlib.util, "module_from_spec", return_value=observer), \
                patch.object(entry.subprocess, "run", side_effect=launch_error) as launch:
            result = entry.main()
            return result, runtime, observer, status, launch

    def test_only_approved_exit_reaches_publication(self):
        result, runtime, observer, status, launch = self.invoke("install", observe_error=RuntimeError("crash"))
        self.assertEqual(result, 1)
        observer.observe_parent_exit.assert_called_once()
        runtime._apply_verified_pending.assert_not_called()
        launch.assert_not_called()
        self.assertEqual(status.call_args.args[1], "refused")

    def test_install_never_relaunches_and_restart_requests_once(self):
        result, runtime, _, status, launch = self.invoke("install")
        self.assertEqual(result, 0)
        runtime._apply_verified_pending.assert_called_once()
        launch.assert_not_called()
        self.assertEqual(status.call_args.args[1], "installed")
        result, runtime, _, status, launch = self.invoke("restart")
        self.assertEqual(result, 0)
        runtime._apply_verified_pending.assert_called_once()
        launch.assert_called_once_with(["/usr/bin/open", "-a", "/tmp/hex.app"], check=True, timeout=15,
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.assertEqual(status.call_args.args[1], "restart_requested")

    def test_publication_and_relaunch_failures_are_distinct(self):
        result, _, _, status, launch = self.invoke("restart", apply_error=RuntimeError("copy failed"))
        self.assertEqual(result, 1)
        launch.assert_not_called()
        self.assertEqual(status.call_args.args[1], "refused")
        result, _, _, status, launch = self.invoke("restart", launch_error=subprocess.CalledProcessError(1, "open"))
        self.assertEqual(result, 1)
        launch.assert_called_once()
        self.assertEqual(status.call_args.args[1], "installed_restart_failed")

    def test_committed_and_uncertain_publication_are_not_reported_as_refusal(self):
        for error, expected in ((Uncertain("verify failed"), "publication_uncertain"),
                                (FollowupFailed("receipt failed"), "installed_followup_failed")):
            with self.subTest(expected=expected):
                result, _, _, status, launch = self.invoke("restart", apply_error=error)
                self.assertEqual(result, 1)
                launch.assert_not_called()
                self.assertEqual(status.call_args.args[1], expected)


if __name__ == "__main__":
    unittest.main()
