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
        self.logs = {}
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

    def start(self, name, wait_seconds=None):
        started, release = self.root / (name + ".started"), self.root / (name + ".release")
        checkout = self.root / name
        subprocess.run(["git", "-C", str(self.repo), "worktree", "add", "-q", "--detach", str(checkout)],
                       check=True)
        log_path = self.root / (name + ".log")
        self.logs[name] = log_path
        log = log_path.open("w")
        self.addCleanup(log.close)
        command = [sys.executable, str(SCRIPT)]
        if wait_seconds is not None:
            command.extend(["--wait-seconds", str(wait_seconds)])
        command.extend([sys.executable, "-c", CHILD, str(started), str(release)])
        process = subprocess.Popen(
            command,
            env=self.env, stdout=subprocess.DEVNULL, stderr=log,
            start_new_session=True, cwd=checkout,
        )
        self.processes.append(process)
        self.wait_for_log(name, "waiting for cargo slot:")
        return process, started, release

    def wait_for_log(self, name, marker):
        log_path = self.logs[name]
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            contents = log_path.read_text()
            if marker in contents:
                return contents
            time.sleep(0.01)
        self.fail(f"timed out waiting for {marker!r} in {name} log: {log_path.read_text()}")

    def wait_for(self, path):
        deadline = time.monotonic() + 3
        while not path.exists() and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertTrue(path.exists(), f"timed out waiting for {path}")

    def test_wrapper_death_does_not_release_running_child(self):
        first, first_started, first_release = self.start("first")
        self.wait_for(first_started)
        first.kill()
        first.wait(timeout=3)
        probe = subprocess.run(
            [
                sys.executable,
                "-c",
                "import fcntl, os, sys\n"
                "fd = os.open(sys.argv[1], os.O_RDWR)\n"
                "try:\n"
                "    fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)\n"
                "except BlockingIOError:\n"
                "    sys.exit(75)\n"
                "sys.exit(0)",
                self.env["HEX_CARGO_SLOT_FILE"],
            ],
            check=False,
            capture_output=True,
        )
        self.assertEqual(
            probe.returncode,
            75,
            "the child must retain the execution lock after wrapper death",
        )
        second, second_started, second_release = self.start("second")
        self.wait_for_log("second", "next cargo slot reserved:")
        self.assertFalse(second_started.exists())
        first_release.touch()
        self.wait_for(second_started)
        second_release.touch()
        self.assertEqual(second.wait(timeout=3), 0)

    def test_cancelled_waiter_does_not_block_next_command(self):
        first, first_started, first_release = self.start("first")
        self.wait_for(first_started)
        waiter, waiter_started, _ = self.start("waiter")
        self.wait_for_log("waiter", "next cargo slot reserved:")
        self.assertFalse(waiter_started.exists())
        waiter.terminate()
        waiter.wait(timeout=3)
        next_process, next_started, next_release = self.start("next")
        self.wait_for_log("next", "next cargo slot reserved:")
        first_release.touch()
        self.assertEqual(first.wait(timeout=3), 0)
        self.wait_for(next_started)
        next_release.touch()
        self.assertEqual(next_process.wait(timeout=3), 0)

    def test_reserved_waiter_precedes_newcomer(self):
        first, first_started, first_release = self.start("first")
        self.wait_for(first_started)
        waiter, waiter_started, waiter_release = self.start("waiter")
        self.wait_for_log("waiter", "next cargo slot reserved:")
        newcomer, newcomer_started, newcomer_release = self.start("newcomer")

        first_release.touch()
        self.assertEqual(first.wait(timeout=3), 0)
        self.wait_for(waiter_started)
        self.assertFalse(newcomer_started.exists())
        self.wait_for_log("newcomer", "next cargo slot reserved:")

        waiter_release.touch()
        self.assertEqual(waiter.wait(timeout=3), 0)
        self.wait_for(newcomer_started)
        newcomer_release.touch()
        self.assertEqual(newcomer.wait(timeout=3), 0)

    def test_timeout_releases_reservation_without_starting_child(self):
        first, first_started, first_release = self.start("first")
        self.wait_for(first_started)
        waiter, waiter_started, _ = self.start("waiter", wait_seconds=1)
        self.wait_for_log("waiter", "next cargo slot reserved:")
        self.wait_for_log("waiter", "timed out waiting for cargo slot")
        self.assertEqual(waiter.wait(timeout=3), 75)
        self.assertFalse(waiter_started.exists())

        newcomer, newcomer_started, newcomer_release = self.start("newcomer")
        self.wait_for_log("newcomer", "next cargo slot reserved:")
        first_release.touch()
        self.assertEqual(first.wait(timeout=3), 0)
        self.wait_for(newcomer_started)
        newcomer_release.touch()
        self.assertEqual(newcomer.wait(timeout=3), 0)

    def test_zero_timeout_fails_immediately_without_starting_child(self):
        first, first_started, first_release = self.start("first")
        self.wait_for(first_started)
        waiter, waiter_started, _ = self.start("waiter", wait_seconds=0)
        self.wait_for_log("waiter", "timed out waiting for cargo slot")
        self.assertEqual(waiter.wait(timeout=3), 75)
        self.assertFalse(waiter_started.exists())
        first_release.touch()
        self.assertEqual(first.wait(timeout=3), 0)

    def test_timeout_also_applies_behind_the_reserved_waiter(self):
        first, first_started, first_release = self.start("first")
        self.wait_for(first_started)
        waiter, waiter_started, waiter_release = self.start("waiter")
        self.wait_for_log("waiter", "next cargo slot reserved:")
        newcomer, newcomer_started, _ = self.start("newcomer", wait_seconds=0)
        self.assertEqual(newcomer.wait(timeout=3), 75)
        log = self.wait_for_log("newcomer", "timed out waiting for cargo slot")
        self.assertIn("slot.lock.admission", log)
        self.assertNotIn("next cargo slot reserved:", log)
        self.assertFalse(newcomer_started.exists())

        first_release.touch()
        self.assertEqual(first.wait(timeout=3), 0)
        self.wait_for(waiter_started)
        waiter_release.touch()
        self.assertEqual(waiter.wait(timeout=3), 0)

    def test_invalid_budgets_fail_before_opening_the_slot(self):
        for value in ("-1", "nan", "inf"):
            with self.subTest(value=value):
                result = subprocess.run(
                    [sys.executable, str(SCRIPT), "--wait-seconds", value,
                     sys.executable, "-c", "raise SystemExit(7)"],
                    env=self.env, capture_output=True, text=True, check=False,
                )
                self.assertEqual(result.returncode, 2)
                self.assertIn("finite nonnegative number", result.stderr)
                self.assertFalse(Path(self.env["HEX_CARGO_SLOT_FILE"]).exists())

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
