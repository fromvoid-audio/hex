"""Bounded terminal activity without real build/test subprocesses."""

import importlib.util
from contextlib import contextmanager
import io
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import unittest
from types import SimpleNamespace
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location(
    "progress_local_ci", Path(__file__).resolve().parents[1] / "local-ci.py")
ci = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = ci
SPEC.loader.exec_module(ci)
DELIVERY_SPEC = importlib.util.spec_from_file_location(
    "progress_delivery", Path(__file__).resolve().parents[1] / "hex-test-build.py")
delivery = importlib.util.module_from_spec(DELIVERY_SPEC)
sys.modules[DELIVERY_SPEC.name] = delivery
DELIVERY_SPEC.loader.exec_module(delivery)


class Terminal(io.StringIO):
    def __init__(self, tty=True):
        super().__init__()
        self.tty = tty
        self.animated = threading.Event()
        self.frames = 0

    def isatty(self):
        return self.tty

    def write(self, text):
        result = super().write(text)
        if "checking fixture" in text:
            self.frames += 1
            if self.frames >= 2:
                self.animated.set()
        return result


class ProgressTests(unittest.TestCase):
    def test_terminal_animates_and_joins_before_following_diagnostics(self):
        output, errors = Terminal(), Terminal()
        with patch.object(ci.sys, "stdout", output), patch.object(ci.sys, "stderr", errors):
            with ci.terminal_progress("checking fixture"):
                self.assertTrue(errors.animated.wait(2), "spinner did not advance")
                workers = [t for t in threading.enumerate() if t.name == "hex-terminal-progress"]
                self.assertEqual(len(workers), 1)
            self.assertFalse(workers[0].is_alive())
            self.assertTrue(errors.getvalue().endswith("\r\033[2K"))
            print("failure detail remains readable", file=errors)
        self.assertTrue(errors.getvalue().endswith("\r\033[2Kfailure detail remains readable\n"))
        self.assertEqual(output.getvalue(), "")

    def test_redirected_output_is_plain_with_no_thread(self):
        for stdout_tty, stderr_tty in ((False, False), (False, True), (True, False)):
            with self.subTest(stdout=stdout_tty, stderr=stderr_tty):
                output, errors = Terminal(stdout_tty), Terminal(stderr_tty)
                with patch.object(ci.sys, "stdout", output), patch.object(ci.sys, "stderr", errors), \
                        patch.object(ci.threading, "Thread", side_effect=AssertionError("thread for redirected logs")):
                    with ci.terminal_progress("building optimized hex"):
                        pass
                self.assertEqual(errors.getvalue(), "building optimized hex...\n")
                self.assertEqual(output.getvalue(), "")

    def test_failure_and_interrupt_propagate_after_spinner_cleanup(self):
        for failure in (subprocess.CalledProcessError(7, ["fixture"]), KeyboardInterrupt(), ci.InterruptedRun()):
            with self.subTest(failure=type(failure).__name__):
                output, errors = Terminal(), Terminal()
                with patch.object(ci.sys, "stdout", output), patch.object(ci.sys, "stderr", errors):
                    with self.assertRaises(type(failure)) as caught:
                        with ci.terminal_progress("checking fixture"):
                            workers = [t for t in threading.enumerate() if t.name == "hex-terminal-progress"]
                            raise failure
                self.assertIs(caught.exception, failure)
                self.assertFalse(workers[0].is_alive())
                self.assertTrue(errors.getvalue().endswith("\r\033[2K"))

    def test_narrow_terminal_frame_does_not_wrap(self):
        output, errors = Terminal(), Terminal()
        with patch.object(ci.sys, "stdout", output), patch.object(ci.sys, "stderr", errors), \
                patch.object(ci.shutil, "get_terminal_size", return_value=SimpleNamespace(columns=8)):
            with ci.terminal_progress("building optimized hex"):
                pass
        self.assertTrue(errors.getvalue().startswith("\r\033[2K| build"))
        self.assertNotIn("optimized", errors.getvalue())

    def test_build_failure_and_interrupt_keep_diagnostics_and_release_guard(self):
        for interrupted in (False, True):
            with self.subTest(interrupted=interrupted), tempfile.TemporaryDirectory() as directory:
                released = []

                @contextmanager
                def guarded_release(**kwargs):
                    try:
                        yield {}
                    finally:
                        released.append(True)

                runner = SimpleNamespace(root=Path(directory), output_dir=Path(directory),
                                         env={}, guarded_release=guarded_release)

                def build(argv, **kwargs):
                    self.assertEqual(kwargs["stdout"], subprocess.PIPE)
                    kwargs["stderr"].write("compiler diagnostic retained\n")
                    if interrupted:
                        raise KeyboardInterrupt()
                    return SimpleNamespace(stdout="", returncode=7)

                output, errors = Terminal(), Terminal()
                with patch.object(ci.sys, "stdout", output), patch.object(ci.sys, "stderr", errors), \
                        patch.object(delivery.subprocess, "run", side_effect=build):
                    with self.assertRaises(KeyboardInterrupt if interrupted else delivery.DeliveryError):
                        with delivery.optimized_artifact(runner):
                            self.fail("failed build yielded an artifact")
                self.assertEqual(released, [True])
                self.assertIn("compiler diagnostic retained", (Path(directory) / "optimized-build.log").read_text())
                self.assertTrue(errors.getvalue().endswith("\r\033[2K"))
                self.assertEqual(output.getvalue(), "")
                self.assertFalse(any(t.name == "hex-terminal-progress" for t in threading.enumerate()))

    def test_logged_runner_failure_and_interrupt_keep_status_and_child_log(self):
        for interruption in (False, True):
            with self.subTest(interruption=interruption), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                runner = ci.LocalCiRunner(root, steps=(ci.Step("fixture", ("fixture",)),))
                runner.output_dir.mkdir(parents=True)
                errors, output = Terminal(), Terminal()

                def child(argv, **kwargs):
                    kwargs["stdout"].write("actual child diagnostic\n")
                    self.assertIs(kwargs["stderr"], subprocess.STDOUT)
                    if interruption:
                        raise ci.InterruptedRun()
                    return SimpleNamespace(returncode=7)

                with patch.object(ci.sys, "stdout", output), patch.object(ci.sys, "stderr", errors), \
                        patch.object(ci.subprocess, "run", side_effect=child):
                    if interruption:
                        with self.assertRaises(ci.InterruptedRun):
                            runner._run_step(runner.steps[0], 1)
                        self.assertEqual(runner.summary["steps"][0]["exit_code"], 130)
                    else:
                        result = runner._run_step(runner.steps[0], 1)
                        self.assertEqual((result["status"], result["exit_code"]), ("failed", 7))
                self.assertIn("actual child diagnostic", (runner.output_dir / "01-fixture.log").read_text())
                self.assertTrue(errors.getvalue().endswith("\r\033[2K"))
                self.assertFalse(any(t.name == "hex-terminal-progress" for t in threading.enumerate()))


if __name__ == "__main__":
    unittest.main()
