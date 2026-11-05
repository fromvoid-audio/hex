"""Exercise the launcher using fixture-owned git and cargo commands."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / "launch"
RUNNER = Path("scripts/run-desktop.sh")


class LaunchTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "checkout with spaces"
        self.root.mkdir()
        shutil.copy2(SCRIPT, self.root / "launch")
        (self.root / RUNNER.parent).mkdir()
        shutil.copy2(REPO / RUNNER, self.root / RUNNER)
        self.bin = Path(self.temp.name) / "bin"
        self.bin.mkdir()
        self.trace = Path(self.temp.name) / "trace"
        for name in ("git", "cargo"):
            fake = self.bin / name
            fake.write_text(
                '#!/bin/sh\nprintf "%s\\n" "' + name + '" "$(pwd)" "$@" "--end--" >> "$TRACE"\n'
                + ('if [ -n "${UPDATED_RUNNER:-}" ]; then mv "$UPDATED_RUNNER" scripts/run-desktop.sh; fi\n'
                   if name == "git" else '')
                + ('printf "%s\\n" "$HEX_LAUNCH_DISCOVERY_APP" "$HEX_LAUNCH_INSTANCE_APP" > "$TOOLS_TRACE"\n'
                   if name == "cargo" else '')
                + 'exit "${' + name.upper() + '_EXIT:-0}"\n'
            )
            fake.chmod(0o755)
        (self.bin / "uname").write_text(
            '#!/bin/sh\ncase "$1" in -s) echo Darwin;; -m) echo arm64;; esac\n'
        )
        (self.bin / "python3").write_text(
            '#!/bin/sh\nprintf "%s\\n" "$*" >> "$PREPARE_TRACE"\n'
            'if [ "$2" = --instance ]; then\n'
            '    [ "$INSTANCE_EXIT" = 1 ] && exit 1\n'
            '    printf "%s\\n" "$INSTANCE_APP"\n'
            '    exit\n'
            'fi\n'
            'if [ "${PREPARE_EXIT:-0}" != 0 ]; then echo "scanner unavailable" >&2; exit 1; fi\n'
            'printf "%s\\n" "$TOOLS_APP"\n'
        )
        for name in ("uname", "python3"):
            (self.bin / name).chmod(0o755)
        self.tools_trace = Path(self.temp.name) / "tools-trace"
        self.prepare_trace = Path(self.temp.name) / "prepare-trace"
        self.tools_app = self.root / "discovery tools" / "hex.app"
        self.instance_app = self.root / "instance tools" / "hex.app"
        self.env = dict(os.environ, PATH=f"{self.bin}:{os.environ['PATH']}", TRACE=str(self.trace),
                        TOOLS_TRACE=str(self.tools_trace), PREPARE_TRACE=str(self.prepare_trace),
                        TOOLS_APP=str(self.tools_app), INSTANCE_APP=str(self.instance_app),
                        HEX_LAUNCH_DISCOVERY_APP="", HEX_LAUNCH_INSTANCE_APP="")

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
        self.assertEqual(self.prepare_trace.read_text().splitlines(), [
            "scripts/prepare-launch-discovery.py",
            "scripts/prepare-launch-discovery.py --instance",
        ])
        self.assertEqual(self.tools_trace.read_text().splitlines(), [
            str(self.tools_app), str(self.instance_app),
        ])

    def test_runs_the_desktop_script_installed_by_pull(self):
        (self.root / RUNNER).write_text("#!/bin/sh\nexit 19\n")
        updated = self.root / "incoming runner"
        updated.write_text(
            '#!/bin/sh\nexport HEX_LAUNCH_INSTANCE_APP="$INSTANCE_APP"\n'
            'exec cargo run -r -- "$@"\n'
        )
        result = self.run_launch("path with spaces", UPDATED_RUNNER=str(updated))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.tools_trace.read_text().splitlines(), ["", str(self.instance_app)])
        self.assertEqual(self.trace.read_text().splitlines(), [
            "git", str(self.root), "pull", "--end--",
            "cargo", str(self.root), "run", "-r", "--", "path with spaces", "--end--",
        ])

    def test_pull_failure_stops_before_running_cargo(self):
        result = self.run_launch(GIT_EXIT="17")
        self.assertEqual(result.returncode, 17)
        self.assertEqual(self.trace.read_text().splitlines(), ["git", str(self.root), "pull", "--end--"])
        self.assertFalse(self.prepare_trace.exists())

    def test_prepare_failure_preserves_source_launch_and_clears_an_old_tools_path(self):
        result = self.run_launch(PREPARE_EXIT="1", HEX_LAUNCH_DISCOVERY_APP=str(self.tools_app))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("scanner unavailable", result.stderr)
        self.assertEqual(self.tools_trace.read_text().splitlines(), ["", str(self.instance_app)])

    def test_instance_failure_keeps_discovery_and_clears_an_old_instance_path(self):
        result = self.run_launch(INSTANCE_EXIT="1", HEX_LAUNCH_INSTANCE_APP=str(self.instance_app))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.tools_trace.read_text().splitlines(), [str(self.tools_app), ""])

    def test_cargo_failure_reaches_the_caller(self):
        result = self.run_launch(CARGO_EXIT="23")
        self.assertEqual(result.returncode, 23)
