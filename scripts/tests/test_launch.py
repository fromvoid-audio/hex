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
        self.raw_processes = ""

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
            return self.process(argv, stdout=self.raw_processes)
        self.fail(f"unexpected command: {argv}")

    def run_success(self, *, tested=False):
        with patch.object(launch.sys, "platform", "darwin"), \
                patch.object(launch, "command", side_effect=self.command), \
                patch.object(launch.ci, "LocalCiRunner", side_effect=self.fake_ci if tested else AssertionError("CI on default launch")), \
                patch.object(launch.delivery, "assert_inactive"), \
                patch.object(launch.delivery, "verify", return_value={
                    "source": self.source, "candidate": self.source, "profile": "release",
                }):
            return launch.run_launch(self.root, argv=["--test"] if tested else [])

    def test_success_fetches_verifies_packages_then_opens_exact_configured_app(self):
        # An installed app must not turn normal launch into installed-only startup.
        (self.root / "artifacts/hex.app").mkdir(parents=True)
        app = self.run_success()
        self.assertEqual(app, self.root.resolve() / "artifacts/hex.app")
        steps = [call[0] if call[0] == "ci" else call[:2] for call in self.calls]
        self.assertEqual(steps, [
            ("git", "rev-parse"), ("git", "branch"), ("git", "status"),
            ("git", "rev-parse"), ("git", "fetch"), ("git", "rev-parse"), ("git", "merge-base"),
            ("git", "merge"), ("git", "rev-parse"), ("git", "status"),
            ("gh", "api"), ("ps", "-ww"),
            ("sh", "scripts/bundle-macos.sh"), ("open", "-a"),
        ])
        self.assertEqual(self.calls[-1], ("open", "-a", str(app)))
        package = next(call for call in self.calls if call[0] == "sh")
        self.assertIn("--build-only", package)
        self.assertNotIn("--ci-report", package)
        self.assertFalse((self.root / "artifacts/local-ci").exists())
        included = json.loads(Path(package[package.index("--included") + 1]).read_text())
        self.assertEqual(included, [{"pr": "https://github.com/PORTALSURFER/hex/pull/316", "head": self.pull_head}])
        checklist = Path(package[package.index("--checklist") + 1]).read_text()
        self.assertIn(self.source, checklist)
        self.assertIn("tests not run", checklist)
        self.assertNotIn("full local CI passed", checklist)

    def test_test_mode_runs_full_ci_before_packaging_with_its_report(self):
        self.run_success(tested=True)
        self.assertLess(self.calls.index(("ci",)), next(i for i, call in enumerate(self.calls) if call[0] == "sh"))
        package = next(call for call in self.calls if call[0] == "sh")
        self.assertNotIn("--build-only", package)
        report = Path(package[package.index("--ci-report") + 1])
        self.assertEqual(report, self.root / "artifacts/local-ci/run-id/summary.json")
        self.assertIn("full local CI passed", (report.parent / "USER-TEST.md").read_text())

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
                launch.run_launch(self.root, argv=["--test"])
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

    def test_raw_desktop_process_refuses_before_ci_or_replacement(self):
        for executable in ("/tmp/hex/target/release/hex-desktop",
                           "/tmp/hex checkout/target/debug/hex-desktop", "hex-desktop"):
            with self.subTest(executable=executable), patch.object(launch.sys, "platform", "darwin"), \
                    patch.object(launch, "command", side_effect=self.command), \
                    patch.object(launch.ci, "LocalCiRunner", self.fake_ci), \
                    patch.object(launch.delivery, "assert_inactive"):
                self.calls.clear()
                self.raw_processes = f" 40060 {executable}\n"
                with self.assertRaisesRegex(launch.LaunchError, r"raw hex-desktop is running \(pid 40060\)"):
                    launch.run_launch(self.root)
                self.assertNotIn(("ci",), self.calls)
                self.assertFalse(any(call[:2] == ("sh", "scripts/bundle-macos.sh") for call in self.calls))
                self.assertFalse(any(call[:2] == ("open", "-a") for call in self.calls))

    def test_process_inspection_failure_refuses_before_ci_or_replacement(self):
        def failed_ps(argv, **kwargs):
            if argv[:3] == ["ps", "-ww", "-axo"]:
                self.calls.append(tuple(argv))
                return self.process(argv, returncode=1, stderr="process inspection unavailable")
            return self.command(argv, **kwargs)

        with patch.object(launch.sys, "platform", "darwin"), \
                patch.object(launch, "command", side_effect=failed_ps), \
                patch.object(launch.ci, "LocalCiRunner", self.fake_ci), \
                patch.object(launch.delivery, "assert_inactive"):
            with self.assertRaisesRegex(launch.LaunchError, "process inspection unavailable"):
                launch.run_launch(self.root)
        self.assertNotIn(("ci",), self.calls)
        self.assertFalse(any(call[0] in ("sh", "open") for call in self.calls))

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

    def test_default_update_reexecs_updated_entrypoint_without_changing_mode(self):
        self.initial_head = "c" * 40
        with patch.object(launch.sys, "platform", "darwin"), \
                patch.object(launch, "command", side_effect=self.command), \
                patch.object(launch.os, "execv") as execv, \
                patch.object(launch.ci, "LocalCiRunner", self.fake_ci):
            with self.assertRaisesRegex(launch.LaunchError, "could not restart"):
                launch.run_launch(self.root, argv=[])
        execv.assert_called_once_with(sys.executable, [sys.executable, str(self.root / "launch")])
        self.assertNotIn(("ci",), self.calls)

    def test_installed_only_opens_verified_app_without_git_cargo_ci_or_source_checks(self):
        app = self.root.resolve() / "artifacts/hex.app"
        # Installed-only startup does not require the caller's source to match the app.
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
                with self.assertRaisesRegex(launch.LaunchError, "run ./launch to verify and build it"):
                    launch.launch_installed(self.root)
        for receipt in ({"source": self.source, "profile": "debug"}, {"source": "unknown", "profile": "release"}):
            with self.subTest(receipt=receipt), patch.object(launch.sys, "platform", "darwin"), \
                    patch.object(launch.delivery, "verify", return_value=receipt):
                with self.assertRaisesRegex(launch.LaunchError, "run ./launch to verify and build it"):
                    launch.launch_installed(self.root)
        self.assertEqual(self.calls, [])

    def test_cli_default_test_open_help_and_unknown_option_dispatch(self):
        with patch.object(launch, "launch_installed") as startup, patch.object(launch, "run_launch") as test_run:
            self.assertEqual(launch.main([]), 0)
            startup.assert_not_called()
            test_run.assert_called_once_with(argv=[])
        with patch.object(launch, "launch_installed") as startup, patch.object(launch, "run_launch") as test_run:
            self.assertEqual(launch.main(["--test"]), 0)
            startup.assert_not_called()
            test_run.assert_called_once_with(argv=["--test"])
        with patch.object(launch, "launch_installed") as startup, patch.object(launch, "run_launch") as test_run:
            self.assertEqual(launch.main(["--open"]), 0)
            startup.assert_called_once_with()
            test_run.assert_not_called()
        with patch.object(launch, "launch_installed") as startup, \
                patch.object(launch, "run_launch", side_effect=launch.LaunchError("full local CI failed")), \
                redirect_stderr(io.StringIO()) as error:
            self.assertEqual(launch.main(["--test"]), 1)
            self.assertIn("full local CI failed", error.getvalue())
            startup.assert_not_called()
        for args, exit_code in ((["--help"], 0), (["--unknown"], 2), (["--open", "--test"], 2)):
            with self.subTest(args=args), patch.object(launch, "launch_installed") as startup, \
                    patch.object(launch, "run_launch") as test_run, redirect_stdout(io.StringIO()) as output, \
                    redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as result:
                    launch.main(args)
                self.assertEqual(result.exception.code, exit_code)
                startup.assert_not_called()
                test_run.assert_not_called()
                if args == ["--help"]:
                    self.assertIn("update clean main", output.getvalue())
                    self.assertIn("--open", output.getvalue())
                    self.assertIn("--test", output.getvalue())

    def test_opt_in_tests_survive_updated_launcher_reexec(self):
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

    def test_force_restart_passes_only_inspected_owner_and_preserves_default_no_tests(self):
        for owner in (None,'e'*64):
            self.calls.clear();self.head_reads=0
            with patch.object(launch.sys,'platform','darwin'), patch.object(launch,'command',side_effect=self.command), \
                    patch.object(launch.restart,'prepare',return_value=owner) as prepare, \
                    patch.object(launch.delivery,'assert_inactive'), \
                    patch.object(launch.delivery,'verify',return_value={'source':self.source,'candidate':self.source,'profile':'release'}), \
                    patch.object(launch.ci,'LocalCiRunner',side_effect=AssertionError('default ran tests')):
                launch.run_launch(self.root,argv=['--force-restart'])
            prepare.assert_called_once()
            package=next(x for x in self.calls if x[0]=='sh')
            self.assertIn('--build-only',package)
            if owner:
                self.assertIn('--adopt-release-target',package)
                self.assertEqual(package[package.index('--expected-release-owner-sha256')+1],owner)
            else:self.assertNotIn('--adopt-release-target',package)
            self.assertEqual(sum(x[0]=='open' for x in self.calls),1)

    def test_force_test_combination_keeps_actual_full_ci_path_and_report(self):
        with patch.object(launch.sys,'platform','darwin'),patch.object(launch,'command',side_effect=self.command), \
                patch.object(launch.restart,'prepare',return_value='e'*64),patch.object(launch.delivery,'assert_inactive'), \
                patch.object(launch.ci,'LocalCiRunner',side_effect=self.fake_ci), \
                patch.object(launch.delivery,'verify',return_value={'source':self.source,'candidate':self.source,'profile':'release'}):
            launch.run_launch(self.root,argv=['--force-restart','--test'])
        self.assertIn(('ci',),self.calls)
        package=next(x for x in self.calls if x[0]=='sh')
        self.assertIn('--ci-report',package);self.assertNotIn('--build-only',package)
        self.assertIn('--expected-release-owner-sha256',package)
        self.assertEqual(sum(x[0]=='open' for x in self.calls),1)

    def test_restart_refusal_interrupt_or_build_failure_never_opens_app(self):
        for failure in (launch.restart.RestartError('identity changed'),KeyboardInterrupt()):
            self.calls.clear();self.head_reads=0
            with patch.object(launch.sys,'platform','darwin'),patch.object(launch,'command',side_effect=self.command), \
                    patch.object(launch.restart,'prepare',side_effect=failure):
                with self.assertRaises(type(failure)):launch.run_launch(self.root,argv=['--force-restart'])
            self.assertFalse(any(x[0] in ('sh','open') for x in self.calls))
        self.calls.clear();self.head_reads=0
        def fail_build(argv,**kwargs):
            if argv[:2]==['sh','scripts/bundle-macos.sh']:
                self.calls.append(tuple(argv));return self.process(argv,returncode=1,stderr='build failed after quit')
            return self.command(argv,**kwargs)
        with patch.object(launch.sys,'platform','darwin'),patch.object(launch,'command',side_effect=fail_build), \
                patch.object(launch.restart,'prepare',return_value='e'*64),patch.object(launch.delivery,'assert_inactive'):
            with self.assertRaisesRegex(launch.LaunchError,'build failed after quit'):
                launch.run_launch(self.root,argv=['--force-restart'])
        self.assertFalse(any(x[0]=='open' for x in self.calls))

    def test_force_flag_is_preserved_across_main_update_reexec(self):
        self.initial_head='c'*40
        with patch.object(launch.sys,'platform','darwin'),patch.object(launch,'command',side_effect=self.command), \
                patch.object(launch.os,'execv') as execv,patch.object(launch.restart,'prepare') as prepare:
            with self.assertRaises(launch.LaunchError):launch.run_launch(self.root,argv=['--force-restart','--test'])
        execv.assert_called_once_with(sys.executable,[sys.executable,str(self.root/'launch'),'--force-restart','--test'])
        prepare.assert_not_called()


    def test_recreated_target_opt_in_forwards_only_inspected_identity_without_restart_or_tests(self):
        authority={'owner_sha256':'e'*64,'target_identity':{'device':17,'inode':42}}
        with patch.object(launch.sys,'platform','darwin'), patch.object(launch,'command',side_effect=self.command), \
                patch.object(launch.restart,'inspect_recreated_target',return_value=authority) as inspect, \
                patch.object(launch.restart,'prepare',side_effect=AssertionError('no process control')), \
                patch.object(launch.delivery,'assert_inactive') as inactive, \
                patch.object(launch.delivery,'verify',return_value={'source':self.source,'candidate':self.source,'profile':'release'}), \
                patch.object(launch.ci,'LocalCiRunner',side_effect=AssertionError('tests not requested')):
            launch.run_launch(self.root,argv=['--recover-release-target'])
        inspect.assert_called_once();inactive.assert_called_once()
        package=next(x for x in self.calls if x[0]=='sh')
        self.assertIn('--build-only',package)
        for flag,value in [('--expected-release-owner-sha256','e'*64),
                           ('--expected-recreated-target-device','17'),('--expected-recreated-target-inode','42')]:
            self.assertEqual(package[package.index(flag)+1],value)
        self.assertEqual(sum(x[0]=='open' for x in self.calls),1)

    def test_recreated_target_inspection_failure_or_running_raw_never_builds_or_opens(self):
        for running in (False,True):
            self.calls.clear();self.head_reads=0
            self.raw_processes='123 /fixture/hex-desktop' if running else ''
            with patch.object(launch.sys,'platform','darwin'), patch.object(launch,'command',side_effect=self.command), \
                    patch.object(launch.restart,'inspect_recreated_target',side_effect=launch.ci.TargetCacheError('foreign')) as inspect, \
                    patch.object(launch.restart,'prepare') as force, patch.object(launch.delivery,'assert_inactive'):
                with self.assertRaises((launch.LaunchError,launch.ci.TargetCacheError)):
                    launch.run_launch(self.root,argv=['--recover-release-target'])
            if running:inspect.assert_not_called()
            force.assert_not_called()
            self.assertFalse(any(x[0] in ('sh','open') for x in self.calls))

    def test_recreated_target_cli_excludes_force_and_installed_only_but_allows_test(self):
        for incompatible in ('--open','--force-restart'):
            with patch.object(launch,'run_launch') as run, redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as error:
                    launch.main(['--recover-release-target',incompatible])
            self.assertEqual(error.exception.code,2);run.assert_not_called()
        with patch.object(launch,'run_launch') as run:
            self.assertEqual(launch.main(['--recover-release-target','--test']),0)
        run.assert_called_once_with(argv=['--recover-release-target','--test'])

    def test_recreated_target_flag_survives_ordinary_main_update_reexec(self):
        with patch.object(launch.sys,'platform','darwin'), patch.object(launch,'require_clean_main'), \
                patch.object(launch,'checked',return_value='d'*40), \
                patch.object(launch,'fetch_and_fast_forward',return_value=self.source), \
                patch.object(launch.os,'execv',side_effect=OSError('fixture exec')) as reexec, \
                patch.object(launch.restart,'inspect_recreated_target') as inspect:
            with self.assertRaises(OSError):launch.run_launch(self.root,argv=['--recover-release-target'])
        reexec.assert_called_once_with(launch.sys.executable,[launch.sys.executable,str(self.root/'launch'),'--recover-release-target'])
        inspect.assert_not_called()


    def test_recover_with_tests_uses_fresh_real_runner_target_preserving_stale_dev_owner_and_release_target(self):
        import os
        import test_local_ci as fixtures
        f=fixtures.LocalCiRunnerTests();f.setUp();self.addCleanup(f.tearDown)
        self.root=f.repo
        (self.root/'scripts').mkdir();(self.root/'.git').mkdir()
        (self.root/'scripts/hex-test-build.json').write_text('{"app_path":"artifacts/hex.app"}')
        git=f.bin/'git';git.write_text(git.read_text().replace('deadbeef',self.source))
        cargo=f.bin/'cargo';cargo.write_text(cargo.read_text().replace('"$FAKE_METADATA_TARGET"','"${CARGO_TARGET_DIR:-$FAKE_METADATA_TARGET}"'))
        target=Path(f.env['FAKE_METADATA_TARGET']);target.mkdir()
        old=f.runner('old-dev-owner');self.assertEqual(old.run(),0)
        # Read the marker selected by the runner, not a second lexical-path hash.
        owner=old.target_owner_path
        self.assertIsNotNone(owner)
        raw=owner.read_bytes()
        target.rename(target.with_name('retained-dev'));target.mkdir()
        (target/'release').mkdir();(target/'release/external').write_bytes(b'preserve release')
        stale=f.runner('stale-default-control');self.assertEqual(stale.run(),1)
        self.assertEqual(owner.read_bytes(),raw)
        authority={'owner_sha256':'e'*64,'target_identity':stale._directory_identity(target)}
        observed=[];package_targets=[]
        def verification(root,mode,env=None):
            selected=dict(f.env)
            if env is not None and 'CARGO_TARGET_DIR' in env:selected['CARGO_TARGET_DIR']=env['CARGO_TARGET_DIR']
            runner=f.runner('recovery-verification',root=root,env=selected)
            observed.append(runner);return runner
        def command(argv,**kwargs):
            if argv[:2]==['sh','scripts/bundle-macos.sh']:package_targets.append(os.environ['CARGO_TARGET_DIR'])
            return self.command(argv,**kwargs)
        with patch.dict(os.environ,{**f.env,'CARGO_TARGET_DIR':str(target)},clear=True), \
                patch.object(launch.sys,'platform','darwin'),patch.object(launch,'command',side_effect=command), \
                patch.object(launch.ci,'LocalCiRunner',side_effect=verification), \
                patch.object(launch.restart,'inspect_recreated_target',return_value=authority), \
                patch.object(launch.delivery,'assert_inactive'), \
                patch.object(launch.delivery,'verify',return_value={'source':self.source,'candidate':self.source,'profile':'release'}):
            launch.run_launch(self.root,argv=['--recover-release-target','--test'])
        self.assertEqual(len(observed),1)
        runner=observed[0];actual=Path(runner.cargo_identity['target_directory'])
        self.assertNotEqual(actual,target);self.assertTrue(actual.is_relative_to(self.root/'artifacts/launch'))
        self.assertFalse(runner.adopt_target);self.assertEqual(runner.summary['target_cache']['action'],'claim-empty')
        self.assertEqual(runner.summary['status'],'passed');self.assertIsNone(runner.target_lock)
        self.assertEqual(owner.read_bytes(),raw)
        self.assertEqual((target/'release/external').read_bytes(),b'preserve release')
        self.assertEqual(package_targets,[str(target)],'packaging inherits ORIGINAL release target')
        self.assertNotIn('cargo clean',f.call_log.read_text())
        package=next(x for x in self.calls if x[0]=='sh')
        self.assertIn('--ci-report',package);self.assertIn('--expected-recreated-target-inode',package)


