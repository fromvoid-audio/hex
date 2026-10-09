"""Behavior checks for the local CI runner using fake executables."""

import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "local-ci.py"
SPEC = importlib.util.spec_from_file_location("local_ci", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("could not load local-ci.py")
local_ci = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = local_ci
SPEC.loader.exec_module(local_ci)


FAKE_TOOL = """#!/bin/sh
tool=${0##*/}
printf '%s %s\\n' "$tool" "$*" >> "$CALL_LOG"
case "$tool $*" in
  git\ rev-parse*) echo deadbeef; exit 0 ;;
  git\ status*) exit 0 ;;
  git\ ls-files*) printf '%s\\0' "$FAKE_FILE_LIST"; exit 0 ;;
esac
case "$tool $*" in
  *"$FAIL_MATCH"*) echo "intentional failure"; exit 7 ;;
esac
if [ -n "$MUTATE_FILE" ] && [ "$tool" != git ]; then
  echo changed >> "$MUTATE_FILE"
fi
if [ "$INTERRUPT_ON" = 1 ] && [ "$tool" != git ]; then
  kill -TERM "$PPID"
fi
echo "completed $tool $*"
"""


class LocalCiRunnerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.base = Path(self.temp.name)
        self.repo = self.base / "repo"
        self.repo.mkdir()
        self.bin = self.base / "bin"
        self.bin.mkdir()
        self.call_log = self.base / "calls.log"
        for name in ("git", "python3", "cargo", "first", "second", "bad"):
            tool = self.bin / name
            tool.write_text(FAKE_TOOL)
            tool.chmod(0o755)
        self.env = {
            "PATH": str(self.bin),
            "CALL_LOG": str(self.call_log),
            "FAIL_MATCH": "never-match",
            "MUTATE_FILE": "",
            "INTERRUPT_ON": "0",
            "FAKE_FILE_LIST": "",
        }

    def tearDown(self) -> None:
        self.temp.cleanup()

    def summary(self, run_id: str = "case") -> dict[str, object]:
        return json.loads((self.repo / "artifacts/local-ci" / run_id / "summary.json").read_text())

    def test_quick_success_records_commit_commands_and_per_step_logs_as_partial(self) -> None:
        runner = local_ci.LocalCiRunner(
            self.repo,
            mode="quick",
            run_id="success",
            env=self.env,
            steps=(local_ci.Step("first", ("first", "--flag")), local_ci.Step("second", ("second",))),
        )
        self.assertEqual(runner.run(), 0)
        summary = self.summary("success")
        self.assertEqual(summary["status"], "partial")
        self.assertEqual(summary["commit"], "deadbeef")
        self.assertFalse(summary["dirty"])
        self.assertEqual(summary["repository_commands"][0], ["git", "rev-parse", "HEAD"])
        self.assertEqual([step["argv"] for step in summary["steps"]], [["first", "--flag"], ["second"]])
        for step in summary["steps"]:
            self.assertEqual(step["status"], "passed")
            self.assertIn("completed", (self.repo / step["log"]).read_text())

    def test_failure_stops_pipeline_and_marks_remaining_step_not_run(self) -> None:
        runner = local_ci.LocalCiRunner(
            self.repo,
            run_id="failure",
            env={**self.env, "FAIL_MATCH": "bad"},
            steps=(local_ci.Step("bad", ("bad",)), local_ci.Step("later", ("second",))),
        )
        self.assertEqual(runner.run(), 1)
        summary = self.summary("failure")
        self.assertEqual(summary["status"], "failed")
        self.assertEqual([step["status"] for step in summary["steps"]], ["failed", "not_run"])
        self.assertEqual(summary["steps"][0]["exit_code"], 7)
        self.assertIn("intentional failure", (self.repo / summary["steps"][0]["log"]).read_text())

    def test_missing_executable_is_logged_as_failure(self) -> None:
        runner = local_ci.LocalCiRunner(
            self.repo,
            run_id="missing",
            env=self.env,
            steps=(local_ci.Step("missing", ("not-installed",)),),
        )
        self.assertEqual(runner.run(), 1)
        summary = self.summary("missing")
        self.assertEqual(summary["steps"][0]["status"], "failed")
        self.assertIsNone(summary["steps"][0]["exit_code"])
        self.assertIn("not-installed", (self.repo / summary["steps"][0]["log"]).read_text())

    def test_source_change_during_run_marks_evidence_stale(self) -> None:
        source = self.repo / "source.rs"
        source.write_text("before\n")
        runner = local_ci.LocalCiRunner(
            self.repo,
            run_id="stale",
            env={**self.env, "MUTATE_FILE": str(source), "FAKE_FILE_LIST": "source.rs"},
            steps=(local_ci.Step("mutate", ("first",)),),
        )
        self.assertEqual(runner.run(), 3)
        summary = self.summary("stale")
        self.assertEqual(summary["status"], "stale")
        self.assertEqual(summary["verification_status"], "passed")
        self.assertNotEqual(summary["start_tree"]["content_sha256"], summary["end_tree"]["content_sha256"])

    def test_interruption_records_exit_time_and_skipped_steps(self) -> None:
        runner = local_ci.LocalCiRunner(
            self.repo,
            run_id="interrupted",
            env={**self.env, "INTERRUPT_ON": "1"},
            steps=(local_ci.Step("interrupt", ("first",)), local_ci.Step("later", ("second",))),
        )
        self.assertEqual(runner.run(), 130)
        summary = self.summary("interrupted")
        self.assertEqual(summary["status"], "interrupted")
        self.assertEqual(summary["exit_code"], 130)
        self.assertEqual(summary["steps"][0]["status"], "interrupted")
        self.assertEqual(summary["steps"][0]["exit_code"], 130)
        self.assertIsNotNone(summary["steps"][0]["ended_at"])
        self.assertEqual(summary["steps"][1]["status"], "not_run")


if __name__ == "__main__":
    unittest.main()
