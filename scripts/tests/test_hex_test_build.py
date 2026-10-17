"""Deterministic tests for exact-source delivery and interruption recovery."""
import importlib.util
import io
import json
import os
from pathlib import Path
import plistlib
import subprocess
import sys
import tempfile
import shutil
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

    def test_activation_is_serialized(self):
        with delivery.activation_lock(self.app):
            with self.assertRaisesRegex(delivery.DeliveryError, "another"):
                with delivery.activation_lock(self.app):
                    self.fail("second installer entered")

    def test_production_build_assembles_and_replaces_stable_app(self):
        # Exercise real assembly/receipt/activation with native external tools
        # substituted. A missing Contents parent failed actual post-merge delivery.
        self.bundle(self.app, b"old")
        self.workspace.rmdir()
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
        report.write_text('{"status":"passed"}')
        included = self.root / "included.json"
        included.write_text(json.dumps([{"pr":"https://github.com/PORTALSURFER/hex/pull/168",
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
        @contextmanager
        def artifact_guard(*args, **kwargs):
            try:
                yield executable, {"build_argv":["cargo", "build", "--release"],
                    "profile":"release", "source":runner.summary["start_tree"], "packages":[],
                    "target_directory":str(target.parent.parent), "owner_path":"fixture-owner",
                    "lock_path":"fixture-lock", "action":"reuse", "cleanup_status":"not_needed",
                    "artifact":{"path":str(executable), "sha256":delivery.sha(executable)}}
            finally:
                runner.target_lock.release()
        with patch.object(delivery, "ROOT", source), \
                patch.object(delivery.sys, "platform", "darwin"), \
                patch.dict(os.environ, {"HEX_DEV_APP_PATH":str(self.app)}), \
                patch.object(delivery.ci, "LocalCiRunner", return_value=runner), \
                patch.object(delivery, "validate_ci", return_value={}), \
                patch.object(runner, "_resolve_cargo_identity", return_value=True), \
                patch.object(runner, "_check_disk_headroom", return_value=True), \
                patch.object(delivery, "optimized_artifact", side_effect=artifact_guard), \
                patch.object(delivery, "native_signature", return_value=SIGNED), \
                patch.object(delivery, "run", side_effect=tool), \
                patch.object(delivery.subprocess, "run", return_value=SimpleNamespace(stdout=json.dumps(event))):
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
            receipt = delivery.build(args)
            verified = delivery.verify(self.app)
            wrong = {**verified, "signing": {**SIGNING, "certificate_sha1": "0" * 40}}
            delivery.write_json(self.app / "Contents/Resources/test-build.json", wrong)
            with self.assertRaisesRegex(delivery.DeliveryError, "configured certificate"):
                delivery.verify(self.app)
            delivery.write_json(self.app / "Contents/Resources/test-build.json", verified)
        self.assertEqual(verified["source"], candidate)
        self.assertEqual(verified["profile"], "release")
        self.assertEqual((self.app / "Contents/MacOS/hex").read_bytes(), b"optimized fixture")
        self.assertEqual((self.app / "Contents/Resources/USER-TEST.md").read_text(), checklist.read_text())
        self.assertEqual((self.app / "Contents/Resources/local-ci-summary.json").read_bytes(), report.read_bytes())
        self.assertEqual((self.app / "Contents/Resources/licenses/ioskeley-mono-OFL.txt").read_text(), "font license")
        self.assertNotIn("rollback_path", receipt)
        self.assertFalse(self.workspace.exists())
        self.assertEqual(list(self.root.glob(".hex-rollback-*.app")), [])
        self.assertEqual(json.loads((self.root / "hex-test-build-receipt.json").read_text()), receipt)
        self.assertEqual(receipt["schema"], 2)
        self.assertNotIn("Contents/MacOS/hex", verified["files"])
        self.assertIn("Contents/MacOS/hex", receipt["artifact_files"])
        self.assertIn("Contents/Resources/test-build.json", receipt["artifact_files"])
        self.assertEqual(runner.target_lock.release.call_count, 2)

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
                resource = resources / "fixture.txt"
                resource.write_text("original")
                receipt_path = resources / "test-build.json"
                receipt = {"schema": 2, "signing": SIGNING,
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
                result = subprocess.run(argv, cwd=cwd, env=env, text=True, capture_output=True)
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
            def build(cwd, name, expected, selected_env=env):
                instance = runner(cwd, name, selected_env)
                with delivery.optimized_artifact(instance, package="fixture-app") as (binary, ownership):
                    self.assertEqual(command([str(binary)]), expected)
                    self.assertEqual(delivery.sha(binary), ownership["artifact"]["sha256"])
                    self.assertIsNotNone(instance.target_lock)
                    with self.assertRaisesRegex(delivery.ci.TargetCacheError, "already in use"):
                        with runner(cwd, name + "-contender").guarded_release():
                            self.fail("second owner entered")
                    # Artifact-only consumption does not activate/install any app.
                    shutil.copy2(binary, base / (name + "-retained-binary"))
                self.assertIsNone(instance.target_lock)
                self.assertEqual(ownership["state"], "ready")
                return ownership
            first = build(root, "release-a", "16")
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
            second = build(other, "release-b", "B string API 9", guarded_env)
            third = build(root, "release-a-again", "16", guarded_env)
            self.assertEqual(second["cleanup_status"], "passed")
            self.assertEqual(third["cleanup_status"], "passed")
            self.assertEqual({p.name:delivery.sha(p) for p in (target / "release/deps").glob("libexternal_helper-*.rlib")}, dependencies)
            self.assertEqual({str(p.relative_to(target)):delivery.sha(p) for p in (target / "debug").rglob("*") if p.is_file()}, dev)
            self.assertEqual(command([str(base / "release-a-retained-binary")]), "16")
            self.assertEqual(command([str(base / "release-b-retained-binary")]), "B string API 9")
            build(root, "release-a-reuse", "16")
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
            addition = build(root, "release-addition", "17", guarded_env)
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


if __name__ == "__main__":
    unittest.main()
