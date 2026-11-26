#!/usr/bin/env python3
"""Unit tests for scripts/hex-auto-stage.py."""
from __future__ import annotations

import fcntl
import importlib.util
import json
from pathlib import Path
import plistlib
import tempfile
import unittest

SCRIPT_PATH = Path(__file__).resolve().parent.parent / "hex-auto-stage.py"
SPEC = importlib.util.spec_from_file_location("hex_auto_stage", SCRIPT_PATH)
assert SPEC is not None and SPEC.loader is not None
auto_stage = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(auto_stage)

SHA_OLD = "a" * 40
SHA_NEW = "b" * 40


class HexAutoStageTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory(prefix="hex-auto-stage-test-")
        self.root = Path(self._tmp.name)
        self.app = self.root / "artifacts" / "hex.app"
        (self.app / "Contents" / "Resources").mkdir(parents=True)
        self.state_file = self.root / "state" / "auto-stage-state.json"
        self.worktree = self.root / "worktree"

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def test_refuses_when_update_lease_lock_is_held(self) -> None:
        lock_path = self.app.parent / ".hex-test-build.lock"
        lock_path.write_text("", encoding="utf-8")
        with lock_path.open("r+") as handle:
            fcntl.flock(handle.fileno(), fcntl.LOCK_SH | fcntl.LOCK_NB)
            try:
                result = auto_stage.stage_update(
                    repo_root=self.root,
                    app_path=self.app,
                    state_file=self.state_file,
                    worktree_dir=self.worktree,
                    ps_runner=lambda: "",
                    git_runner=lambda _argv: SHA_NEW,
                )
                self.assertEqual(result["action"], "skipped_running")
                self.assertIn(".hex-test-build.lock", result["reason"])
            finally:
                fcntl.flock(handle.fileno(), fcntl.LOCK_UN)

    def test_refuses_when_hex_process_is_running(self) -> None:
        ps_output = f"1234 {self.app}/Contents/MacOS/hex\n"
        result = auto_stage.stage_update(
            repo_root=self.root,
            app_path=self.app,
            state_file=self.state_file,
            worktree_dir=self.worktree,
            ps_runner=lambda: ps_output,
            git_runner=lambda _argv: SHA_NEW,
        )
        self.assertEqual(result["action"], "skipped_running")
        self.assertIn("1234", result["reason"])

    def test_up_to_date_when_remote_matches_staged_or_installed_sha(self) -> None:
        (self.app / "Contents" / "Resources" / "test-build.json").write_text(
            json.dumps({"source": SHA_OLD}), encoding="utf-8"
        )
        result = auto_stage.stage_update(
            repo_root=self.root,
            app_path=self.app,
            state_file=self.state_file,
            worktree_dir=self.worktree,
            ps_runner=lambda: "",
            git_runner=lambda _argv: SHA_OLD,
        )
        self.assertEqual(result["action"], "up_to_date")
        self.assertEqual(result["target_sha"], SHA_OLD)

    def test_dry_run_plans_ci_and_prepare_only_without_executing(self) -> None:
        auto_stage.save_state(self.state_file, {"last_staged_sha": SHA_OLD})
        commands: list[list[str]] = []

        result = auto_stage.stage_update(
            repo_root=self.root,
            app_path=self.app,
            state_file=self.state_file,
            worktree_dir=self.worktree,
            dry_run=True,
            ps_runner=lambda: "",
            git_runner=lambda argv: SHA_NEW if argv[1] == "rev-parse" else "",
            cmd_runner=lambda argv, _cwd: commands.append(argv),
        )
        self.assertEqual(result["action"], "dry_run_plan")
        self.assertEqual(result["target_sha"], SHA_NEW)
        self.assertIn("--prepare-only", result["bundle_command"])
        self.assertEqual(commands, [])
        self.assertEqual(auto_stage.load_state(self.state_file)["last_staged_sha"], SHA_OLD)

    def test_full_stage_runs_worktree_ci_bundle_and_persists_state(self) -> None:
        executed: list[tuple[list[str], Path]] = []

        def fake_git(argv: list[str]) -> str:
            if argv[1] == "rev-parse":
                return SHA_NEW
            if argv[1] == "log":
                return "feat: closed-loop reporting (#420)"
            return ""

        def fake_cmd(argv: list[str], cwd: Path) -> None:
            executed.append((argv, cwd))

        result = auto_stage.stage_update(
            repo_root=self.root,
            app_path=self.app,
            state_file=self.state_file,
            worktree_dir=self.worktree,
            ps_runner=lambda: "",
            git_runner=fake_git,
            cmd_runner=fake_cmd,
        )
        self.assertEqual(result["action"], "staged")
        self.assertEqual(result["target_sha"], SHA_NEW)
        self.assertEqual(auto_stage.load_state(self.state_file)["last_staged_sha"], SHA_NEW)
        # Expect worktree add, local-ci.py --profile full, and bundle-macos.sh --prepare-only
        self.assertEqual(len(executed), 3)
        self.assertEqual(executed[0][0][:3], ["git", "worktree", "add"])
        self.assertIn("local-ci.py", executed[1][0][1])
        self.assertIn("bundle-macos.sh", executed[2][0][0])
        self.assertIn("--prepare-only", executed[2][0])

    def test_print_launchd_plist_emits_valid_xml(self) -> None:
        raw = auto_stage.render_launchd_plist(
            SCRIPT_PATH, repo_root=self.root, interval_secs=600
        )
        parsed = plistlib.loads(raw)
        self.assertEqual(parsed["Label"], auto_stage.LAUNCHD_LABEL)
        self.assertEqual(parsed["StartInterval"], 600)
        self.assertTrue(parsed["LowPriorityIO"])
        self.assertIn(str(SCRIPT_PATH.resolve()), parsed["ProgramArguments"])


if __name__ == "__main__":
    unittest.main()
