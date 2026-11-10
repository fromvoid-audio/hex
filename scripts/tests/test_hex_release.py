"""Release channel/custody regressions; native gates remain separate acceptance."""
import importlib.util
import os
import py_compile
import shutil
import sys
import json
from pathlib import Path
import plistlib
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("release_fixture", Path(__file__).resolve().parents[1] / "hex-release.py")
release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release)


class DeliveryImportSourceTests(unittest.TestCase):
    def test_release_and_launch_import_current_delivery_source_with_stale_pyc(self):
        source_scripts = Path(__file__).resolve().parents[1]
        with tempfile.TemporaryDirectory() as temporary:
            scripts = Path(temporary).resolve() / "scripts"
            scripts.mkdir()
            for name in ("hex-release.py", "prepare-launch-discovery.py", "hex-test-build.py",
                         "local-ci.py", "ci_plan.py", "python_test_plan.py", "build_inputs.py",
                         "ci_checkpoint.py", "ci_target_storage.py", "release_notes.py",
                         "changelog.py", "manual.py"):
                shutil.copyfile(source_scripts / name, scripts / name)
            delivery_path = scripts / "hex-test-build.py"
            original = delivery_path.read_bytes()
            before = b'IDENTITY = "org.portalsurfer.hex.dev"'
            after = b'IDENTITY = "org.portalsurfer.hex.dex"'
            self.assertEqual(original.count(before), 1)
            self.assertEqual(len(before), len(after))
            py_compile.compile(str(delivery_path), doraise=True,
                               invalidation_mode=py_compile.PycInvalidationMode.TIMESTAMP)
            stamp = delivery_path.stat()
            delivery_path.write_bytes(original.replace(before, after, 1))
            os.utime(delivery_path, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))

            # Sensitivity control: standard timestamp-based loading really sees
            # the stale bytecode; runtime-only name avoids CI imports.
            control_spec = importlib.util.spec_from_file_location("hex_update_runtime", delivery_path)
            control = importlib.util.module_from_spec(control_spec)
            with patch.dict(sys.modules, {control_spec.name: control}):
                control_spec.loader.exec_module(control)
            self.assertEqual(control.IDENTITY, "org.portalsurfer.hex.dev")
            self.assertFalse(hasattr(control, "ci"))
            self.assertFalse(hasattr(control, "changelog"))

            for filename, name, delivery_name in (
                ("hex-release.py", "release_source_probe", "hex_release_delivery"),
                ("prepare-launch-discovery.py", "launch_source_probe", "hex_launch_delivery"),
            ):
                with self.subTest(filename=filename), patch.dict(sys.modules):
                    spec = importlib.util.spec_from_file_location(name, scripts / filename)
                    entry = importlib.util.module_from_spec(spec)
                    sys.modules[name] = entry
                    spec.loader.exec_module(entry)
                    self.assertEqual(entry.delivery.IDENTITY, "org.portalsurfer.hex.dex")
                    self.assertEqual(entry.delivery.__name__, delivery_name)
                    self.assertEqual(Path(entry.delivery.__file__), delivery_path)
                    self.assertEqual(entry.delivery.__spec__.origin, str(delivery_path))
                    self.assertTrue(hasattr(entry.delivery, "ci"))
                    self.assertTrue(hasattr(entry.delivery, "changelog"))


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.addCleanup(patch.stopall)
        patch.object(release, "ROOT", self.root).start()
        (self.root / "scripts").mkdir()
        config = Path(__file__).resolve().parents[1] / "hex-test-build.json"
        (self.root / "scripts/hex-test-build.json").write_bytes(config.read_bytes())
        (self.root / "scripts/hex-release.json").write_text('{"notary_profile":"fixture"}')
        (self.root / "Cargo.toml").write_text('[workspace.package]\nversion="0.1.0"\n')
        self.store = self.root / "artifacts/releases"
        self.snapshot = {"format": 1, "source": "a" * 40, "base": "b" * 40,
                         "covered_changes": ["a" * 40], "batches": []}
        patch.object(release, "source", return_value="a" * 40).start()
        patch.object(release, "git", side_effect=lambda *a: "c" * 40 if "HEAD^{tree}" in a else "").start()
        patch.object(release.notes, "prepare", return_value={"pending": 0}).start()
        patch.object(release.notes, "snapshot", side_effect=lambda *a, **k: self.snapshot).start()
        def documentation(root, destination, version, source):
            destination.mkdir()
            (destination / "hex-manual.html").write_text("<html>" + version + "</html>")
            (destination / "hex-manual.pdf").write_bytes(b"%PDF-1.7\n/BaseFont /AAAAAA+Ioskeley-Mono\n%%EOF\n")
            manifest = {"schema": 1, "version": version, "source": source,
                        "outputs": {name: release.delivery.sha(destination / name)
                                    for name in ("hex-manual.html", "hex-manual.pdf")}}
            (destination / "documentation.json").write_bytes(release.encode(manifest))
            return manifest
        patch.object(release.manual, "build", side_effect=documentation).start()

    def cut(self, channel="beta", **kwargs):
        return release.cut(SimpleNamespace(channel=channel, version="0.1.0", base=None,
                           max_changes=10, generation_timeout=300, checklist=None, included=None,
                           accept_source=kwargs.get("accept_source")), self.store)

    def test_beta_beta_stable_and_next_target_preserve_original_snapshots(self):
        first = self.cut()
        old = {str(p.relative_to(first)): p.read_bytes() for p in first.rglob("*") if p.is_file()}
        second = self.cut()
        self.assertEqual(release.read(first / "plan.json")["version"], "0.1.0-beta.1")
        self.assertEqual(release.read(second / "plan.json")["version"], "0.1.0-beta.2")
        stable = self.cut("stable", accept_source="a" * 40)
        self.assertEqual(release.read(stable / "plan.json")["version"], "0.1.0")
        self.assertEqual([release.read(p / "plan.json")["build"] for p in (first, second, stable)], [1, 2, 3])
        with self.assertRaisesRegex(release.Refusal, "already reserved"):
            self.cut("stable", accept_source="a" * 40)
        self.assertEqual({str(p.relative_to(first)): p.read_bytes() for p in first.rglob("*") if p.is_file()}, old)
        self.assertEqual(release.next_version("beta", "0.2.0", {"0.1.0-beta.9"}, "20261023"), "0.2.0-beta.1")

    def test_stable_without_exact_native_acceptance_cannot_reserve_version(self):
        with self.assertRaisesRegex(release.Refusal, "native acceptance"):
            self.cut("stable")
        self.assertFalse((self.store / "versions").exists())

    def test_channel_order_is_numeric_and_nightly_order_is_separate(self):
        self.assertEqual(release.next_version("beta", "0.1.0", {"0.1.0-beta.9", "0.1.0-beta.10"}, "20261023"), "0.1.0-beta.11")
        self.assertEqual(release.next_version("nightly", "0.1.0", {"0.1.0-beta.99", "0.1.0-nightly.20261022.99"}, "20261023"), "0.1.0-nightly.20261023.1")
        for version in ("../0.1.0", "0.1.0-beta.1", "01.2.3", "1.2", "1.2.3+build"):
            with self.subTest(version=version), self.assertRaises(release.Refusal):
                release.next_version("beta", version, set(), "20261023")

    def test_frozen_notes_source_and_checklist_mutation_refuse_before_build(self):
        candidate = self.cut()
        release.validate_plan(candidate)
        for name in ("notes.json", "snapshot.json", "RELEASE-NOTES.md", "TESTING.md"):
            before = (candidate / name).read_bytes()
            (candidate / name).write_bytes(before + b" ")
            with self.subTest(name=name), self.assertRaisesRegex(release.Refusal, "changed"):
                release.validate_plan(candidate)
            (candidate / name).write_bytes(before)
        with patch.object(release, "source", return_value="d" * 40), self.assertRaisesRegex(release.Refusal, "source changed"):
            release.validate_plan(candidate)
        plan = release.read(candidate / "plan.json")
        plan["channel"] = "stable"
        (candidate / "plan.json").write_bytes(release.encode(plan))
        with self.assertRaisesRegex(release.Refusal, "version reservation"):
            release.validate_plan(candidate)

    def test_release_environment_preserves_explicit_profile_and_binds_frozen_notes(self):
        candidate = self.cut()
        plan = release.read(candidate / "plan.json")
        with patch.dict(release.os.environ, {"CARGO_PROFILE_DEV_DEBUG": "2"}, clear=True):
            env = release.release_environment(candidate, plan)
        self.assertEqual(env["CARGO_PROFILE_DEV_DEBUG"], "2")
        self.assertEqual(env["CARGO_PROFILE_TEST_DEBUG"], "0")
        self.assertEqual(env["HEX_RELEASE_CHANGELOG"], str(candidate / "notes.json"))
        self.assertEqual(env["HEX_RELEASE_LABEL"], "0.1.0-beta.1")

    def test_generation_failure_leaves_previous_release_and_no_new_reservation(self):
        first = self.cut()
        before = (first / "plan.json").read_bytes()
        with patch.object(release.notes, "prepare", side_effect=ValueError("model unavailable")):
            with self.assertRaisesRegex(ValueError, "unavailable"):
                self.cut()
        self.assertEqual((first / "plan.json").read_bytes(), before)
        self.assertEqual(len(list((self.store / "versions").glob("*.json"))), 1)

    def test_only_inference_timeouts_retry_and_retries_are_bounded(self):
        failure = release.notes.Refusal("generation failed with retained diagnostics")
        failure.__cause__ = subprocess.TimeoutExpired("configured codex", 300)
        with patch.object(release.notes, "prepare", side_effect=[failure, {"complete": True}]) as prepare:
            self.assertEqual(release.prepare_notes(self.store, "a" * 40), {"complete": True})
            self.assertEqual(prepare.call_count, 2)
        with patch.object(release.notes, "prepare", side_effect=failure) as prepare:
            with self.assertRaises(release.notes.Refusal):
                release.prepare_notes(self.store, "a" * 40)
            self.assertEqual(prepare.call_count, 3)
        with patch.object(release.notes, "prepare", side_effect=release.notes.Refusal("invalid model output")) as prepare:
            with self.assertRaises(release.notes.Refusal):
                release.prepare_notes(self.store, "a" * 40)
            self.assertEqual(prepare.call_count, 1)

    def test_immutable_retry_and_concurrent_reservation_do_not_overwrite(self):
        path = self.root / "snapshot.json"
        release.immutable(path, b"original")
        release.immutable(path, b"original")
        with self.assertRaises(release.Refusal):
            release.immutable(path, b"changed")
        self.assertEqual(path.read_bytes(), b"original")
        with release.locked(self.store), self.assertRaisesRegex(release.Refusal, "active"):
            with release.locked(self.store):
                self.fail("concurrent writer admitted")

    def test_interrupted_reservation_recovers_original_version_before_next_cut(self):
        first = self.cut()
        (self.store / "versions/0.1.0-beta.1.json").unlink()
        second = self.cut()
        self.assertEqual(release.read(second / "plan.json")["version"], "0.1.0-beta.2")
        release.validate_plan(first)
        (self.store / "versions/0.1.0-beta.1.json").unlink()
        release.validate_plan(first)
        self.assertEqual(release.read(self.store / "versions/0.1.0-beta.1.json")["candidate"], str(first))

    def test_failed_notary_wait_resumes_existing_submission_without_reupload(self):
        candidate = self.cut()
        app = candidate / "hex.app"
        app.mkdir()
        plan = release.read(candidate / "plan.json")
        patch.object(release, "check_bundle").start()
        patch.object(release.delivery, "inventory", return_value={"Contents/MacOS/hex": "fixture"}).start()
        calls = []

        def command(argv, log):
            calls.append(argv)
            if "submit" in argv:
                return b'{"id":"same-submission"}'
            if "wait" in argv:
                raise release.Refusal("offline")
            return b""

        with patch.object(release, "command", side_effect=command):
            for _ in range(2):
                with self.assertRaisesRegex(release.Refusal, "offline"):
                    release.notarize(candidate, app, plan)
        self.assertEqual(sum("submit" in call for call in calls), 1)
        self.assertEqual(sum("wait" in call for call in calls), 2)
        self.assertFalse((candidate / "release.json").exists())

    def test_archive_corruption_refuses_before_extraction_or_install(self):
        archive = self.root / "hex.zip"
        archive.write_bytes(b"original")
        manifest = self.root / "release.json"
        manifest.write_bytes(release.encode({"schema": 1, "status": "ready", "archive": archive.name,
                                            "archive_sha256": release.delivery.sha(archive)}))
        archive.write_bytes(b"changed")
        with patch.object(release.subprocess, "run") as run:
            with self.assertRaisesRegex(release.Refusal, "hash mismatch"):
                release.install(manifest)
            run.assert_not_called()

    def test_out_of_order_completion_cannot_rewind_channel_and_retry_is_idempotent(self):
        first, second = self.cut(), self.cut()
        for candidate in (first, second):
            plan = release.read(candidate / "plan.json")
            (candidate / "release.json").write_bytes(release.encode({**plan, "status": "ready", "archive_sha256": "f" * 64}))
        release.publish_channel(second / "release.json")
        index = self.store / "channels/beta.json"
        before = index.read_bytes()
        release.publish_channel(first / "release.json")
        release.publish_channel(second / "release.json")
        self.assertEqual(index.read_bytes(), before)
        self.assertEqual(release.read(index)["version"], "0.1.0-beta.2")

    def test_explicit_target_adoption_reaches_guard_without_changing_candidate(self):
        for adopt in (False, True):
            candidate = self.cut()
            frozen = (candidate / "plan.json").read_bytes()
            report = candidate / "fixture-ci-summary.json"
            report.write_text("{}")
            runner = SimpleNamespace(summary_path=report, output_dir=candidate / "fixture-ci-output",
                                     run=lambda: 0)
            with patch.object(release, "preflight"), \
                    patch.object(release.ci, "LocalCiRunner", return_value=runner), \
                    patch.object(release.delivery, "validate_ci"), \
                    patch.object(release.delivery, "optimized_artifact",
                                 side_effect=release.Refusal("guard boundary")) as guarded:
                with self.assertRaisesRegex(release.Refusal, "guard boundary"):
                    release.package(candidate, adopt_release_target=adopt)
            self.assertEqual(guarded.call_args.kwargs["adopt"], adopt)
            self.assertTrue(callable(guarded.call_args.kwargs["verification_check"]))
            self.assertEqual((candidate / "plan.json").read_bytes(), frozen)
            self.assertFalse((candidate / "release.json").exists())

    def test_cli_requires_explicit_adoption_for_cut_and_resume(self):
        candidate = self.cut()
        for operation in ("beta", "resume"):
            for adopt in (False, True):
                argv = [operation] + ([str(candidate)] if operation == "resume" else [])
                if adopt:
                    argv.append("--adopt-release-target")
                with patch.object(release, "store_path", return_value=self.store), \
                        patch.object(release, "cut", return_value=candidate), \
                        patch.object(release, "package", return_value={}) as package:
                    self.assertEqual(release.main(argv), 0)
                package.assert_called_once_with(candidate, adopt_release_target=adopt)

    def test_partial_generated_stage_is_recoverable_but_redirected_stage_refuses(self):
        candidate = self.cut()
        stage = candidate / "bundle.pending.app"
        stage.mkdir()
        (stage / "partial-binary").write_bytes(b"partial generated bytes")
        release.discard_generated_stage(candidate, "bundle.pending.app")
        self.assertFalse(stage.exists())
        original = self.root / "project.hex"
        original.write_bytes(b"original user data")
        stage.symlink_to(original)
        with self.assertRaises(release.Refusal):
            release.discard_generated_stage(candidate, "bundle.pending.app")
        self.assertEqual(original.read_bytes(), b"original user data")


class DistributionReceiptTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        self.app = self.root / "hex.app"
        resources = self.app / "Contents/Resources"
        (resources / "manual").mkdir(parents=True)
        (self.app / "Contents/MacOS").mkdir()
        executable = self.app / "Contents/MacOS/hex"
        executable.write_bytes(b"fixture executable; native checks injected")
        executable.chmod(0o755)
        configuration = json.loads((Path(__file__).resolve().parents[1] / "hex-test-build.json").read_text())
        (self.root / "scripts").mkdir()
        (self.root / "scripts/hex-test-build.json").write_bytes(release.encode(configuration))
        self.receipt = {"schema": 1, "channel": "beta", "profile": "release", "build": 1,
                        "verification": {"mode": "full-ci", "tests": "passed"}, "source": "a" * 40,
                        "target": "0.1.0", "version": "0.1.0-beta.1", "signing": configuration["signing"]}
        for name, key in (("notes.json", "notes_sha256"), ("snapshot.json", "snapshot_sha256"),
                          ("TESTING.md", "checklist_sha256"), ("local-ci-summary.json", "ci_report_sha256")):
            (resources / name).write_bytes(b"fixture")
            self.receipt[key] = release.delivery.sha(resources / name)
        documents = {"schema": 1, "version": self.receipt["version"], "source": self.receipt["source"], "outputs": {}}
        for name in ("hex-manual.html", "hex-manual.pdf"):
            (resources / "manual" / name).write_bytes(b"fixture document")
            documents["outputs"][name] = release.delivery.sha(resources / "manual" / name)
        (resources / "manual/documentation.json").write_bytes(release.encode(documents))
        self.receipt["documentation_sha256"] = release.delivery.sha(resources / "manual/documentation.json")
        (resources / "release-build.json").write_bytes(release.encode(self.receipt))
        self.info = {"CFBundleIdentifier": release.delivery.IDENTITY, "CFBundleExecutable": "hex", "CFBundleVersion": "1",
                     "CFBundleShortVersionString": "0.1.0", "HexReleaseVersion": self.receipt["version"],
                     "HexReleaseChannel": "beta", "HexSourceRevision": self.receipt["source"],
                     "NSMicrophoneUsageDescription": "hex uses audio input to record audio into your project."}
        self.save_info()
        self.addCleanup(patch.stopall)
        patch.object(release.delivery, "ROOT", self.root).start()
        patch.object(release.delivery, "native_signature", return_value={}).start()
        patch.object(release.delivery, "verify_signed_bundle").start()
        patch.object(release.delivery, "verify_native_executable").start()
        self.signature = patch.object(release.delivery.subprocess, "run", return_value=SimpleNamespace(
            stdout=plistlib.dumps({"com.apple.security.device.audio-input": True}),
            stderr=b"flags=0x10000(runtime)\nTimestamp=fixture secure timestamp\n")).start()

    def save_info(self):
        (self.app / "Contents/Info.plist").write_bytes(plistlib.dumps(self.info))

    def test_installer_recognizes_distribution_and_refuses_changed_version_or_notes(self):
        self.assertEqual(release.delivery.verify(self.app), self.receipt)
        self.info["CFBundleVersion"] = "2"
        self.save_info()
        with self.assertRaisesRegex(release.Refusal, "metadata"):
            release.delivery.verify(self.app)
        self.info["CFBundleVersion"] = "1"
        self.save_info()
        (self.app / "Contents/Resources/notes.json").write_bytes(b"changed")
        with self.assertRaisesRegex(release.Refusal, "resource"):
            release.delivery.verify(self.app)

    def test_distribution_requires_runtime_timestamp_and_audio_input_entitlement(self):
        for details, entitlements in ((b"flags=0x0\nTimestamp=fixture\n", {"com.apple.security.device.audio-input": True}),
                                      (b"flags=0x10000(runtime)\n", {"com.apple.security.device.audio-input": True}),
                                      (b"flags=0x10000(runtime)\nTimestamp=fixture\n", {})):
            self.signature.return_value = SimpleNamespace(stdout=plistlib.dumps(entitlements), stderr=details)
            with self.subTest(details=details, entitlements=entitlements), self.assertRaisesRegex(release.Refusal, "hardened"):
                release.delivery.verify_distribution(self.app)


if __name__ == "__main__":
    unittest.main()