class ForceRestartFlagTests(unittest.TestCase):
    def test_native_bundle_verification_failure_is_a_controlled_cli_refusal(self):
        with patch.object(launch,'run_launch',side_effect=launch.delivery.DeliveryError('sealed bundle mismatch')), \
                redirect_stderr(io.StringIO()) as errors:
            self.assertEqual(launch.main(['--force-restart']),1)
        self.assertIn('sealed bundle mismatch',errors.getvalue())

    def test_installed_only_force_combination_is_refused_and_help_warns_unsaved_loss(self):
        with redirect_stderr(io.StringIO()),self.assertRaises(SystemExit) as result:
            launch.main(['--open','--force-restart'])
        self.assertEqual(result.exception.code,2)
        with redirect_stdout(io.StringIO()) as output,self.assertRaises(SystemExit):launch.main(['--help'])
        self.assertIn('unsaved work may be lost',' '.join(output.getvalue().split()))

    def test_force_restart_is_forwarded_and_can_opt_in_to_tests(self):
        for arguments in (["--force-restart"], ["--force-restart", "--test"]):
            with self.subTest(arguments=arguments), patch.object(launch, "run_launch") as run:
                self.assertEqual(launch.main(list(arguments)), 0)
                run.assert_called_once_with(argv=list(arguments))



if __name__ == "__main__":
    unittest.main()
