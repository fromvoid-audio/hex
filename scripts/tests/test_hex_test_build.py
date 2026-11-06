"""Deterministic tests for exact-source delivery and interruption recovery."""
import importlib.util
import errno
import io
import json
import os
from pathlib import Path
import plistlib
import subprocess
import sys
import tempfile
import shutil
import signal
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch
from contextlib import contextmanager, redirect_stderr

SPEC = importlib.util.spec_from_file_location("hex_test_build", Path(__file__).resolve().parents[1] / "hex-test-build.py")
delivery = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = delivery
SPEC.loader.exec_module(delivery)
SIGNING = {"certificate_sha1": "4EDF56A7ADD0B4786C2EEE23A50D61620BCE7E4E", "team_identifier": "DKTKQ8U5T8"}
SIGNED = {"identifier": delivery.IDENTITY, "team_identifier": SIGNING["team_identifier"],
          "designated_requirement": f'designated => identifier "{delivery.IDENTITY}" and anchor apple generic and certificate leaf[subject.OU] = {SIGNING["team_identifier"]}'}


class DeliveryTests(unittest.TestCase):
    def test_update_runtime_import_does_not_load_build_tools(self):
        spec = importlib.util.spec_from_file_location(
            "hex_update_runtime", Path(__file__).resolve().parents[1] / "hex-test-build.py")
        runtime = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = runtime
        try:
            spec.loader.exec_module(runtime)
            self.assertFalse(hasattr(runtime, "ci"))
            self.assertFalse(hasattr(runtime, "changelog"))
            self.assertTrue(callable(runtime.apply_after_approved_parent_exit))
            self.assertIsNot(runtime.DeliveryError, delivery.DeliveryError)
        finally:
            sys.modules.pop(spec.name, None)

    def test_full_checkpoint_delivery_requires_unchanged_original_report_and_log(self):
        # Synthetic full-plan certificates exercise the consumer, not app CI.
        runner = delivery.ci.LocalCiRunner(self.root)
        tree = {"commit": "a" * 40, "dirty": False, "content_sha256": "fixture"}
        runner.summary["start_tree"] = tree
        folder = self.root / "artifacts/local-ci/original"
        folder.mkdir(parents=True)
        log = folder / "01-guard.log"
        log.write_text("guard passed\n")
        steps = delivery.ci.FULL_STEPS
        passed = [{"step_id": s.step_id, "argv": list(s.argv), "status": "passed",
                   "exit_code": 0, "error": None, "test_results": {"python_executions": [1]}}
                  for s in steps]
        passed[0]["log"] = str(log.relative_to(self.root))
        original = {"schema_version": 2, "mode": "full", "status": "failed", "stale": False,
                    "minimum_free_bytes": 2 * 1024 ** 3,
                    "exit_code": 2, "commit": tree["commit"], "dirty": False,
                    "start_tree": tree, "end_tree": tree, "disk_error": "capacity refusal",
                    "disk_refusal": {"before_step": 1},
                    "steps": [passed[0], *[{"step_id": s.step_id, "argv": list(s.argv),
                              "status": "not_run", "log": None, "exit_code": None} for s in steps[1:]]]}
        target = {"target_identity": {"device": 1, "inode": 2}, "dependency_inputs": {"fixture": True}}
        checkpoint = delivery.ci.ci_checkpoint
        original["checkpoint"] = checkpoint.seal(self.root, original, steps, {}, target)
        source = folder / "summary.json"
        source.write_text(json.dumps(original))
        lineage = {"report": str(source), "sha256": checkpoint.file_digest(source),
                   "passed_count": 1, "logs": original["checkpoint"]["logs"]}
        complete = {**original, "status": "passed", "exit_code": 0, "resume": lineage,
                    "target_cache": {"dependency_inputs": target["dependency_inputs"]},
                    "steps": [*checkpoint.inherited_steps(original, lineage), *passed[1:]]}
        complete.pop("checkpoint")
        report = self.root / "completed.json"
        report.write_text(json.dumps(complete))
        with patch.object(runner, "_capture_repository_state", return_value=True):
            delivery.validate_ci(report, runner, tree["commit"])
            log.write_text("changed\n")
            with self.assertRaisesRegex(delivery.DeliveryError, "inherited"):
                delivery.validate_ci(report, runner, tree["commit"])
            log.write_text("guard passed\n")
            source.write_text(source.read_text() + "\n")
            with self.assertRaisesRegex(delivery.DeliveryError, "inherited"):
                delivery.validate_ci(report, runner, tree["commit"])
            complete.pop("resume")
            report.write_text(json.dumps(complete))
            with self.assertRaisesRegex(delivery.DeliveryError, "lineage"):
                delivery.validate_ci(report, runner, tree["commit"])

    def test_current_review_malformed_ci_report_is_explicit_refusal(self):
        runner = delivery.ci.LocalCiRunner(self.root)
        tree = {"commit": "a" * 40, "dirty": False, "content_sha256": "fixture"}
        runner.summary["start_tree"] = tree
        evidence = {"schema_version": 2, "mode": "full", "status": "passed", "stale": False,
                    "exit_code": 0, "commit": tree["commit"], "dirty": False, "start_tree": tree, "end_tree": tree,
                    "steps": [{"step_id": s.step_id, "argv": list(s.argv), "status": "passed",
                               "test_results": {"python_executions": [1]}} for s in delivery.ci.FULL_STEPS]}
        report = self.root / "malformed-ci.json"
        cases = [None, [], {**evidence, "steps": None}, {**evidence, "steps": [None]},
                 {**evidence, "steps": [7]}, {**evidence, "steps": "bad"},
                 {**evidence, "steps": [{**evidence["steps"][0], "test_results": None}]},
                 {**evidence, "start_tree": None, "end_tree": None}]
        with patch.object(runner, "_capture_repository_state", return_value=True):
            for value in cases:
                report.write_text(json.dumps(value))
                with self.subTest(value=value), self.assertRaises(delivery.DeliveryError):
                    delivery.validate_ci(report, runner, tree["commit"])
            report.write_text(json.dumps(evidence))
            self.assertEqual(delivery.validate_ci(report, runner, tree["commit"]), evidence)

    def test_review_ci_gate_rejects_legacy_and_changed_guarded_inputs(self):
        runner = delivery.ci.LocalCiRunner(self.root)
        tree = {"commit": "a" * 40, "dirty": False, "content_sha256": "fixture"}
        runner.summary["start_tree"] = tree
        original = {"version": 1, "sha256": "a" * 64, "packages": []}
        evidence = {"schema_version": 2, "mode": "full", "status": "passed", "stale": False,
                    "exit_code": 0, "commit": tree["commit"], "dirty": False,
                    "start_tree": tree, "end_tree": tree, "target_cache": {"dependency_inputs": original},
                    "steps": [{"step_id": s.step_id, "argv": list(s.argv), "status": "passed",
                               "test_results": {"python_executions": [1]}} for s in delivery.ci.FULL_STEPS]}
        report = self.root / "review-ci.json"
        with patch.object(runner, "_capture_repository_state", return_value=True):
            for schema in (None, 1, True):
                delivery.write_json(report, {**evidence, "schema_version": schema})
                with self.subTest(schema=schema), self.assertRaisesRegex(delivery.DeliveryError, "schema"):
                    delivery.validate_ci(report, runner, tree["commit"])
            runner.target_lock = Mock()
            delivery.write_json(report, evidence)
            with patch.object(runner, "_dependency_context", return_value=original):
                delivery.validate_ci(report, runner, tree["commit"])
            with patch.object(runner, "_dependency_context", return_value={**original, "sha256": "b" * 64}):
                with self.assertRaisesRegex(delivery.DeliveryError, "toolchain|configuration"):
                    delivery.validate_ci(report, runner, tree["commit"])

    def test_cli_release_ownership_refusal_is_controlled_and_never_activates(self):
        arguments = ["hex-test-build", "--ci-report", "report.json", "--candidate", "a" * 40,
                     "--included", "included.json", "--checklist", "checklist.md"]
        errors = io.StringIO()
        with patch.object(delivery.sys, "argv", arguments), \
                patch.object(delivery.signal, "signal"), \
                patch.object(delivery, "build", side_effect=delivery.ci.TargetCacheError("release ownership is pending")), \
                patch.object(delivery, "activate") as activate, redirect_stderr(errors):
            self.assertEqual(delivery.main(), 1)
        self.assertIn("release ownership is pending", errors.getvalue())
        self.assertIn("temporary publication files preserved", errors.getvalue())
        activate.assert_not_called()

    def test_recovery_mutation_boundary_refuses_configured_or_raw_app_without_control(self):
        app=Path('/owned/hex.app')
        for observed in ('3 /owned/hex.app/Contents/MacOS/hex', '4 /other/hex-desktop'):
            with patch.object(delivery,'run',return_value=observed):
                with self.assertRaisesRegex(delivery.DeliveryError,'running'):
                    delivery.assert_recovery_inactive(app)
        with patch.object(delivery,'run',return_value='5 /other/other-program'):
            delivery.assert_recovery_inactive(app)

    def test_recovery_cli_requires_complete_identity_owner_and_explicit_adoption(self):
        base=['hex-test-build','--build-only','--candidate','a'*40,'--included','included.json','--checklist','list.md']
        cases=[['--expected-recreated-target-device','1'],
               ['--expected-recreated-target-inode','2'],
               ['--expected-recreated-target-device','1','--expected-recreated-target-inode','2'],
               ['--expected-recreated-target-device','-1','--expected-recreated-target-inode','2',
                '--adopt-release-target','--expected-release-owner-sha256','e'*64]]
        for args in cases:
            with patch.object(delivery.sys,'argv',base+args), patch.object(delivery,'build') as build, redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as error:delivery.main()
            self.assertEqual(error.exception.code,2);build.assert_not_called()

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name).resolve()
        self.app = self.root / "hex.app"
        self.workspace = delivery.install_workspace(self.app)
        self.workspace.mkdir()
        self.stage = self.workspace / "stage.app"
        self.backup = self.workspace / "previous.app"
        self.journal = self.root / ".hex-test-build-transaction.json"
        self.mock = patch.object(delivery, "run", return_value="")
        self.mock.start()
        self.signature_mock = patch.object(delivery, "native_signature", return_value={"team_identifier": "not set"})
        self.signature_mock.start()

    def tearDown(self):
        self.mock.stop()
        self.signature_mock.stop()
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

    def test_replacement_removes_temporary_custody_and_preserves_history_and_user_files(self):
        self.bundle(self.app, b"old")
        self.bundle(self.stage)
        user = self.root / "project.hex"
        user.write_bytes(b"user project")
        historical = self.root / ".hex-rollback-historical.app"
        self.bundle(historical, b"historical")
        delivery.activate(self.app, self.stage)
        self.assertEqual((historical / "Contents/MacOS/hex").read_bytes(), b"historical")
        self.assertFalse(self.workspace.exists())
        self.assertEqual(list(self.root.glob(".hex-rollback-*.app")), [historical])
        self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"new")
        self.assertEqual(user.read_bytes(), b"user project")
        self.assertFalse(self.journal.exists())
        delivery.verify(self.app)

    def test_first_install_publishes_complete_bundle_without_temporary_custody(self):
        self.bundle(self.stage)
        delivery.activate(self.app, self.stage)
        delivery.verify(self.app)
        self.assertFalse(self.workspace.exists())

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

    def test_signing_configuration_is_explicit_and_rejects_ad_hoc_or_partial_identity(self):
        self.assertEqual(delivery.signing_configuration({"signing": SIGNING}), SIGNING)
        for value in (None, {}, {"certificate_sha1": "-", "team_identifier": "DKTKQ8U5T8"},
                      {**SIGNING, "team_identifier": 'team" injection'}, {**SIGNING, "extra": True}):
            with self.subTest(value=value), self.assertRaises(delivery.DeliveryError):
                delivery.signing_configuration({"signing": value})

    def test_signed_bundle_cannot_downgrade_receipt_or_use_wrong_identity_or_cdhash(self):
        self.bundle(self.stage)
        with patch.object(delivery, "native_signature", return_value=SIGNED):
            with self.assertRaisesRegex(delivery.DeliveryError, "schema-2"):
                delivery.verify(self.stage)
        for actual in ({**SIGNED, "team_identifier": "OTHERTEAM0"},
                       {**SIGNED, "identifier": "another.app"},
                       {**SIGNED, "designated_requirement": "designated => cdhash H\"abc\""}):
            with self.subTest(actual=actual), self.assertRaisesRegex(delivery.DeliveryError, "stable"):
                delivery.verify_signed_bundle(self.stage, SIGNING, actual)

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
        delivery.recover(self.app)
        self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"old")
        self.assertFalse(self.workspace.exists())
        self.workspace.mkdir()
        self.bundle(self.stage, b"retry")
        delivery.activate(self.app, self.stage)
        self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"retry")

    def test_recovery_restores_backup_when_new_artifact_is_corrupt(self):
        self.bundle(self.backup, b"old")
        self.bundle(self.app)
        (self.app / "Contents/MacOS/hex").write_bytes(b"broken")
        delivery.recover(self.app)
        self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"old")
        self.assertFalse(self.workspace.exists())

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

    def test_publication_failure_restores_last_app_and_retry_discards_failed_stage(self):
        self.bundle(self.app, b"old")
        self.bundle(self.stage)
        rename = Path.rename
        def fail_publish(path, destination):
            if path == self.stage:
                raise OSError("publication failed")
            return rename(path, destination)
        with patch.object(Path, "rename", fail_publish):
            with self.assertRaisesRegex(OSError, "publication failed"):
                delivery.activate(self.app, self.stage)
        self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"old")
        delivery.recover(self.app)
        self.assertFalse(self.workspace.exists())

    def test_publication_verifier_timeout_restores_old_app(self):
        self.bundle(self.app, b"old")
        self.bundle(self.stage)
        original = delivery.verify
        def timeout_installed(path):
            if path == self.app:
                raise subprocess.TimeoutExpired("codesign", 30)
            return original(path)
        with patch.object(delivery, "verify", side_effect=timeout_installed):
            with self.assertRaises(subprocess.TimeoutExpired):
                delivery.activate(self.app, self.stage)
        self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"old")
        self.assertEqual((self.stage / "Contents/MacOS/hex").read_bytes(), b"new")

    def test_interruption_after_publication_finishes_cleanup(self):
        self.bundle(self.app, b"old")
        self.bundle(self.stage)
        original = delivery.verify
        def interrupted(path):
            if path == self.app:
                raise KeyboardInterrupt
            return original(path)
        with patch.object(delivery, "verify", interrupted):
            with self.assertRaises(KeyboardInterrupt):
                delivery.activate(self.app, self.stage)
        self.assertTrue(self.backup.exists())
        delivery.recover(self.app)
        self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"new")
        self.assertFalse(self.workspace.exists())

    def test_interrupted_old_custody_cleanup_can_finish_without_old_metadata(self):
        self.bundle(self.app, b"old")
        self.bundle(self.stage)
        remove = shutil.rmtree
        def interrupted(path):
            if path == self.backup:
                (path / "Contents/Info.plist").unlink()
                raise KeyboardInterrupt
            remove(path)
        with patch.object(delivery.shutil, "rmtree", interrupted):
            with self.assertRaises(KeyboardInterrupt):
                delivery.activate(self.app, self.stage)
        delivery.recover(self.app)
        self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"new")
        self.assertFalse(self.workspace.exists())

    def test_partial_stage_cleanup_and_legacy_journal_preservation(self):
        self.bundle(self.app, b"old")
        self.stage.mkdir()
        (self.stage / "partial").write_bytes(b"incomplete")
        delivery.recover(self.app)
        self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"old")
        self.assertFalse(self.workspace.exists())
        self.journal.write_text("historical uncertain installation")
        with self.assertRaisesRegex(delivery.DeliveryError, "legacy"):
            delivery.recover(self.app)
        self.assertEqual(self.journal.read_text(), "historical uncertain installation")

    def test_workspace_symlink_and_unexpected_contents_are_preserved(self):
        foreign = self.root / "foreign"
        foreign.mkdir()
        self.workspace.rmdir()
        self.workspace.symlink_to(foreign, target_is_directory=True)
        with self.assertRaisesRegex(delivery.DeliveryError, "symlink"):
            delivery.recover(self.app)
        self.workspace.unlink()
        self.workspace.mkdir()
        (self.workspace / "unknown").write_text("preserve")
        with self.assertRaisesRegex(delivery.DeliveryError, "unexpected"):
            delivery.recover(self.app)
        self.assertEqual((self.workspace / "unknown").read_text(), "preserve")

    def test_recovery_removes_only_regular_finder_metadata_and_is_idempotent(self):
        self.bundle(self.app)
        receipt = self.root / "hex-test-build-receipt.json"
        receipt.write_bytes(b"old adjacent receipt")
        metadata = self.workspace / ".DS_Store"
        metadata.write_bytes(b"finder metadata")
        with patch.object(delivery, "run", return_value=f"123 {self.app}/Contents/MacOS/hex"):
            with self.assertRaisesRegex(delivery.DeliveryError, "running"):
                delivery.recover(self.app)
        self.assertTrue(metadata.exists())
        delivery.recover(self.app)
        delivery.recover(self.app)
        self.assertFalse(self.workspace.exists())
        delivery.verify(self.app)
        # Recovery cleans custody; only the normal build publishes a new sidecar.
        self.assertEqual(receipt.read_bytes(), b"old adjacent receipt")

    def test_finder_name_is_not_a_directory_symlink_or_hidden_file_allowlist(self):
        self.bundle(self.app, b"old")
        self.bundle(self.stage)
        foreign = self.root / "project.hex"
        foreign.write_bytes(b"user project")
        metadata = self.workspace / ".DS_Store"
        for kind in ("directory", "symlink", "unknown"):
            with self.subTest(kind=kind):
                path = metadata if kind != "unknown" else self.workspace / ".other"
                if kind == "directory":
                    path.mkdir()
                    (path / "user").write_bytes(b"preserve")
                elif kind == "symlink":
                    path.symlink_to(foreign)
                else:
                    path.write_bytes(b"preserve")
                for operation in (lambda: delivery.activate(self.app, self.stage),
                                  lambda: delivery.recover(self.app)):
                    with self.assertRaisesRegex(delivery.DeliveryError, "unexpected"):
                        operation()
                    self.assertTrue(os.path.lexists(path))
                    self.assertEqual(foreign.read_bytes(), b"user project")
                    self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"old")
                    self.assertTrue(self.stage.exists())
                # Test fixture teardown only; the installer refused these members.
                if kind == "directory":
                    (path / "user").unlink()
                    path.rmdir()
                else:
                    path.unlink()

    def test_activation_is_serialized(self):
        with delivery.activation_lock(self.app):
            with self.assertRaisesRegex(delivery.DeliveryError, "another"):
                with delivery.activation_lock(self.app):
                    self.fail("second installer entered")

    def test_production_build_assembles_and_replaces_stable_app(self):
        self.assert_production_build(build_only=False)

    def test_default_build_stages_discovery_in_the_installed_app(self):
        self.assert_production_build(build_only=False, discovery=True)

    def test_discovery_input_builds_by_default_and_verifies_the_exact_candidate(self):
        candidate = "a" * 40
        args = SimpleNamespace(candidate=candidate, vst3_discovery_receipt=None,
                               vst3_sdk=None, without_vst3_discovery=False)
        sdk = self.root / "sdk"
        output = self.root / "vst3-discovery"
        receipt = output / "controlled-macos.json"
        builder = Mock()
        builder.build.return_value = receipt
        with patch.dict(os.environ, {"HEX_VST3_SDK": str(sdk)}), \
                patch.object(delivery, "discovery_builder", return_value=builder), \
                patch.object(delivery, "validate_discovery_input") as verify:
            self.assertEqual(delivery.prepare_discovery_input(args, self.root), receipt)
            builder.build.assert_called_once_with(sdk, output, discovery_only=True)
            verify.assert_called_once_with(receipt, candidate)
            verify.reset_mock()
            args.vst3_discovery_receipt = receipt
            self.assertEqual(delivery.prepare_discovery_input(args, self.root), receipt)
            builder.build.assert_called_once()
            verify.assert_called_once_with(receipt, candidate)

    def test_default_discovery_refuses_missing_sdk_and_allows_explicit_omission(self):
        args = SimpleNamespace(candidate="a" * 40, vst3_discovery_receipt=None,
                               vst3_sdk=None, without_vst3_discovery=False)
        with patch.dict(os.environ, {"HEX_VST3_SDK": ""}), \
                patch.object(delivery, "discovery_builder") as builder:
            with self.assertRaisesRegex(delivery.DeliveryError, "HEX_VST3_SDK"):
                delivery.prepare_discovery_input(args, self.root)
            args.without_vst3_discovery = True
            self.assertIsNone(delivery.prepare_discovery_input(args, self.root))
            builder.assert_not_called()

    def test_finder_metadata_after_custody_deletion_publishes_matching_receipt(self):
        self.assert_production_build(build_only=False, finder_metadata=True)

    def test_late_unknown_member_preserved_until_inspection_then_normal_retry(self):
        self.assert_production_build(build_only=False, finder_metadata="unknown")

    def test_late_finder_symlink_preserved_until_inspection_then_normal_retry(self):
        self.assert_production_build(build_only=False, finder_metadata="symlink")

    def test_finder_recreation_immediately_before_first_rmdir_finishes_publication(self):
        self.assert_production_build(build_only=False, rmdir_member="metadata")

    def test_unknown_immediately_before_first_rmdir_is_preserved(self):
        self.assert_production_build(build_only=False, rmdir_member="unknown")

    def test_symlink_immediately_before_first_rmdir_is_preserved(self):
        self.assert_production_build(build_only=False, rmdir_member="symlink")

    def test_repeated_finder_recreation_exhausts_bounded_retry_without_success(self):
        self.assert_production_build(build_only=False, rmdir_member="repeated")

    def test_nonempty_is_the_only_workspace_removal_error_retried(self):
        self.bundle(self.app)
        with patch.object(Path, "rmdir", side_effect=PermissionError(errno.EACCES, "denied")) as remove:
            with self.assertRaises(PermissionError):
                delivery.recover(self.app)
        remove.assert_called_once_with()
        self.assertTrue(self.workspace.exists())
        delivery.verify(self.app)

    def test_build_only_assembles_without_ci_and_records_untested_source(self):
        self.assert_production_build(build_only=True)

    def test_build_only_source_drift_during_build_preserves_installed_app(self):
        self.assert_production_build(build_only=True, drift="build")

    def test_build_only_source_drift_during_signing_preserves_installed_app(self):
        self.assert_production_build(build_only=True, drift="signing")

    def test_full_ci_receipt_reaches_stable_app_publication(self):
        self.assert_production_build(build_only=False, validate_ci_receipt=True)

    def test_full_ci_receipt_refuses_source_drift_before_publication(self):
        self.assert_production_build(build_only=False, validate_ci_receipt=True, drift="build")

    def test_supervisor_packaging_seals_fixed_runtime_and_rejects_changed_binary(self):
        self.stage.mkdir()
        policy = {"version": 1, "interpreter": "/fixed/python", "interpreter_sha256": "a" * 64,
                  "interpreter_device": 1, "interpreter_inode": 2}
        helper_signature = {"identifier": delivery.UPDATE_HELPER_ID, "team_identifier": SIGNING["team_identifier"]}
        commands = []
        def native(argv, **kwargs):
            commands.append(argv)
            if argv[0] == "/usr/bin/swiftc":
                binary = Path(argv[argv.index("-o") + 1])
                binary.write_bytes(b"native helper fixture")
                binary.chmod(0o755)
            return SimpleNamespace(returncode=0, stdout=b"", stderr=b"")
        with patch.object(delivery, "_python_runtime_identity", return_value=policy), \
                patch.object(delivery, "native_signature", return_value=helper_signature), \
                patch.object(delivery.subprocess, "run", side_effect=native):
            evidence = delivery.stage_update_supervisor(self.stage, SIGNING)
            helper = self.stage / delivery.UPDATE_HELPER
            self.assertEqual(evidence["identifier"], delivery.UPDATE_HELPER_ID)
            self.assertEqual(json.loads((helper / "Contents/Resources/policy.json").read_text()), policy)
            self.assertTrue((helper / "Contents/Resources/entry.py").is_file())
            self.assertTrue((helper / "Contents/Resources/runtime/scripts/hex-test-build.py").is_file())
            self.assertTrue(any(argv[:2] == ["/usr/bin/codesign", "--verify"] for argv in commands))
            delivery.verify_update_supervisor(self.stage, {"update_supervisor": evidence, "signing": SIGNING})
            (helper / "Contents/MacOS/hex-update").write_bytes(b"tampered")
            with self.assertRaisesRegex(delivery.DeliveryError, "executable changed"):
                delivery.verify_update_supervisor(self.stage, {"update_supervisor": evidence, "signing": SIGNING})

    def test_runtime_verifies_new_candidate_native_source_not_old_runtime_source(self):
        self.stage.mkdir()
        native = delivery.discovery_builder()
        candidate_root = self.stage / delivery.UPDATE_HELPER / "Contents/Resources/runtime/crates/hex-plugin-scan/native"
        candidate_root.mkdir(parents=True)
        for name in native.SOURCES:
            shutil.copy2(delivery.ROOT / "crates/hex-plugin-scan/native" / name, candidate_root / name)
        changed = candidate_root / native.SOURCES[1]
        changed.write_bytes(changed.read_bytes() + b"\n// newer candidate\n")
        receipt = {"update_supervisor": {"native_sources": list(native.SOURCES), "native_sdk_pins": native.PINS}}
        digest, pins = delivery.candidate_native_provenance(self.stage, receipt)
        self.assertNotEqual(digest, native.source_digest())
        app = self.stage / "Contents/Helpers" / delivery.DISCOVERY_APP
        for path in (app / "Contents/MacOS/hex-vst3-factory-scan", app / "Contents/Resources/sdk-notices.json",
                     app / "Contents/_CodeSignature/CodeResources"):
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"fixture")
        info = app / "Contents/Info.plist"
        info.write_bytes(plistlib.dumps({"CFBundleExecutable": "hex-vst3-factory-scan",
                                        "CFBundleIdentifier": "org.portalsurfer.hex.vst3.discovery"}))
        packaged = self.stage / delivery.DISCOVERY_RECEIPT
        packaged.parent.mkdir(parents=True, exist_ok=True)
        data = {"version": 1, "platform": "darwin-arm64", "scope": "native-macos-factory-discovery",
                "sdk_pins": pins, "source_sha256": digest, "source_commit": "a" * 40,
                "source_dirty": False, "package_layout": "hex-app-discovery-v1",
                "factory_fixtures": [], "factory_scan_fault_helper": None,
                "bundle_files": native.inventory(app)}
        packaged.write_text(json.dumps(data))
        with patch.object(native, "macho"), patch.object(delivery, "discovery_builder", return_value=native):
            with self.assertRaisesRegex(ValueError, "stale bootstrap"):
                delivery.validate_discovery_input(packaged, "a" * 40, packaged=True)
            self.assertEqual(delivery.validate_discovery_input(packaged, "a" * 40, packaged=True,
                                                               provenance=(digest, pins)), data)
            (app / "Contents/Resources/sdk-notices.json").write_bytes(b"tampered")
            with self.assertRaisesRegex(ValueError, "changed bundle"):
                delivery.validate_discovery_input(packaged, "a" * 40, packaged=True,
                                                  provenance=(digest, pins))

    def test_prepare_two_candidates_while_installed_app_is_running(self):
        self.assert_production_build(build_only=False, prepare_only=True)

    def test_post_publication_receipt_failure_reports_installed_app(self):
        self.assert_production_build(build_only=False, prepare_only=True, followup_failure=True)

    def test_pending_store_requires_target_ownership_and_preserves_unknown_contents(self):
        pending = delivery.pending_candidate(self.app)
        self.assertEqual(pending, self.root / ".hex-pending-update/hex.app")
        self.assertEqual(delivery.pending_candidate(self.app), pending)
        foreign = pending.parent / "user-data"
        foreign.write_text("preserve")
        with self.assertRaisesRegex(delivery.DeliveryError, "unexpected"):
            delivery.pending_candidate(self.app)
        self.assertEqual(foreign.read_text(), "preserve")
        foreign.unlink()
        (pending.parent / "owner.json").write_text('{"schema":1,"target":"other"}')
        with self.assertRaisesRegex(delivery.DeliveryError, "not owned"):
            delivery.pending_candidate(self.app)

    def test_pending_preparation_refuses_unqualified_evidence(self):
        self.workspace.rmdir()
        source = self.root / "repo"
        (source / "scripts").mkdir(parents=True)
        (source / "scripts/hex-test-build.json").write_text(json.dumps({"app_path": "hex.app", "signing": SIGNING}))
        args = SimpleNamespace(build_only=True, prepare_only=True, dev_profile=False)
        with patch.object(delivery, "ROOT", source), patch.object(delivery.sys, "platform", "darwin"), \
                patch.dict(os.environ, {"HEX_DEV_APP_PATH": str(self.app)}):
            with self.assertRaisesRegex(delivery.DeliveryError, "full exact-source CI"):
                delivery.build(args)
        self.assertFalse((self.root / ".hex-pending-update").exists())

    def test_pending_release_recovery_still_refuses_running_installed_app(self):
        self.bundle(self.app, b"old")
        source = self.root / "repo"
        (source / "scripts").mkdir(parents=True)
        (source / "scripts/hex-test-build.json").write_text(json.dumps({"app_path": "hex.app", "signing": SIGNING}))
        args = SimpleNamespace(build_only=False, prepare_only=True, dev_profile=False,
                               adopt_release_target=True)
        def tool(argv, root=source):
            if argv[:2] == ["git", "rev-parse"]:
                return str(source / ".git")
            if argv[0] == "ps":
                return f"123 {self.app}/Contents/MacOS/hex"
            return ""
        with patch.object(delivery, "ROOT", source), patch.object(delivery.sys, "platform", "darwin"), \
                patch.dict(os.environ, {"HEX_DEV_APP_PATH": str(self.app)}), \
                patch.object(delivery, "run", side_effect=tool):
            with self.assertRaisesRegex(delivery.DeliveryError, "running"):
                delivery.build(args)
        self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"old")
        self.assertFalse((self.root / ".hex-pending-update").exists())

    def assert_production_build(self, *, build_only, drift=False, finder_metadata=False, rmdir_member=None,
                                prepare_only=False, followup_failure=False, validate_ci_receipt=False,
                                discovery=False):
        # Exercise real assembly/receipt/activation with native external tools
        # substituted. A missing Contents parent failed actual post-merge delivery.
        self.bundle(self.app, b"old")
        self.workspace.rmdir()
        sidecar = self.root / "hex-test-build-receipt.json"
        sidecar.write_bytes(b"old adjacent receipt")
        source = self.root / "repo"
        source.mkdir()
        for relative in ("scripts", "assets/fonts/ioskeley-mono", "third_party/gpui"):
            (source / relative).mkdir(parents=True)
        (source / "scripts/hex-test-build.json").write_text(json.dumps({"app_path":"artifacts/hex.app", "signing": SIGNING}))
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
        if not build_only:
            report.write_text('{"status":"passed"}')
        included = self.root / "included.json"
        included.write_text(json.dumps([{"pr":"https://github.com/PORTALSURFER/hex/pull/168",
                                         "head":candidate}]))
        checklist = self.root / "USER-TEST.md"
        checklist.write_text("1. launch stable app\n2. inspect source identity\n")
        runner = delivery.ci.LocalCiRunner(source)
        runner.summary["start_tree"] = {"commit":candidate, "dirty":False, "dirty_files":[],
                                        "content_sha256":"d" * 64}
        dependency_inputs = {"fixture": "guarded input identity"}
        if validate_ci_receipt:
            # Synthetic full-plan evidence exercises the real receipt consumer;
            # it is not a claim that these fixture steps ran product CI.
            tree = runner.summary["start_tree"]
            report.write_text(json.dumps({
                "schema_version": 2, "mode": "full", "status": "passed", "stale": False,
                "exit_code": 0, "commit": candidate, "dirty": False,
                "start_tree": tree, "end_tree": tree,
                "target_cache": {"dependency_inputs": dependency_inputs},
                "steps": [{"step_id": step.step_id, "argv": list(step.argv), "status": "passed",
                           "test_results": {"python_executions": [1]}}
                          for step in delivery.ci.FULL_STEPS],
            }))
        runner.summary["runtime_inputs"] = {"version": 1, "sha256": "d" * 64, "symlinks": []}
        runner.cargo_identity = {"target_directory":str(target.parent.parent)}
        runner.target_lock = None if rmdir_member else SimpleNamespace(release=Mock(return_value=None))
        runner.target_state_dir = self.root / "target-state"
        runner.git_identity = {"workspace_root":str(source), "git_common_dir":str(source / ".git"), "commit":candidate}
        scope = [{"name":"hex-desktop", "version":"0.1.0", "spec":"hex-desktop@0.1.0"}]
        runner.cargo_identity["workspace_packages"] = scope
        # The substituted metadata query must populate the same complete package
        # view as production before name-scoped cleanup can be proven safe.
        runner._release_metadata = {"workspace_members": [scope[0]["spec"]], "packages": [
            {"id": scope[0]["spec"], "name": "hex-desktop", "source": None}]}
        _, dev_owner = delivery.ci.target_state_paths(target.parent.parent, runner.target_state_dir)
        owner = dev_owner.with_name(dev_owner.name.replace(".owner.json", ".release-owner.json"))
        event = {"reason":"compiler-artifact", "executable":str(executable),
                 "target":{"name":"hex-desktop"}, "profile":{"opt_level":"3"}}
        def tool(argv, root=source):
            if argv[:2] == ["git", "rev-parse"]:
                return str(source / ".git")
            if argv[:2] == ["cargo", "metadata"]:
                return json.dumps({"packages":[{"name":"hex-desktop", "version":"0.1.0"}]})
            if prepare_only and argv[0] == "ps":
                return f"123 {self.app}/Contents/MacOS/hex"
            return ""
        args = SimpleNamespace(ci_report=None if build_only else report, candidate=candidate, build_only=build_only,
                               included=included, checklist=checklist, prepare_only=prepare_only,
                               vst3_discovery_receipt=None, vst3_sdk=self.root / "sdk",
                               without_vst3_discovery=not discovery)
        scanner = {"path": str(target / "hex-vst3-scan-macos")}
        native_input = self.root / "controlled-macos.json"
        discovery_evidence = {"source": candidate}
        def stage_discovery(app, native, exact_source, artifact, _signing):
            self.assertEqual((native, exact_source, artifact), (native_input, candidate, scanner))
            helper = app / delivery.DISCOVERY_ROOT / delivery.DISCOVERY_SCANNER
            helper.parent.mkdir(parents=True)
            helper.write_bytes(b"scanner fixture")
            return discovery_evidence

        @contextmanager
        def artifact_guard(*args, **kwargs):
            self.assertEqual(kwargs.get("discovery", False), discovery)
            if rmdir_member:
                # Real durable pending/ready publication and target lock, with
                # only compilation/native tools substituted in this fixture.
                with runner.guarded_release(adopt=True) as ownership:
                    argv = ["cargo", "build", "--release", "-p", "hex-desktop", "--locked"]
                    ownership.update(build_argv=argv, build_exit_code=0,
                                     artifact=runner.identify_release_artifact(executable, argv))
                    yield executable, ownership
                return
            try:
                if drift == "build":
                    runner.summary["start_tree"] = {**runner.summary["start_tree"], "content_sha256": "changed"}
                yield executable, {"build_argv":["cargo", "build", "--release"],
                    "profile":"release", "source":runner.summary["start_tree"], "packages":[],
                    "target_directory":str(target.parent.parent), "owner_path":"fixture-owner",
                    "lock_path":"fixture-lock", "action":"reuse", "cleanup_status":"not_needed",
                    "artifact":{"path":str(executable), "sha256":delivery.sha(executable)},
                    "discovery_scanner":scanner}
            finally:
                runner.target_lock.release()
        def capture(key):
            runner.summary[key] = dict(runner.summary["start_tree"])
            return True
        ci_check = (patch.object(delivery, "validate_ci", wraps=delivery.validate_ci) if validate_ci_receipt else
                    patch.object(delivery, "validate_ci", return_value={},
                                 side_effect=AssertionError("CI required for build-only") if build_only else None))
        with patch.object(delivery, "ROOT", source), \
                patch.object(delivery.sys, "platform", "darwin"), \
                patch.dict(os.environ, {"HEX_DEV_APP_PATH":str(self.app)}), \
                patch.object(delivery.ci, "LocalCiRunner", return_value=runner), \
                ci_check as checked_ci, \
                patch.object(runner, "run", side_effect=AssertionError("tests during packaging")), \
                patch.object(runner, "_capture_repository_state", side_effect=capture), \
                patch.multiple(runner, _resolve_cargo_identity=Mock(return_value=True),
                               _dependency_context=Mock(return_value=dependency_inputs)), \
                patch.object(runner, "_release_scope", return_value=scope), \
                patch.object(delivery.ci.build_inputs, "workspace_snapshot", return_value={"packages": [], "observed_ns": 1}), \
                patch.object(runner, "_path_dependency_inputs", return_value=[]), \
                patch.object(runner, "_check_disk_headroom", return_value=True), \
                patch.object(delivery.ci.ci_target_storage, "inspect", return_value={
                    "platform": "darwin", "status": "identified", "filesystem_type": "apfs"}), \
                patch.multiple(delivery, optimized_artifact=Mock(side_effect=artifact_guard),
                               stage_update_supervisor=Mock(return_value=None),
                               discovery_builder=Mock(return_value=Mock(build=Mock(return_value=native_input))),
                               validate_discovery_input=Mock(),
                               stage_discovery=Mock(side_effect=stage_discovery),
                               verify_discovery_package=Mock()), \
                patch.object(delivery, "native_signature", return_value=SIGNED), \
                patch.object(delivery, "run", side_effect=tool), \
                patch.object(delivery.subprocess, "run", return_value=SimpleNamespace(stdout=json.dumps(event), returncode=0)):
            if prepare_only:
                pending = self.root / ".hex-pending-update/hex.app"
                # The installed app's publication lock is independent of the
                # pending store and remains held throughout this preparation.
                with delivery.activation_lock(self.app):
                    first = delivery.build(args)
                self.assertEqual(first["app_path"], str(self.app))
                self.assertEqual(first["verification"], {"mode": "full-ci", "tests": "passed"})
                self.assertEqual(first["pending_target_identity"]["executable_sha256"],
                                 delivery.sha(self.app / "Contents/MacOS/hex"))
                self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"old")
                self.assertEqual((pending / "Contents/MacOS/hex").read_bytes(), b"optimized fixture")
                self.assertEqual(sidecar.read_bytes(), b"old adjacent receipt")
                executable.write_bytes(b"second optimized fixture")
                runner.output_dir = runner.output_dir.parent / "second-pending"
                second = delivery.build(args)
                self.assertNotEqual(first["input_executable_sha256"], second["input_executable_sha256"])
                self.assertEqual((pending / "Contents/MacOS/hex").read_bytes(), b"second optimized fixture")
                self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"old")
                runner.output_dir = runner.output_dir.parent / "failed-pending"
                with patch.object(delivery, "sign_bundle", side_effect=subprocess.CalledProcessError(1, ["codesign"])):
                    with self.assertRaises(subprocess.CalledProcessError):
                        delivery.build(args)
                self.assertEqual((pending / "Contents/MacOS/hex").read_bytes(), b"second optimized fixture")
                self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"old")
                runner.output_dir = runner.output_dir.parent / "changed-target"
                def change_target(_stage, _signing):
                    (self.app / "Contents/MacOS/hex").write_bytes(b"external replacement")
                with patch.object(delivery, "sign_bundle", side_effect=change_target):
                    with self.assertRaisesRegex(delivery.DeliveryError, "installed app changed"):
                        delivery.build(args)
                self.assertEqual((pending / "Contents/MacOS/hex").read_bytes(), b"second optimized fixture")
                self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"external replacement")
                selected = delivery.sha(pending / "Contents/Resources/test-build.json")
                with patch.object(delivery, "run", return_value=""):
                    with self.assertRaisesRegex(delivery.DeliveryError, "changed since preparation"):
                        delivery._apply_verified_pending(self.app, selected)
                    (self.app / "Contents/MacOS/hex").write_bytes(b"old")
                    with self.assertRaisesRegex(delivery.DeliveryError, "selection changed"):
                        delivery._apply_verified_pending(self.app, "0" * 64)
                    changed_signature = {**SIGNED, "designated_requirement": "different requirement"}
                    with patch.object(delivery, "native_signature", side_effect=[SIGNED, SIGNED, changed_signature]):
                        with self.assertRaisesRegex(delivery.DeliveryError, "signing requirement"):
                            delivery._apply_verified_pending(self.app, selected)
                    with delivery.activation_lock(self.app):
                        with self.assertRaisesRegex(delivery.DeliveryError, "another test-build"):
                            delivery._apply_verified_pending(self.app, selected)
                with self.assertRaisesRegex(delivery.DeliveryError, "running"):
                    delivery._apply_verified_pending(self.app, selected)
                self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"old")
                receipt_path = pending / "Contents/Resources/test-build.json"
                receipt_bytes = receipt_path.read_bytes()
                original_copy = shutil.copytree
                def change_selection_during_copy(source, destination, *args, **kwargs):
                    result = original_copy(source, destination, *args, **kwargs)
                    if source == pending:
                        receipt_path.write_bytes(b"superseded")
                    return result
                with patch.object(delivery, "run", return_value=""), \
                        patch.object(delivery.shutil, "copytree", side_effect=change_selection_during_copy):
                    with self.assertRaisesRegex(delivery.DeliveryError, "changed during publication"):
                        delivery._apply_verified_pending(self.app, selected)
                self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"old")
                receipt_path.write_bytes(receipt_bytes)
                original_verify = delivery.verify
                def fail_installed_verification(path):
                    if path == self.app and (path / "Contents/MacOS/hex").read_bytes() == b"second optimized fixture":
                        raise delivery.DeliveryError("injected installed verification failure")
                    return original_verify(path)
                with patch.object(delivery, "run", return_value=""), \
                        patch.object(delivery, "verify", side_effect=fail_installed_verification):
                    with self.assertRaisesRegex(delivery.DeliveryError, "injected installed"):
                        delivery._apply_verified_pending(self.app, selected)
                self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"old")
                if followup_failure:
                    original_write = delivery.write_json
                    def fail_sidecar(path, value):
                        if path == sidecar:
                            raise OSError("sidecar unavailable")
                        return original_write(path, value)
                    with patch.object(delivery, "run", return_value=""), patch.object(delivery, "write_json", side_effect=fail_sidecar):
                        with self.assertRaisesRegex(delivery.PublicationFollowupFailed, "update installed"):
                            delivery._apply_verified_pending(self.app, selected)
                    self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"second optimized fixture")
                    self.assertEqual(sidecar.read_bytes(), b"old adjacent receipt")
                    return
                with patch.object(delivery, "run", return_value=""):
                    installed = delivery._apply_verified_pending(self.app, selected)
                self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"second optimized fixture")
                self.assertEqual(installed["candidate"], candidate)
                self.assertFalse(delivery.install_workspace(self.app).exists())
                return
            if drift == "build":
                error = "full exact-source local CI evidence" if validate_ci_receipt else "source is dirty, changed"
                with patch.object(delivery, "activate") as activate:
                    with self.assertRaisesRegex(delivery.DeliveryError, error):
                        delivery.build(args)
                    activate.assert_not_called()
                self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"old")
                self.assertFalse(self.workspace.exists())
                runner.target_lock.release.assert_called_once()
                if validate_ci_receipt:
                    self.assertGreaterEqual(checked_ci.call_count, 3)
                return
            if drift == "signing":
                def change_source(*_args):
                    runner.summary["start_tree"] = {**runner.summary["start_tree"], "content_sha256": "changed"}
                with patch.object(delivery, "sign_bundle", side_effect=change_source), \
                        patch.object(delivery, "activate") as activate:
                    with self.assertRaisesRegex(delivery.DeliveryError, "source is dirty, changed"):
                        delivery.build(args)
                    activate.assert_not_called()
                self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"old")
                self.assertFalse(self.backup.exists())
                runner.target_lock.release.assert_called_once()
                return
            with patch.object(delivery, "sign_bundle", side_effect=subprocess.CalledProcessError(1, ["codesign"])), \
                    patch.object(delivery, "activate") as activate:
                with self.assertRaises(subprocess.CalledProcessError):
                    delivery.build(args)
                activate.assert_not_called()
                self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"old")
                self.assertFalse(self.backup.exists())
            # A real retry gets a fresh runner/log directory while preserving
            # the failed signing attempt's evidence.
            runner.output_dir = runner.output_dir.parent / "retry"
            remove = shutil.rmtree
            rmdir = Path.rmdir
            rmdir_calls = 0
            foreign = self.root / "user-project.hex"
            foreign.write_bytes(b"user project")
            member = self.workspace / ("unknown" if finder_metadata == "unknown" else ".DS_Store")
            late_member = self.workspace / ("unknown" if rmdir_member == "unknown" else ".DS_Store")
            def finder_at_rmdir(path):
                nonlocal rmdir_calls
                if (path == self.workspace and rmdir_member
                        and json.loads((self.app / "Contents/Resources/test-build.json").read_text()).get("source") == candidate):
                    rmdir_calls += 1
                    if rmdir_calls == 1 or rmdir_member == "repeated":
                        self.assertFalse(self.backup.exists())
                        self.assertFalse(self.stage.exists())
                        delivery.verify(self.app)
                        self.assertEqual(sidecar.read_bytes(), b"old adjacent receipt")
                        if rmdir_member == "symlink":
                            late_member.symlink_to(foreign)
                        else:
                            late_member.write_bytes(b"finder or unknown late file")
                return rmdir(path)
            def finder_after_custody_deletion(path):
                remove(path)
                if path == self.backup:
                    # Actual PhaseB cutpoint: new app is verified and old custody
                    # is gone, but activate has not removed the workspace or
                    # returned to build's adjacent-receipt publication.
                    self.assertFalse(self.stage.exists())
                    delivery.verify(self.app)
                    self.assertEqual(sidecar.read_bytes(), b"old adjacent receipt")
                    if finder_metadata == "symlink":
                        member.symlink_to(foreign)
                    else:
                        member.write_bytes(b"finder metadata")
            with patch.object(delivery.shutil, "rmtree", side_effect=finder_after_custody_deletion if finder_metadata else remove), \
                    patch.object(Path, "rmdir", finder_at_rmdir):
                if rmdir_member in ("unknown", "symlink", "repeated"):
                    error = OSError if rmdir_member == "repeated" else delivery.DeliveryError
                    with self.assertRaises(error) as failure:
                        delivery.build(args)
                    if rmdir_member == "repeated":
                        self.assertEqual(failure.exception.errno, errno.ENOTEMPTY)
                    else:
                        self.assertIn("unexpected temporary installation contents", str(failure.exception))
                    self.assertTrue(os.path.lexists(late_member))
                    self.assertEqual(foreign.read_bytes(), b"user project")
                    self.assertEqual(delivery.verify(self.app)["source"], candidate)
                    self.assertEqual(sidecar.read_bytes(), b"old adjacent receipt")
                    self.assertEqual(rmdir_calls, 2 if rmdir_member == "repeated" else 1)
                    self.assertEqual(json.loads(owner.read_text())["state"], "pending")
                    self.assertIsNone(runner.target_lock)
                    return
                if finder_metadata in ("unknown", "symlink"):
                    with self.assertRaisesRegex(delivery.DeliveryError, "unexpected"):
                        delivery.build(args)
                    self.assertTrue(os.path.lexists(member))
                    self.assertFalse(self.backup.exists())
                    self.assertEqual(delivery.verify(self.app)["source"], candidate)
                    self.assertEqual(sidecar.read_bytes(), b"old adjacent receipt")
                    with self.assertRaisesRegex(delivery.DeliveryError, "unexpected"):
                        delivery.recover(self.app)
                else:
                    receipt = delivery.build(args)
                    self.assertEqual(receipt["diagnostic_capabilities"], ["actions-v1", "audio-v1"])
                    if rmdir_member:
                        self.assertEqual(rmdir_calls, 2)
            if finder_metadata in ("unknown", "symlink"):
                self.assertEqual(foreign.read_bytes(), b"user project")
                # Simulate explicit inspection resolving the TEST's foreign entry,
                # then retry the ordinary supported build, not a sidecar write.
                member.unlink()
                (self.workspace / ".DS_Store").write_bytes(b"finder metadata")
                runner.output_dir = runner.output_dir.parent / "publication-retry"
                receipt = delivery.build(args)
                self.assertEqual(foreign.read_bytes(), b"user project")
            verified = delivery.verify(self.app)
            if validate_ci_receipt:
                self.assertGreaterEqual(checked_ci.call_count, 3)
                self.assertEqual(json.loads((self.app / "Contents/Resources/local-ci-summary.json").read_text())["start_tree"],
                                 tree)
            wrong = {**verified, "signing": {**SIGNING, "certificate_sha1": "0" * 40}}
            delivery.write_json(self.app / "Contents/Resources/test-build.json", wrong)
            with self.assertRaisesRegex(delivery.DeliveryError, "configured certificate"):
                delivery.verify(self.app)
            delivery.write_json(self.app / "Contents/Resources/test-build.json", verified)
        self.assertEqual(verified["source"], candidate)
        self.assertEqual(verified["profile"], "release")
        if discovery:
            self.assertEqual(verified["vst3_discovery"], discovery_evidence)
            self.assertEqual((self.app / delivery.DISCOVERY_ROOT / delivery.DISCOVERY_SCANNER).read_bytes(),
                             b"scanner fixture")
        self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"optimized fixture")
        self.assertEqual((self.app / "Contents/Resources/USER-TEST.md").read_text(), checklist.read_text())
        if build_only:
            self.assertIsNone(receipt["ci_report"])
            self.assertIsNone(receipt["ci_report_sha256"])
            self.assertEqual(receipt["verification"], {"mode": "source-only", "tests": "not_run"})
            self.assertFalse((self.app / "Contents/Resources/local-ci-summary.json").exists())
            source_report = json.loads((self.app / "Contents/Resources/source-verification.json").read_text())
            self.assertEqual(source_report["source"], runner.summary["start_tree"])
            self.assertEqual(source_report["tests"], "not_run")
            self.assertIn("Contents/Resources/source-verification.json", verified["files"])
        else:
            self.assertEqual((self.app / "Contents/Resources/local-ci-summary.json").read_bytes(), report.read_bytes())
            self.assertEqual(receipt["verification"], {"mode": "full-ci", "tests": "passed"})
            self.assertIn("Contents/Resources/local-ci-summary.json", verified["files"])
        self.assertEqual((self.app / "Contents/Resources/licenses/ioskeley-mono-OFL.txt").read_text(), "font license")
        self.assertNotIn("rollback_path", receipt)
        self.assertFalse(self.workspace.exists())
        self.assertEqual(list(self.root.glob(".hex-rollback-*.app")), [])
        self.assertEqual(json.loads((self.root / "hex-test-build-receipt.json").read_text()), receipt)
        self.assertEqual(receipt["schema"], 2)
        self.assertNotIn("Contents/MacOS/hex", verified["files"])
        self.assertIn("Contents/MacOS/hex", receipt["artifact_files"])
        self.assertIn("Contents/Resources/test-build.json", receipt["artifact_files"])
        if rmdir_member:
            ownership = json.loads(owner.read_text())
            self.assertEqual(ownership["state"], "ready")
            self.assertEqual(ownership["source"], runner.summary["start_tree"])
            self.assertEqual(ownership["artifact"]["sha256"], delivery.sha(executable))
            self.assertIsNone(runner.target_lock)
        else:
            self.assertEqual(runner.target_lock.release.call_count, 3 if finder_metadata in ("unknown", "symlink") else 2)

    def test_build_only_source_gate_rejects_dirty_wrong_or_changed_candidate(self):
        runner = delivery.ci.LocalCiRunner(self.root)
        tree = {"commit": "a" * 40, "dirty": False, "content_sha256": "digest"}
        runner.summary["start_tree"] = tree
        with patch.object(runner, "_capture_repository_state", return_value=True):
            expected = delivery.validate_source(runner, tree["commit"])
            for changed in ({**tree, "dirty": True}, {**tree, "commit": "b" * 40},
                            {**tree, "content_sha256": "changed"}):
                with self.subTest(tree=changed):
                    runner.summary["start_tree"] = changed
                    with self.assertRaises(delivery.DeliveryError):
                        delivery.validate_source(runner, tree["commit"], expected)

    def test_ci_gate_rejects_shortcuts_stale_and_changed_source(self):
        runner = delivery.ci.LocalCiRunner(self.root, steps=(delivery.ci.Step("fake", ("true",)),))
        tree = {"commit": "a" * 40, "dirty": False, "dirty_files": [], "content_sha256": "digest"}
        runner.summary["start_tree"] = tree
        evidence = {"schema_version": 2, "mode": "full", "status": "passed", "stale": False, "exit_code": 0,
                    "commit": tree["commit"], "dirty": False, "start_tree": tree, "end_tree": tree,
                    "steps": [{"step_id": step.step_id, "argv": list(step.argv), "status": "passed",
                               "test_results": {"rust_passed_by_binary": [1]}}
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

    def test_scoped_ci_requires_explicit_dev_and_recomputed_plan_with_positive_tests(self):
        # Fabricated report data tests consumer refusal, not product execution.
        runner = delivery.ci.LocalCiRunner(self.root)
        tree = {"commit": "a" * 40, "dirty": False, "content_sha256": "fixture"}
        runner.summary["start_tree"] = tree
        plan = {"scope": "tooling", "profile": "merge", "qualifies_merge": True,
                "source": tree["commit"], "base": "b" * 40}
        required = delivery.ci.planned_steps(plan)
        evidence = {"schema_version": 2, "mode": "merge", "status": "passed", "stale": False,
                    "exit_code": 0, "commit": tree["commit"], "dirty": False,
                    "start_tree": tree, "end_tree": tree, "plan": plan,
                    "steps": [{"step_id": s.step_id, "argv": list(s.argv), "status": "passed",
                               "test_results": {"python_executions": [1]}} for s in required]}
        report = self.root / "scoped.json"
        with patch.object(runner, "_capture_repository_state", return_value=True), \
                patch.object(delivery, "run", return_value="{}"), \
                patch.object(delivery.ci.ci_plan, "resolve", return_value=plan) as resolve:
            delivery.write_json(report, evidence)
            with self.assertRaisesRegex(delivery.DeliveryError, "full audit"):
                delivery.validate_ci(report, runner, tree["commit"])
            delivery.validate_ci(report, runner, tree["commit"], allow_dev_profile=True)
            resolve.return_value = {**plan, "scope": "full"}
            with self.assertRaisesRegex(delivery.DeliveryError, "differs"):
                delivery.validate_ci(report, runner, tree["commit"], allow_dev_profile=True)
            resolve.return_value = plan
            next(s for s in evidence["steps"] if s["step_id"] == "runner-tests")["test_results"] = {
                "python_executions": [2], "python_skipped": [2]}
            delivery.write_json(report, evidence)
            with self.assertRaisesRegex(delivery.DeliveryError, "no executed"):
                delivery.validate_ci(report, runner, tree["commit"], allow_dev_profile=True)


    def test_core_shared_python_refuses_native_before_pending_but_dev_and_full_work(self):
        # Synthetic passing reports exercise the consumer, not product tests.
        # The existing fake-tool fixture runs the actual guarded release boundary.
        import test_local_ci as fixtures
        fixture = fixtures.LocalCiRunnerTests()
        fixture.setUp()
        self.addCleanup(fixture.tearDown)
        runner = fixture.runner("core-python-consumer")
        self.assertTrue(runner._capture_repository_state("start_tree"))
        tree = runner.summary["start_tree"]
        impact = delivery.ci.ci_plan.python_tests
        selected = impact.select("merge", ["crates/hex-engine/src/lib.rs"],
            [{"name": "hex-engine", "manifest": "crates/hex-engine/Cargo.toml"}],
            delivery.ci.ci_plan.UI_PACKAGES, sorted(impact.REVIEWED_MODULES), env={})
        plan = {"scope": "full", "profile": "merge", "qualifies_merge": True,
                "source": tree["commit"], "base": "base", "python_tests": selected}
        dependencies = {"fixture": "same guarded inputs"}
        def evidence_for(chosen):
            return {"schema_version": 2, "mode": chosen["profile"], "status": "passed",
                    "stale": False, "exit_code": 0, "commit": tree["commit"], "dirty": False,
                    "start_tree": tree, "end_tree": tree, "plan": chosen,
                    "target_cache": {"dependency_inputs": dependencies},
                    "steps": [{"step_id": step.step_id, "argv": list(step.argv), "status": "passed",
                               "test_results": {"python_executions": [1]}}
                              for step in delivery.ci.planned_steps(chosen)]}
        report = fixture.repo / "core-ci.json"
        delivery.write_json(report, evidence_for(plan))
        with patch.object(delivery, "run", return_value="{}"), \
                patch.object(delivery.ci.ci_plan, "resolve", return_value=plan) as resolve, \
                patch.object(runner, "_dependency_context", return_value=dependencies):
            def verify():
                delivery.validate_ci(report, runner, tree["commit"])
            with self.assertRaisesRegex(delivery.DeliveryError, "full audit"):
                with runner.guarded_release(verification_check=verify):
                    self.fail("partial Python coverage reached release pending/build boundary")
            self.assertFalse(list(fixture.target_state_dir.glob("*.release-owner.json")))
            self.assertIsNone(runner.target_lock)
            calls = fixture.call_log.read_text()
            self.assertNotIn("cargo build", calls)
            self.assertNotIn("cargo clean", calls)
            delivery.validate_ci(report, runner, tree["commit"], allow_dev_profile=True)
            complete = {**plan, "profile": "release", "python_tests": impact.select(
                "release", [], [], set(), sorted(impact.REVIEWED_MODULES))}
            resolve.return_value = complete
            delivery.write_json(report, evidence_for(complete))
            delivery.validate_ci(report, runner, tree["commit"])


@unittest.skipUnless(sys.platform == "darwin", "native macOS signature behavior")
class NativeSignatureTests(unittest.TestCase):
    def test_developer_id_seals_receipt_and_keeps_requirement_across_changed_executables(self):
        # Opt in only on a configured developer machine; never select another
        # identity or substitute ad-hoc signing on ordinary test machines.
        if os.environ.get("HEX_TEST_SIGNING_IDENTITY") != SIGNING["certificate_sha1"]:
            self.skipTest("set HEX_TEST_SIGNING_IDENTITY to the configured certificate for real keychain signing")
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            requirements = []
            executable_hashes = []
            for index in (0, 1):
                app = root / str(index) / "hex.app"
                binary = app / "Contents/MacOS/hex"
                binary.parent.mkdir(parents=True)
                resources = app / "Contents/Resources"
                resources.mkdir()
                source = root / "main.c"
                source.write_text(f"int main(void) {{ return {index}; }}\n")
                subprocess.run(["clang", str(source), "-o", str(binary)], check=True, capture_output=True)
                with (app / "Contents/Info.plist").open("wb") as output:
                    plistlib.dump({"CFBundleIdentifier": delivery.IDENTITY, "CFBundleExecutable": "hex"}, output)
                resource = resources / ("source-verification.json" if index else "local-ci-summary.json")
                delivery.write_json(resource, {"mode": "source-only", "tests": "not_run"} if index else {"status": "passed"})
                receipt_path = resources / "test-build.json"
                receipt = {"schema": 2, "signing": SIGNING,
                           "verification": {"mode": "source-only" if index else "full-ci",
                                            "tests": "not_run" if index else "passed"},
                           "files": delivery.inventory(app, signed_payload=True)}
                delivery.write_json(receipt_path, receipt)
                delivery.sign_bundle(app, SIGNING)
                delivery.verify(app)
                metadata = delivery.native_signature(binary)
                requirements.append(metadata["designated_requirement"])
                executable_hashes.append(delivery.sha(binary))
                for wrong in ({**SIGNING, "certificate_sha1": "0" * 40},
                              {**SIGNING, "team_identifier": "OTHERTEAM0"}):
                    with self.subTest(wrong=wrong), self.assertRaises(subprocess.CalledProcessError):
                        delivery.verify_signed_bundle(app, wrong, metadata)
                # Resource hash/receipt rewriting cannot bypass the native seal.
                resource.write_text("tampered")
                receipt["files"] = delivery.inventory(app, signed_payload=True)
                delivery.write_json(receipt_path, receipt)
                with self.assertRaises(subprocess.CalledProcessError):
                    delivery.verify(app)
                # Nor can removing the signed-schema declaration downgrade it.
                delivery.write_json(receipt_path, {"schema": 1, "files": delivery.inventory(app)})
                with self.assertRaisesRegex(delivery.DeliveryError, "schema-2"):
                    delivery.verify(app)
            self.assertNotEqual(*executable_hashes)
            self.assertEqual(*requirements)

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


class ReleaseCargoTests(unittest.TestCase):
    __hex_shard_early__ = True

    def test_real_release_a_b_a_api_outputs_dependencies_and_scope_refusal(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            root = base / "a"
            external = base / "external"
            for directory in (root / "api/src", root / "app/src", external / "src"):
                directory.mkdir(parents=True)
            (external / "Cargo.toml").write_text('[package]\nname="external-helper"\nversion="0.1.0"\nedition="2021"\n')
            (external / "src/lib.rs").write_text('pub fn suffix() -> u32 { 9 }\n')
            (root / "Cargo.toml").write_text('[workspace]\nmembers=["api", "app"]\nresolver="2"\n')
            (root / ".gitignore").write_text('/artifacts/\n')
            (root / "api/Cargo.toml").write_text('[package]\nname="fixture-api"\nversion="0.1.0"\nedition="2021"\n')
            (root / "api/src/lib.rs").write_text('pub fn value() -> u32 { 7 }\n')
            manifest = ('[package]\nname="fixture-app"\nversion="0.1.0"\nedition="2021"\n'
                        '[dependencies]\nfixture-api={path="../api"}\n'
                        f'external-helper={{path={json.dumps(str(external))}}}\n')
            (root / "app/Cargo.toml").write_text(manifest)
            (root / "app/src/main.rs").write_text('fn main() { println!("{}", fixture_api::value() + external_helper::suffix()); }\n')
            def command(argv, cwd=root, env=None):
                # Only the newly built, disposable fixture executable is bounded
                # here. A stall is a failure, never a retry or a passing result.
                executable = len(argv) == 1 and Path(argv[0]).is_absolute()
                try:
                    result = subprocess.run(argv, cwd=cwd, env=env, text=True, capture_output=True,
                                            timeout=120 if executable else None)
                except subprocess.TimeoutExpired:
                    self.fail(f"fixture executable did not finish within 120 seconds: {argv[0]}")
                self.assertEqual(result.returncode, 0, result.stderr)
                return result.stdout.strip()
            command(["git", "init", "-q"])
            command(["git", "config", "user.name", "release fixture"])
            command(["git", "config", "user.email", "fixture@example.invalid"])
            command(["cargo", "generate-lockfile", "--offline"])
            command(["git", "add", "."])
            command(["git", "commit", "-qm", "A numeric API"])
            other = base / "b"
            command(["git", "worktree", "add", "-qb", "b", str(other)])
            (other / "api/src/lib.rs").write_text('pub fn value() -> &\'static str { "B string API" }\n')
            (other / "app/src/main.rs").write_text('fn main() { println!("{} {}", fixture_api::value(), external_helper::suffix()); }\n')
            command(["git", "add", "."], other)
            command(["git", "commit", "-qm", "B incompatible API"], other)
            target = base / "target"
            state = base / "ownership"
            env = {**os.environ, "CARGO_TARGET_DIR":str(target), "CARGO_INCREMENTAL":"0"}
            def runner(cwd, name, selected_env=env):
                return delivery.ci.LocalCiRunner(cwd, run_id=name, env=selected_env,
                    min_free_bytes=0, target_state_dir=state)
            def build(cwd, name, expected, selected_env=env, *, check_contender=False, retain=False):
                instance = runner(cwd, name, selected_env)
                checks = []
                def verify():
                    self.assertIsNotNone(instance.target_lock)
                    checks.append("artifact" if instance.summary.get("release_target_cache", {}).get("artifact") else "before-build")
                with delivery.optimized_artifact(instance, package="fixture-app", verification_check=verify) as (binary, ownership):
                    self.assertEqual(command([str(binary)]), expected)
                    self.assertEqual(delivery.sha(binary), ownership["artifact"]["sha256"])
                    self.assertIsNotNone(instance.target_lock)
                    if check_contender:
                        with self.assertRaisesRegex(delivery.ci.TargetCacheError, "already in use"):
                            with runner(cwd, name + "-contender").guarded_release():
                                self.fail("second owner entered")
                    if retain:
                        shutil.copy2(binary, base / (name + "-retained-binary"))
                self.assertIsNone(instance.target_lock)
                self.assertEqual(ownership["state"], "ready")
                if ownership.get("cleanup_log"):
                    self.assertNotIn("qualifier", Path(ownership["cleanup_log"]).read_text())
                self.assertEqual(checks, ["before-build", "artifact"])
                return ownership
            first = build(root, "release-a", "16", check_contender=True, retain=True)
            owner_before = Path(first["owner_path"]).read_bytes()
            def refuse_certificate():
                raise delivery.DeliveryError("test certificate inputs changed")
            with self.assertRaisesRegex(delivery.DeliveryError, "certificate"):
                with delivery.optimized_artifact(runner(root, "ci-refused"), package="fixture-app",
                                                 verification_check=refuse_certificate):
                    self.fail("changed CI certificate reached compilation/consumer")
            self.assertEqual(Path(first["owner_path"]).read_bytes(), owner_before)
            dependencies = {p.name:delivery.sha(p) for p in (target / "release/deps").glob("libexternal_helper-*.rlib")}
            self.assertTrue(dependencies)
            # Keep a real dev build and ensure release handoff cannot touch it.
            command(["cargo", "build", "--offline", "--locked"], root, env)
            dev = {str(p.relative_to(target)):delivery.sha(p) for p in (target / "debug").rglob("*") if p.is_file()}
            self.assertTrue(dev)
            wrapper_dir = base / "bin"
            wrapper_dir.mkdir()
            cargo = shutil.which("cargo")
            self.assertIsNotNone(cargo)
            wrapper = wrapper_dir / "cargo"
            wrapper.write_text('#!/usr/bin/env python3\nimport os,glob,sys\n'
                'if sys.argv[1:2] == ["build"] and "--release" in sys.argv:\n'
                ' stale=glob.glob(os.path.join(os.environ["CARGO_TARGET_DIR"], "release/deps/libfixture_api-*.rlib"))\n'
                ' if stale:\n  print("workspace release artifact survived handoff",file=sys.stderr)\n  sys.exit(23)\n'
                f'os.execv({cargo!r},[{cargo!r},*sys.argv[1:]])\n')
            wrapper.chmod(0o755)
            guarded_env = {**env, "PATH":str(wrapper_dir) + os.pathsep + env["PATH"]}
            # Negative control: ordinary dev handoff leaves these release artifacts.
            dev_runner = runner(other, "dev-only", guarded_env)
            dev_runner.adopt_target = True
            dev_runner.output_dir.mkdir(parents=True)
            self.assertTrue(dev_runner._capture_repository_state("start_tree"))
            self.assertTrue(dev_runner._resolve_cargo_identity())
            self.assertTrue(dev_runner._prepare_target_cache())
            dev_runner.target_lock.release()
            dev_runner.target_lock = None
            control = subprocess.run(["cargo", "build", "--release", "--locked"], cwd=other,
                                     env=guarded_env, capture_output=True, text=True)
            self.assertEqual(control.returncode, 23, control.stderr)
            command(["cargo", "build", "--offline", "--locked"], other, env)
            dev = {str(p.relative_to(target)):delivery.sha(p) for p in (target / "debug").rglob("*") if p.is_file()}
            second = build(other, "release-b", "B string API 9", guarded_env, retain=True)
            third = build(root, "release-a-again", "16", guarded_env)
            self.assertEqual(second["cleanup_status"], "passed")
            self.assertEqual(third["cleanup_status"], "passed")
            self.assertEqual({p.name:delivery.sha(p) for p in (target / "release/deps").glob("libexternal_helper-*.rlib")}, dependencies)
            self.assertEqual({str(p.relative_to(target)):delivery.sha(p) for p in (target / "debug").rglob("*") if p.is_file()}, dev)
            self.assertEqual(command([str(base / "release-a-retained-binary")]), "16")
            self.assertEqual(command([str(base / "release-b-retained-binary")]), "B string API 9")
            build(root, "release-a-normal-tool", "16")
            reused = build(root, "release-a-reuse", "16")
            self.assertEqual(reused["cleanup_status"], "not_needed")
            # Same checkout/source edit must invoke Cargo and change the actual
            # executable, retaining the known build cache rather than cleaning.
            (root / "api/src/lib.rs").write_text('pub fn value() -> u32 { 10 }\n')
            command(["git", "add", "."])
            command(["git", "commit", "-qm", "same checkout changed behavior"])
            incremental = build(root, "release-a-incremental", "19")
            self.assertEqual(incremental["cleanup_status"], "not_needed")
            self.assertNotEqual(incremental["source"], reused["source"])
            self.assertNotEqual(incremental["artifact"]["sha256"], reused["artifact"]["sha256"])
            # Same-size current source with the exact old mtime must never
            # reuse the old optimized executable. Normal edits above stay warm.
            api = root / "api/src/lib.rs"
            stamp = api.stat()
            api.write_text('pub fn value() -> u32 { 20 }\n')
            os.utime(api, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
            command(["git", "add", "."])
            command(["git", "commit", "-qm", "backdated workspace behavior"])
            restored = build(root, "release-a-restored-mtime", "29")
            self.assertEqual(restored["cleanup_status"], "passed")
            self.assertIn("fixture-api", " ".join(restored["cleanup_argv"]))
            self.assertNotEqual(restored["artifact"]["sha256"], incremental["artifact"]["sha256"])
            # External path dependencies need the same content-based release
            # protection, even when Cargo's timestamp observation is unchanged.
            # Keep workspace source fixed at the previous successful build.
            external_source = external / "src/lib.rs"
            stamp = external_source.stat()
            external_source.write_text('pub fn suffix() -> u32 { 8 }\n')
            os.utime(external_source, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
            external_changed = build(root, "release-a-external-restored", "28")
            self.assertIn("external-helper", " ".join(external_changed["cleanup_argv"]))
            dependencies = {p.name:delivery.sha(p) for p in (target / "release/deps").glob("libexternal_helper-*.rlib")}
            self.assertTrue(dependencies)
            configured_env = {**env, "RUSTFLAGS": "--cfg release_fixture_changed"}
            changed_config = build(root, "release-a-config", "28", configured_env)
            self.assertEqual(changed_config["cleanup_status"], "passed")
            configured_dependencies = {p.name:delivery.sha(p) for p in (target / "release/deps").glob("libexternal_helper-*.rlib")}
            self.assertEqual({name: configured_dependencies[name] for name in dependencies}, dependencies)
            original_main = (root / "app/src/main.rs").read_text()
            (root / "app/src/main.rs").write_text("invalid Rust source")
            command(["git", "add", "."])
            command(["git", "commit", "-qm", "actual failed build"])
            failed = runner(root, "release-a-failed", configured_env)
            with self.assertRaises(delivery.DeliveryError):
                with delivery.optimized_artifact(failed, package="fixture-app"):
                    self.fail("failed Cargo build yielded an artifact")
            owner = json.loads(Path(incremental["owner_path"]).read_text())
            self.assertEqual(owner["state"], "pending")
            (root / "app/src/main.rs").write_text(original_main)
            (root / "api/src/lib.rs").write_text('pub fn value() -> u32 { 7 }\n')
            external_source.write_text('pub fn suffix() -> u32 { 9 }\n')
            command(["git", "add", "."])
            command(["git", "commit", "-qm", "restore behavior"])
            with self.assertRaisesRegex(delivery.ci.TargetCacheError, "pending"):
                with delivery.optimized_artifact(runner(root, "pending-refused", configured_env), package="fixture-app"):
                    self.fail("pending state was silently reused")
            with delivery.optimized_artifact(runner(root, "pending-explicit"), package="fixture-app", adopt=True) as (binary, _):
                self.assertEqual(command([str(binary)]), "16")
            # Explicit recovery restores the original release inputs.
            dependencies = {p.name:delivery.sha(p) for p in (target / "release/deps").glob("libexternal_helper-*.rlib")}
            # Cargo cannot select the old removed/renamed member from this graph.
            (other / "api/Cargo.toml").write_text('[package]\nname="renamed-api"\nversion="0.1.0"\nedition="2021"\n')
            (other / "app/Cargo.toml").write_text(manifest.replace('fixture-api={path=', 'fixture-api={package="renamed-api",path='))
            command(["cargo", "generate-lockfile", "--offline"], other)
            command(["git", "add", "."], other)
            command(["git", "commit", "-qm", "rename member"], other)
            before = delivery.sha(target / "release/fixture-app")
            for name in ("renamed", "removed"):
                if name == "removed":
                    (other / "Cargo.toml").write_text('[workspace]\nmembers=["app"]\nexclude=["api"]\nresolver="2"\n')
                    command(["git", "add", "."], other)
                    command(["git", "commit", "-qm", "remove workspace member"], other)
                with self.assertRaisesRegex(delivery.ci.TargetCacheError, "removed, renamed"):
                    with delivery.optimized_artifact(runner(other, name), package="fixture-app"):
                        self.fail("unsupported package handoff built")
                self.assertEqual(delivery.sha(target / "release/fixture-app"), before)
                self.assertEqual(json.loads(Path(first["owner_path"]).read_text())["state"], "ready")
            # Pure additions retain every old package, so Cargo can clean the
            # complete current workspace and compile the newly added dependency.
            (root / "extra/src").mkdir(parents=True)
            extra_manifest = root / "extra/Cargo.toml"
            extra_manifest.write_text('[package]\nname="fixture-extra"\nversion="0.1.0"\nedition="2021"\n')
            (root / "extra/src/lib.rs").write_text('pub fn bonus() -> u32 { 1 }\n')
            (root / "Cargo.toml").write_text('[workspace]\nmembers=["api", "app", "extra"]\nresolver="2"\n')
            (root / "app/Cargo.toml").write_text(manifest + 'fixture-extra={path="../extra"}\n')
            (root / "app/src/main.rs").write_text(
                'fn main() { println!("{}", fixture_api::value() + external_helper::suffix() + fixture_extra::bonus()); }\n')
            command(["cargo", "generate-lockfile", "--offline"])
            command(["git", "add", "."])
            command(["git", "commit", "-qm", "add workspace dependency"])
            addition = build(root, "release-addition", "17")
            self.assertEqual(addition["cleanup_status"], "passed")
            self.assertTrue(any("fixture-extra" in argument for argument in addition["cleanup_argv"]))
            self.assertEqual({p.name:delivery.sha(p) for p in (target / "release/deps").glob("libexternal_helper-*.rlib")}, dependencies)
            self.assertEqual({str(p.relative_to(target)):delivery.sha(p) for p in (target / "debug").rglob("*") if p.is_file()}, dev)
            owner = Path(addition["owner_path"])
            owner_before = owner.read_bytes()
            binary_before = delivery.sha(target / "release/fixture-app")
            extra_manifest.write_text(extra_manifest.read_text().replace('version="0.1.0"', 'version="0.2.0"'))
            command(["cargo", "generate-lockfile", "--offline"])
            command(["git", "add", "."])
            command(["git", "commit", "-qm", "change existing member version"])
            with self.assertRaisesRegex(delivery.ci.TargetCacheError, "removed, renamed or version-changed"):
                with delivery.optimized_artifact(runner(root, "version-changed"), package="fixture-app"):
                    self.fail("version-changed handoff built")
            self.assertEqual(delivery.sha(target / "release/fixture-app"), binary_before)
            self.assertEqual(owner.read_bytes(), owner_before)
            print("real release A -> B -> A: APIs, additive dependency output, cache preservation, and removed/renamed/version refusal verified", flush=True)



@unittest.skipUnless(os.environ.get("HEX_VST3_DISCOVERY_RECEIPT") and os.environ.get("HEX_VST3_PACKAGE_SCANNER")
                     and os.environ.get("HEX_VST3_PACKAGE_FIXTURES"), "explicit native package qualification inputs required")
class NativeDiscoveryPackageTests(unittest.TestCase):
    """Real nested signing/lookup/protocol fixtures; never starts the Hex UI."""
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="hex-discovery-package-")
        self.addCleanup(self.temporary.cleanup)
        self.app = Path(self.temporary.name).resolve() / "hex.app"
        self.contents = self.app / "Contents"
        (self.contents / "MacOS").mkdir(parents=True)
        (self.contents / "Resources").mkdir()
        self.input = Path(os.environ["HEX_VST3_DISCOVERY_RECEIPT"]).resolve()
        self.scanner = Path(os.environ["HEX_VST3_PACKAGE_SCANNER"]).resolve()
        self.fixtures = Path(os.environ["HEX_VST3_PACKAGE_FIXTURES"]).resolve()
        self.candidate = json.loads(self.input.read_text())["source_commit"]
        self.signing = delivery.signing_configuration(json.loads((delivery.ROOT / "scripts/hex-test-build.json").read_text()))
        # Original isolated package fixture, not a playable desktop/user-test app.
        shutil.copy2(self.scanner, self.contents / "MacOS/hex")
        (self.contents / "Info.plist").write_bytes(plistlib.dumps({
            "CFBundleExecutable": "hex", "CFBundleIdentifier": delivery.IDENTITY, "CFBundlePackageType": "APPL"}))
        evidence = delivery.stage_discovery(self.app, self.input, self.candidate,
                                           {"path": str(self.scanner), "sha256": delivery.sha(self.scanner)}, self.signing)
        self.receipt = {"schema": 2, "profile": "release", "source": self.candidate,
                        "signing": self.signing, "vst3_discovery": evidence}
        self.seal()

    def seal(self):
        self.receipt["files"] = delivery.inventory(self.app, signed_payload=True)
        delivery.write_json(self.contents / "Resources/test-build.json", self.receipt)
        delivery.sign_bundle(self.app, self.signing)
        delivery.verify(self.app)

    def scan(self, name="normal"):
        executable = self.app / delivery.DISCOVERY_ROOT / delivery.DISCOVERY_SCANNER
        module = self.fixtures / "hex-vst3-host.app/Contents/Resources/factory-fixtures" / (name + ".vst3")
        process = subprocess.Popen([str(executable), "--bundled", "--trust-user-code", str(module), "3000"],
                                   cwd="/", env={"PATH": "/no-checkout-or-tools"}, stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        # communicate cannot be retried with new input after its first call.
        # Initial stdin remains open so cancellation is available if needed.
        try:
            out, err = process.communicate(timeout=30)
        except subprocess.TimeoutExpired:
            process.send_signal(signal.SIGTERM)  # standalone scoped cancellation, never group kill
            try:
                process.communicate(timeout=30)
            except subprocess.TimeoutExpired:
                self.fail(f"owned scanner {process.pid} retained after cancellation; no forced termination")
            self.fail("packaged original fixture exceeded qualification deadline")
        return process.returncode, out, err

    def test_signed_package_scans_thin_and_universal_outside_checkout(self):
        for name in ("normal", "universal"):
            code, out, err = self.scan(name)
            self.assertEqual(code, 0, err.decode(errors="replace"))
            report = json.loads(out)
            self.assertIsNone(report["error"])
            self.assertTrue(report["cleanup"]["direct_reaped"])
            self.assertEqual(report["cleanup"]["direct_exit_code"], 0)
            self.assertEqual(len(report["discovery"]["classes"]), 2)
            self.assertNotIn(b"forbidden ordinary instance", report["stderr"].encode())

    def test_missing_changed_and_stale_package_refuse_before_selected_module_load(self):
        root = self.app / delivery.DISCOVERY_ROOT
        helper = root / "hex-vst3-host.app/Contents/MacOS/hex-vst3-factory-scan"
        original = helper.read_bytes()
        for change in ("missing", "changed"):
            if change == "missing":
                helper.unlink()
            else:
                helper.write_bytes(original + b"changed")
            code, out, err = self.scan("load-crash")
            self.assertEqual(code, 1)
            self.assertEqual(out, b"")
            self.assertIn(b"factory scan refused", err)
            helper.write_bytes(original)
            helper.chmod(0o755)
        receipt = self.app / delivery.DISCOVERY_RECEIPT
        data = json.loads(receipt.read_text())
        data["source_commit"] = "0" * 40
        delivery.write_json(receipt, data)
        self.receipt["vst3_discovery"]["receipt_sha256"] = delivery.sha(receipt)
        # Re-seal caller-owned fixture to reach the source-binding refusal.
        self.receipt["files"] = delivery.inventory(self.app, signed_payload=True)
        delivery.write_json(self.contents / "Resources/test-build.json", self.receipt)
        delivery.sign_bundle(self.app, self.signing)
        code, out, err = self.scan("load-crash")
        self.assertEqual((code, out), (1, b""))
        self.assertIn(b"stale native discovery package source", err)

    def test_resealed_real_wrong_architecture_helper_is_rejected_before_load(self):
        root = self.app / delivery.DISCOVERY_ROOT
        helper_app = root / "hex-vst3-host.app"
        helper = helper_app / "Contents/MacOS/hex-vst3-factory-scan"
        wrong = self.fixtures / "hex-vst3-host.app/Contents/Resources/factory-fixtures/wrong-architecture.vst3/Contents/MacOS/fixture"
        shutil.copy2(wrong, helper)
        delivery.sign_discovery(helper_app, "org.portalsurfer.hex.vst3.discovery", self.signing)
        receipt = self.app / delivery.DISCOVERY_RECEIPT
        data = json.loads(receipt.read_text())
        data["bundle_files"] = delivery.discovery_builder().inventory(helper_app)
        delivery.write_json(receipt, data)
        self.receipt["vst3_discovery"]["receipt_sha256"] = delivery.sha(receipt)
        self.receipt["files"] = delivery.inventory(self.app, signed_payload=True)
        delivery.write_json(self.contents / "Resources/test-build.json", self.receipt)
        delivery.sign_bundle(self.app, self.signing)
        code, out, err = self.scan("load-crash")
        self.assertEqual((code, out), (1, b""))
        self.assertIn(b"factory scan refused", err)

if __name__ == "__main__":
    unittest.main()
