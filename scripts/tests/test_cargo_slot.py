"""Exercise the cooperative slot with independent processes and no Cargo work."""

import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "cargo-slot.py"
CHILD = """
import pathlib, sys, time
pathlib.Path(sys.argv[1]).touch()
while not pathlib.Path(sys.argv[2]).exists():
    time.sleep(0.01)
"""


class CargoSlotTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.env = dict(os.environ, HEX_CARGO_SLOT_FILE=str(self.root / "slot.lock"))
        self.processes = []
        self.addCleanup(self.stop_processes)
        self.repo = self.root / "repo"
        subprocess.run(["git", "init", "-q", str(self.repo)], check=True)
        subprocess.run(
            ["git", "-C", str(self.repo), "-c", "user.name=fixture", "-c",
             "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false", "-c",
             "core.hooksPath=/dev/null", "commit", "-q", "--allow-empty", "-m", "fixture"],
            check=True,
        )

    def stop_processes(self):
        for process in self.processes:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait(timeout=3)

    def start(self, name):
        started, release = self.root / (name + ".started"), self.root / (name + ".release")
        checkout = self.root / name
        subprocess.run(["git", "-C", str(self.repo), "worktree", "add", "-q", "--detach", str(checkout)],
                       check=True)
        log_path = self.root / (name + ".log")
        log = log_path.open("w")
        self.addCleanup(log.close)
        process = subprocess.Popen(
            [sys.executable, str(SCRIPT), sys.executable, "-c", CHILD, str(started), str(release)],
            env=self.env, stdout=subprocess.DEVNULL, stderr=log,
            start_new_session=True, cwd=checkout,
        )
        self.processes.append(process)
        deadline = time.monotonic() + 3
        while "waiting for cargo slot:" not in log_path.read_text() and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertIn("waiting for cargo slot:", log_path.read_text())
        return process, started, release

    def wait_for(self, path):
        deadline = time.monotonic() + 3
        while not path.exists() and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertTrue(path.exists(), f"timed out waiting for {path}")

    def test_wrapper_death_does_not_release_running_child(self):
        first, first_started, first_release = self.start("first")
        self.wait_for(first_started)
        second, second_started, second_release = self.start("second")
        time.sleep(0.15)
        self.assertFalse(second_started.exists())
        first.kill()
        first.wait(timeout=3)
        time.sleep(0.15)
        self.assertFalse(second_started.exists())
        first_release.touch()
        self.wait_for(second_started)
        second_release.touch()
        self.assertEqual(second.wait(timeout=3), 0)

    def test_cancelled_waiter_does_not_block_next_command(self):
        first, first_started, first_release = self.start("first")
        self.wait_for(first_started)
        waiter, waiter_started, _ = self.start("waiter")
        time.sleep(0.15)
        self.assertFalse(waiter_started.exists())
        waiter.terminate()
        waiter.wait(timeout=3)
        next_process, next_started, next_release = self.start("next")
        first_release.touch()
        self.assertEqual(first.wait(timeout=3), 0)
        self.wait_for(next_started)
        next_release.touch()
        self.assertEqual(next_process.wait(timeout=3), 0)

    def test_refuses_symlink_and_propagates_child_failure(self):
        lock = self.root / "slot.lock"
        lock.symlink_to(self.root / "elsewhere")
        command = [sys.executable, str(SCRIPT), sys.executable, "-c", "raise SystemExit(7)"]
        refused = subprocess.run(command, env=self.env, capture_output=True, check=False)
        self.assertEqual(refused.returncode, 1)
        self.assertFalse((self.root / "elsewhere").exists())
        lock.unlink()
        result = subprocess.run(command, env=self.env, capture_output=True, check=False)
        self.assertEqual(result.returncode, 7)


if __name__ == "__main__":
    unittest.main()
