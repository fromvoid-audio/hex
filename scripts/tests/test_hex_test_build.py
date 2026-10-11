"""Deterministic tests for exact-source delivery and interruption recovery."""
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import sys
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("hex_test_build", Path(__file__).resolve().parents[1] / "hex-test-build.py")
delivery = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = delivery
SPEC.loader.exec_module(delivery)


class DeliveryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name).resolve()
        self.app = self.root / "hex.app"
        self.stage = self.root / ".hex-stage-test.app"
        self.backup = self.root / ".hex-rollback-test.app"
        self.journal = self.root / ".hex-test-build-transaction.json"
        self.mock = patch.object(delivery, "run", return_value="")
        self.mock.start()

    def tearDown(self):
        self.mock.stop()
        self.temp.cleanup()

    def bundle(self, path, data=b"new"):
        (path / "Contents/MacOS").mkdir(parents=True)
        (path / "Contents/Resources").mkdir()
        binary = path / "Contents/MacOS/hex"
        binary.write_bytes(data)
        binary.chmod(0o755)
        with (path / "Contents/Info.plist").open("wb") as output:
            plistlib.dump({"CFBundleIdentifier": delivery.IDENTITY, "CFBundleExecutable": "hex"}, output)
        delivery.write_json(path / "Contents/Resources/test-build.json", {"files": delivery.inventory(path)})

    def test_replacement_retains_old_bundle_and_user_files(self):
        self.bundle(self.app, b"old")
        self.bundle(self.stage)
        user = self.root / "project.hex"
        user.write_bytes(b"user project")
        backup = delivery.activate(self.app, self.stage)
        self.assertEqual((backup / "Contents/MacOS/hex").read_bytes(), b"old")
        self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"new")
        self.assertEqual(user.read_bytes(), b"user project")
        self.assertFalse(self.journal.exists())
        delivery.verify(self.app)

    def test_running_case_alias_refuses_before_mutation(self):
        self.bundle(self.app, b"old")
        self.bundle(self.stage)
        with patch.object(delivery, "run", return_value=f"123 {self.root}/Hex.app/Contents/MacOS/hex"):
            with self.assertRaisesRegex(delivery.DeliveryError, "running"):
                delivery.activate(self.app, self.stage)
        self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"old")
        self.assertFalse(self.journal.exists())

    def test_unrelated_identity_and_symlink_are_refused(self):
        self.bundle(self.app)
        self.bundle(self.stage)
        with (self.app / "Contents/Info.plist").open("wb") as output:
            plistlib.dump({"CFBundleIdentifier": "someone.else", "CFBundleExecutable": "hex"}, output)
        with self.assertRaisesRegex(delivery.DeliveryError, "unrelated"):
            delivery.activate(self.app, self.stage)
        link = self.root / "alias.app"
        link.symlink_to(self.app)
        with self.assertRaisesRegex(delivery.DeliveryError, "symlink"):
            delivery.assert_identity(link)

    def test_failed_validation_keeps_previous_application(self):
        self.bundle(self.app, b"old")
        self.bundle(self.stage)
        (self.stage / "Contents/MacOS/hex").write_bytes(b"corrupt")
        with self.assertRaisesRegex(delivery.DeliveryError, "checksum"):
            delivery.activate(self.app, self.stage)
        self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"old")

    def test_interruption_between_renames_is_recoverable(self):
        self.bundle(self.app, b"old")
        self.bundle(self.stage)
        rename = Path.rename
        def interrupted(path, destination):
            if path == self.stage:
                raise KeyboardInterrupt
            return rename(path, destination)
        with patch.object(Path, "rename", interrupted):
            with self.assertRaises(KeyboardInterrupt):
                delivery.activate(self.app, self.stage)
        self.assertFalse(self.app.exists())
        delivery.recover(self.app, self.journal)
        self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"old")
        self.assertTrue(self.stage.exists())

    def test_recovery_restores_backup_when_new_artifact_is_corrupt(self):
        self.bundle(self.backup, b"old")
        self.bundle(self.app)
        (self.app / "Contents/MacOS/hex").write_bytes(b"broken")
        delivery.write_json(self.journal, {"stage": str(self.stage), "backup": str(self.backup)})
        delivery.recover(self.app, self.journal)
        self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"old")
        self.assertEqual((self.stage / "Contents/MacOS/hex").read_bytes(), b"broken")

    def test_activation_is_serialized(self):
        with delivery.activation_lock(self.app):
            with self.assertRaisesRegex(delivery.DeliveryError, "another"):
                with delivery.activation_lock(self.app):
                    self.fail("second installer entered")

    def test_ci_gate_rejects_shortcuts_stale_and_changed_source(self):
        runner = delivery.ci.LocalCiRunner(self.root, steps=(delivery.ci.Step("fake", ("true",)),))
        tree = {"commit": "a" * 40, "dirty": False, "dirty_files": [], "content_sha256": "digest"}
        runner.summary["start_tree"] = tree
        evidence = {"mode": "full", "status": "passed", "stale": False, "exit_code": 0,
                    "commit": tree["commit"], "dirty": False, "start_tree": tree, "end_tree": tree,
                    "steps": [{"step_id": step.step_id, "argv": list(step.argv), "status": "passed"}
                              for step in delivery.ci.FULL_STEPS]}
        report = self.root / "ci.json"
        with patch.object(runner, "_capture_repository_state", return_value=True):
            delivery.write_json(report, evidence)
            delivery.validate_ci(report, runner, tree["commit"])
            for field, value in (("mode", "quick"), ("stale", True), ("status", "incomplete"),
                                 ("steps", evidence["steps"][:-1]), ("dirty", True)):
                delivery.write_json(report, {**evidence, field: value})
                with self.assertRaises(delivery.DeliveryError):
                    delivery.validate_ci(report, runner, tree["commit"])
            delivery.write_json(report, evidence)
            runner.summary["start_tree"] = {**tree, "content_sha256": "changed"}
            with self.assertRaises(delivery.DeliveryError):
                delivery.validate_ci(report, runner, tree["commit"])


if __name__ == "__main__":
    unittest.main()
