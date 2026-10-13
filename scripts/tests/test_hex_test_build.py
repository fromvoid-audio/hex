"""Deterministic tests for exact-source delivery and interruption recovery."""
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import subprocess
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch

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

    def test_signature_failure_after_rename_restores_previous_app(self):
        self.bundle(self.app, b"old")
        self.bundle(self.stage)
        original = delivery.verify_native_executable
        def reject_installed(binary):
            if binary == self.app / "Contents/MacOS/hex":
                raise subprocess.CalledProcessError(1, ["codesign"])
            return original(binary)
        with patch.object(delivery, "verify_native_executable", side_effect=reject_installed):
            with self.assertRaises(subprocess.CalledProcessError):
                delivery.activate(self.app, self.stage)
        self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"old")
        self.assertEqual((self.stage / "Contents/MacOS/hex").read_bytes(), b"new")

    def test_activation_is_serialized(self):
        with delivery.activation_lock(self.app):
            with self.assertRaisesRegex(delivery.DeliveryError, "another"):
                with delivery.activation_lock(self.app):
                    self.fail("second installer entered")

    def test_production_build_assembles_and_replaces_stable_app(self):
        # Exercise real assembly/receipt/activation with native external tools
        # substituted. A missing Contents parent failed actual post-merge delivery.
        self.bundle(self.app, b"old")
        source = self.root / "repo"
        source.mkdir()
        for relative in ("scripts", "assets/fonts/ioskeley-mono", "third_party/gpui"):
            (source / relative).mkdir(parents=True)
        (source / "scripts/hex-test-build.json").write_text('{"app_path":"artifacts/hex.app"}')
        (source / "assets/fonts/ioskeley-mono/OFL.txt").write_text("font license")
        (source / "third_party/gpui/LICENSE-APACHE").write_text("gpui license")
        # The reported executable deliberately lives outside target/release.
        target = self.root / "configured-target/aarch64-apple-darwin/release"
        target.mkdir(parents=True)
        executable = target / "hex-desktop"
        executable.write_bytes(b"optimized fixture")
        executable.chmod(0o755)
        candidate = "a" * 40
        report = self.root / "ci.json"
        report.write_text('{"status":"passed"}')
        included = self.root / "included.json"
        included.write_text(json.dumps([{"ticket":2411, "ticket_revision":9,
                                         "pr":"https://github.com/PORTALSURFER/hex/pull/168",
                                         "head":candidate}]))
        checklist = self.root / "USER-TEST.md"
        checklist.write_text("1. launch stable app\n2. inspect source identity\n")
        runner = delivery.ci.LocalCiRunner(source)
        runner.summary["start_tree"] = {"commit":candidate, "dirty":False,
                                        "content_sha256":"digest"}
        runner.cargo_identity = {"target_directory":str(target.parent.parent)}
        runner.target_lock = SimpleNamespace(release=Mock(return_value=None))
        event = {"reason":"compiler-artifact", "executable":str(executable),
                 "target":{"name":"hex-desktop"}, "profile":{"opt_level":"3"}}
        def tool(argv, root=source):
            if argv[:2] == ["git", "rev-parse"]:
                return str(source / ".git")
            if argv[:2] == ["cargo", "metadata"]:
                return json.dumps({"packages":[{"name":"hex-desktop", "version":"0.1.0"}]})
            return ""
        args = SimpleNamespace(ci_report=report, candidate=candidate,
                               included=included, checklist=checklist)
        with patch.object(delivery, "ROOT", source), \
                patch.object(delivery.sys, "platform", "darwin"), \
                patch.dict(os.environ, {"HEX_DEV_APP_PATH":str(self.app)}), \
                patch.object(delivery.ci, "LocalCiRunner", return_value=runner), \
                patch.object(delivery, "validate_ci", return_value={}), \
                patch.object(runner, "_resolve_cargo_identity", return_value=True), \
                patch.object(runner, "_check_disk_headroom", return_value=True), \
                patch.object(runner, "_prepare_target_cache", return_value=True), \
                patch.object(delivery, "run", side_effect=tool), \
                patch.object(delivery.subprocess, "run", return_value=SimpleNamespace(stdout=json.dumps(event))):
            receipt = delivery.build(args)
        verified = delivery.verify(self.app)
        self.assertEqual(verified["source"], candidate)
        self.assertEqual(verified["profile"], "release")
        self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"optimized fixture")
        self.assertEqual((self.app / "Contents/Resources/USER-TEST.md").read_text(), checklist.read_text())
        self.assertEqual((self.app / "Contents/Resources/local-ci-summary.json").read_bytes(), report.read_bytes())
        self.assertEqual((self.app / "Contents/Resources/licenses/ioskeley-mono-OFL.txt").read_text(), "font license")
        self.assertEqual((Path(receipt["rollback_path"]) / "Contents/MacOS/hex").read_bytes(), b"old")
        self.assertEqual(json.loads((self.root / "hex-test-build-receipt.json").read_text()), receipt)
        runner.target_lock.release.assert_called_once()

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


@unittest.skipUnless(sys.platform == "darwin", "native macOS signature behavior")
class NativeSignatureTests(unittest.TestCase):
    def test_linker_signature_in_unsigned_bundle_and_tampering(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            app = root / "hex.app"
            binary = app / "Contents/MacOS/hex"
            binary.parent.mkdir(parents=True)
            resources = app / "Contents/Resources"
            resources.mkdir()
            source = root / "main.c"
            source.write_text("int main(void) { return 0; }\n")
            subprocess.run(["clang", str(source), "-Wl,-adhoc_codesign", "-o", str(binary)], check=True, capture_output=True)
            with (app / "Contents/Info.plist").open("wb") as output:
                plistlib.dump({"CFBundleIdentifier": delivery.IDENTITY, "CFBundleExecutable": "hex"}, output)
            resource = resources / "fixture.txt"
            resource.write_text("original")
            receipt = resources / "test-build.json"
            delivery.write_json(receipt, {"files": delivery.inventory(app)})
            delivery.verify(app)
            resource.write_text("tampered")
            with self.assertRaisesRegex(delivery.DeliveryError, "checksum"):
                delivery.verify(app)
            resource.write_text("original")
            # Rehashing cannot conceal an invalid executable signature.
            with binary.open("r+b") as output:
                output.seek(4096)
                old = output.read(1)
                output.seek(4096)
                output.write(bytes([old[0] ^ 1]))
            delivery.write_json(receipt, {"files": delivery.inventory(app)})
            with self.assertRaises(subprocess.CalledProcessError):
                delivery.verify(app)


if __name__ == "__main__":
    unittest.main()
