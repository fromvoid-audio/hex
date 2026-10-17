"""Orchestration tests for the root macOS launch command."""

import importlib.util
from contextlib import redirect_stderr, redirect_stdout
import io
import importlib.machinery
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parents[2] / "launch"
LOADER = importlib.machinery.SourceFileLoader("hex_launch", str(SCRIPT))
SPEC = importlib.util.spec_from_loader("hex_launch", LOADER)
launch = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = launch
SPEC.loader.exec_module(launch)


class LaunchTests(unittest.TestCase):
    source = "a" * 40
    pull_head = "b" * 40

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.root.mkdir(exist_ok=True)
        (self.root / "scripts").mkdir()
        (self.root / ".git").mkdir()
        (self.root / "scripts/hex-test-build.json").write_text('{"app_path":"artifacts/hex.app"}\n')
        self.calls = []
        self.initial_head = self.source
        self.head_reads = 0
        self.ci_result = 0
        self.ci_status = "passed"

        class FakeCiRunner:
            def __init__(inner, root, mode):
                inner.output_dir = root / "artifacts/local-ci/run-id"
                inner.summary_path = inner.output_dir / "summary.json"
                inner.summary = {}

            def run(inner):
                self.calls.append(("ci",))
                inner.output_dir.mkdir(parents=True)
                inner.summary_path.write_text("{}\n")
                inner.summary = {"status": self.ci_status, "commit": self.source,
                                 "dirty": False, "stale": False}
                return self.ci_result

        self.fake_ci = FakeCiRunner

    def tearDown(self):
        self.temp.cleanup()

    def process(self, argv, returncode=0, stdout="", stderr=""):
        return subprocess.CompletedProcess(argv, returncode, stdout, stderr)

    def command(self, argv, *, cwd, capture=True):
        self.calls.append(tuple(argv))
        if argv[:3] == ["git", "rev-parse", "--show-toplevel"]:
            return self.process(argv, stdout=str(self.root))
        if argv == ["git", "branch", "--show-current"]:
            return self.process(argv, stdout="main")
        if argv[:2] == ["git", "status"]:
            return self.process(argv, stdout="")
        if argv[:2] == ["git", "fetch"]:
            return self.process(argv)
        if argv[:2] == ["git", "rev-parse"] and argv[-1] == "refs/remotes/origin/main":
            return self.process(argv, stdout=self.source)
        if argv[:2] == ["git", "merge-base"]:
            return self.process(argv)
        if argv[:2] == ["git", "merge"]:
            return self.process(argv)
        if argv[:2] == ["git", "rev-parse"] and argv[-1] == "HEAD":
            self.head_reads += 1
            head = self.initial_head if self.head_reads == 1 else self.source
            return self.process(argv, stdout=head)
        if argv[:3] == ["gh", "api", f"repos/PORTALSURFER/hex/commits/{self.source}/pulls"]:
            return self.process(argv, stdout=json.dumps([{"number": 316, "head": {"sha": self.pull_head}}]))
        if argv[:3] == ["git", "rev-parse", "--git-common-dir"]:
            return self.process(argv, stdout=".git")
        if argv[:2] == ["sh", "scripts/bundle-macos.sh"]:
            return self.process(argv)
        if argv[:2] == ["open", "-a"]:
            return self.process(argv)
        if argv[:3] == ["ps", "-ww", "-axo"]:
            return self.process(argv)
        self.fail(f"unexpected command: {argv}")

    def run_success(self):
        with patch.object(launch.sys, "platform", "darwin"), \
                patch.object(launch, "command", side_effect=self.command), \
                patch.object(launch.ci, "LocalCiRunner", self.fake_ci), \
                patch.object(launch.delivery, "assert_inactive"), \
                patch.object(launch.delivery, "verify", return_value={
                    "source": self.source, "candidate": self.source, "profile": "release",
                }):
            return launch.run_launch(self.root)

    def test_success_fetches_verifies_packages_then_opens_exact_configured_app(self):
        app = self.run_success()
        self.assertEqual(app, self.root.resolve() / "artifacts/hex.app")
        steps = [call[0] if call[0] == "ci" else call[:2] for call in self.calls]
        self.assertEqual(steps, [
            ("git", "rev-parse"), ("git", "branch"), ("git", "status"),
            ("git", "rev-parse"), ("git", "fetch"), ("git", "rev-parse"), ("git", "merge-base"),
            ("git", "merge"), ("git", "rev-parse"), ("git", "status"),
            ("gh", "api"), "ci",
            ("sh", "scripts/bundle-macos.sh"), ("open", "-a"),
        ])
        self.assertEqual(self.calls[-1], ("open", "-a", str(app)))
        included = json.loads((self.root / "artifacts/local-ci/run-id/included-test-revisions.json").read_text())
        self.assertEqual(included, [{"pr": "https://github.com/PORTALSURFER/hex/pull/316", "head": self.pull_head}])
        self.assertIn(self.source, (self.root / "artifacts/local-ci/run-id/USER-TEST.md").read_text())

    def test_refuses_dirty_checkout(self):
        def dirty(argv, **kwargs):
            result = self.command(argv, **kwargs)
            if argv[:2] == ["git", "status"]:
                return self.process(argv, stdout=" M README.md")
            return result

        with patch.object(launch.sys, "platform", "darwin"), patch.object(launch, "command", side_effect=dirty):
            with self.assertRaisesRegex(launch.LaunchError, "dirty checkout"):
                launch.run_launch(self.root)
        self.assertFalse(any(call[0] == "git" and call[1] == "fetch" for call in self.calls))

    def test_refuses_non_main_and_detached_checkouts(self):
        for branch in ("feature", ""):
            with self.subTest(branch=branch):
                self.calls.clear()

                def wrong_branch(argv, **kwargs):
                    if argv == ["git", "branch", "--show-current"]:
                        return self.process(argv, stdout=branch)
                    return self.command(argv, **kwargs)

                with patch.object(launch.sys, "platform", "darwin"), patch.object(launch, "command", side_effect=wrong_branch):
                    with self.assertRaisesRegex(launch.LaunchError, "refusing branch"):
                        launch.run_launch(self.root)
                self.assertFalse(any(call[0] == "git" and call[1] == "fetch" for call in self.calls))

    def test_refuses_divergent_or_ahead_main_without_merging(self):
        def divergent(argv, **kwargs):
            if argv[:2] == ["git", "merge-base"]:
                self.calls.append(tuple(argv))
                return self.process(argv, returncode=1)
            return self.command(argv, **kwargs)

        with patch.object(launch.sys, "platform", "darwin"), patch.object(launch, "command", side_effect=divergent):
            with self.assertRaisesRegex(launch.LaunchError, "ahead of or divergent"):
                launch.run_launch(self.root)
        self.assertFalse(any(call[0] == "git" and call[1] == "merge" for call in self.calls))

    def test_fetch_failure_stops_before_ci(self):
        def fetch_fails(argv, **kwargs):
            if argv[:2] == ["git", "fetch"]:
                self.calls.append(tuple(argv))
                return self.process(argv, returncode=1, stderr="network unavailable")
            return self.command(argv, **kwargs)

        with patch.object(launch.sys, "platform", "darwin"), patch.object(launch, "command", side_effect=fetch_fails), \
                patch.object(launch.ci, "LocalCiRunner", self.fake_ci):
            with self.assertRaisesRegex(launch.LaunchError, "command failed"):
                launch.run_launch(self.root)
        self.assertNotIn(("ci",), self.calls)

    def test_ci_failure_stops_before_package_and_open(self):
        self.ci_result = 1
        self.ci_status = "failed"
        with patch.object(launch.sys, "platform", "darwin"), patch.object(launch, "command", side_effect=self.command), \
                patch.object(launch.ci, "LocalCiRunner", self.fake_ci), \
                patch.object(launch.delivery, "assert_inactive"):
            with self.assertRaisesRegex(launch.LaunchError, "full local CI failed"):
                launch.run_launch(self.root)
        self.assertFalse(any(call[0] == "open" for call in self.calls))
        self.assertFalse(any(call[0] == "sh" for call in self.calls))

    def test_package_failure_never_opens_app(self):
        def package_fails(argv, **kwargs):
            if argv[:2] == ["sh", "scripts/bundle-macos.sh"]:
                self.calls.append(tuple(argv))
                return self.process(argv, returncode=1, stderr="install refused")
            return self.command(argv, **kwargs)

        with patch.object(launch.sys, "platform", "darwin"), patch.object(launch, "command", side_effect=package_fails), \
                patch.object(launch.ci, "LocalCiRunner", self.fake_ci), \
                patch.object(launch.delivery, "assert_inactive"):
            with self.assertRaisesRegex(launch.LaunchError, "command failed"):
                launch.run_launch(self.root)
        self.assertFalse(any(call[0] == "open" for call in self.calls))

    def test_mismatched_installed_receipt_never_opens_app(self):
        with patch.object(launch.sys, "platform", "darwin"), \
                patch.object(launch, "command", side_effect=self.command), \
                patch.object(launch.ci, "LocalCiRunner", self.fake_ci), \
                patch.object(launch.delivery, "assert_inactive"), \
                patch.object(launch.delivery, "verify", return_value={
                    "source": "c" * 40, "candidate": self.source, "profile": "release",
                }):
            with self.assertRaisesRegex(launch.LaunchError, "does not match fetched main"):
                launch.run_launch(self.root)
        self.assertFalse(any(call[0] == "open" for call in self.calls))

    def test_updated_main_reexecs_updated_entrypoint_before_ci(self):
        self.initial_head = "c" * 40
        with patch.object(launch.sys, "platform", "darwin"), \
                patch.object(launch, "command", side_effect=self.command), \
                patch.object(launch.os, "execv") as execv, \
                patch.object(launch.ci, "LocalCiRunner", self.fake_ci):
            with self.assertRaisesRegex(launch.LaunchError, "could not restart"):
                launch.run_launch(self.root, argv=["--future-option"])
        execv.assert_called_once_with(sys.executable, [sys.executable, str(self.root / "launch"), "--future-option"])
        self.assertNotIn(("ci",), self.calls)
        self.assertFalse(any(call[0] == "gh" for call in self.calls))


    def test_default_opens_verified_app_without_git_cargo_ci_or_source_checks(self):
        app = self.root.resolve() / "artifacts/hex.app"
        # Normal startup does not require the caller's source to match the app.
        with patch.object(launch.sys, "platform", "darwin"), \
                patch.object(launch, "command", side_effect=self.command), \
                patch.object(launch, "require_clean_main", side_effect=AssertionError("source check on startup")), \
                patch.object(launch.ci, "LocalCiRunner", side_effect=AssertionError("CI on startup")), \
                patch.object(launch.delivery, "assert_inactive", side_effect=AssertionError("running app refused")), \
                patch.object(launch.delivery, "verify", return_value={"source": "c" * 40, "profile": "release"}):
            self.assertEqual(launch.launch_installed(self.root), app)
        self.assertEqual(self.calls, [("open", "-a", str(app))])

    def test_installed_app_target_resolves_linked_worktree_without_git(self):
        worktree = self.root / "linked"
        (worktree / "scripts").mkdir(parents=True)
        (worktree / "scripts/hex-test-build.json").write_text('{"app_path":"artifacts/hex.app"}')
        metadata = self.root / ".git/worktrees/linked"
        metadata.mkdir(parents=True)
        (metadata / "commondir").write_text("../..\n")
        (worktree / ".git").write_text("gitdir: ../.git/worktrees/linked\n")
        with patch.dict(launch.os.environ, {}, clear=True), patch.object(launch, "command", side_effect=AssertionError("Git on startup")):
            self.assertEqual(launch.configured_app(worktree), self.root.resolve() / "artifacts/hex.app")

    def test_missing_or_unsupported_app_has_test_guidance_without_running_tests(self):
        for failure in (FileNotFoundError("missing app"), launch.delivery.DeliveryError("checksum mismatch")):
            with self.subTest(failure=str(failure)), patch.object(launch.sys, "platform", "darwin"), \
                    patch.object(launch.delivery, "verify", side_effect=failure), \
                    patch.object(launch.ci, "LocalCiRunner", side_effect=AssertionError("CI on startup")):
                with self.assertRaisesRegex(launch.LaunchError, "run ./launch --test"):
                    launch.launch_installed(self.root)
        for receipt in ({"source": self.source, "profile": "debug"}, {"source": "unknown", "profile": "release"}):
            with self.subTest(receipt=receipt), patch.object(launch.sys, "platform", "darwin"), \
                    patch.object(launch.delivery, "verify", return_value=receipt):
                with self.assertRaisesRegex(launch.LaunchError, "run ./launch --test"):
                    launch.launch_installed(self.root)
        self.assertEqual(self.calls, [])

    def test_cli_default_test_help_and_unknown_option_dispatch(self):
        with patch.object(launch, "launch_installed") as startup, patch.object(launch, "run_launch") as test_run:
            self.assertEqual(launch.main([]), 0)
            startup.assert_called_once_with()
            test_run.assert_not_called()
        with patch.object(launch, "launch_installed") as startup, patch.object(launch, "run_launch") as test_run:
            self.assertEqual(launch.main(["--test"]), 0)
            startup.assert_not_called()
            test_run.assert_called_once_with(argv=["--test"])
        with patch.object(launch, "launch_installed") as startup, \
                patch.object(launch, "run_launch", side_effect=launch.LaunchError("full local CI failed")), \
                redirect_stderr(io.StringIO()) as error:
            self.assertEqual(launch.main(["--test"]), 1)
            self.assertIn("full local CI failed", error.getvalue())
            startup.assert_not_called()
        for args, exit_code in ((["--help"], 0), (["--unknown"], 2)):
            with self.subTest(args=args), patch.object(launch, "launch_installed") as startup, \
                    patch.object(launch, "run_launch") as test_run, redirect_stdout(io.StringIO()) as output, \
                    redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as result:
                    launch.main(args)
                self.assertEqual(result.exception.code, exit_code)
                startup.assert_not_called()
                test_run.assert_not_called()
                if args == ["--help"]:
                    self.assertIn("--test", output.getvalue())

    def test_requested_test_mode_survives_updated_launcher_reexec(self):
        previous = "d" * 40
        with patch.object(launch.sys, "platform", "darwin"), \
                patch.object(launch, "require_clean_main"), \
                patch.object(launch, "checked", return_value=previous), \
                patch.object(launch, "fetch_and_fast_forward", return_value=self.source), \
                patch.object(launch.os, "execv", side_effect=OSError("exec fixture")) as restart, \
                patch.object(launch.ci, "LocalCiRunner") as runner:
            with self.assertRaisesRegex(OSError, "exec fixture"):
                launch.run_launch(self.root, argv=["--test"])
        restart.assert_called_once_with(launch.sys.executable, [launch.sys.executable, str(self.root / "launch"), "--test"])
        runner.assert_not_called()


if __name__ == "__main__":
    unittest.main()
