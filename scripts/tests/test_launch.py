"""Exercise the launcher using fixture-owned git and cargo commands."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[2] / "launch"


class LaunchTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "checkout with spaces"
        self.root.mkdir()
        shutil.copy2(SCRIPT, self.root / "launch")
        self.bin = Path(self.temp.name) / "bin"
        self.bin.mkdir()
        self.trace = Path(self.temp.name) / "trace"
        for name in ("git", "cargo"):
            fake = self.bin / name
            fake.write_text(
                '#!/bin/sh\nprintf "%s\\n" "' + name + '" "$(pwd)" "$@" "--end--" >> "$TRACE"\n'
                'exit "${' + name.upper() + '_EXIT:-0}"\n'
            )
            fake.chmod(0o755)
        self.env = dict(os.environ, PATH=f"{self.bin}:{os.environ['PATH']}", TRACE=str(self.trace))

    def run_launch(self, *args, **env):
        return subprocess.run([str(self.root / "launch"), *args], cwd=self.bin,
                              env=dict(self.env, **env), capture_output=True, text=True)

    def test_pulls_then_runs_release_and_forwards_arguments_in_checkout(self):
        result = self.run_launch("--diagnostics=off", "path with spaces")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.trace.read_text().splitlines(), [
            "git", str(self.root), "pull", "--end--",
            "cargo", str(self.root), "run", "-r", "--", "--diagnostics=off", "path with spaces", "--end--",
        ])

    def test_pull_failure_stops_before_running_cargo(self):
        result = self.run_launch(GIT_EXIT="17")
        self.assertEqual(result.returncode, 17)
        self.assertEqual(self.trace.read_text().splitlines(), ["git", str(self.root), "pull", "--end--"])

    def test_cargo_failure_reaches_the_caller(self):
        result = self.run_launch(CARGO_EXIT="23")
        self.assertEqual(result.returncode, 23)
