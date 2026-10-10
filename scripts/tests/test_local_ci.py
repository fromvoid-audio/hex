"""Behavior checks for the local CI runner using fake executables."""

from __future__ import annotations

import importlib.util
import hashlib
import json
import os
from pathlib import Path
import shutil
import sys
import subprocess
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parents[1] / "local-ci.py"
SPEC = importlib.util.spec_from_file_location("local_ci", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("could not load local-ci.py")
local_ci = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = local_ci
SPEC.loader.exec_module(local_ci)


FAKE_TOOL = """#!/bin/sh
tool=${0##*/}
printf '%s %s [target=%s]\\n' "$tool" "$*" "$CARGO_TARGET_DIR" >> "$CALL_LOG"
case "$tool $*" in
  'git rev-parse --show-toplevel') echo "$FAKE_TOPLEVEL"; exit 0 ;;
  'git rev-parse --git-common-dir') echo "$FAKE_COMMON_DIR"; exit 0 ;;
  git\ rev-parse*) echo deadbeef; exit 0 ;;
  git\ status*) exit 0 ;;
  git\ ls-files*) printf '%s\\0' "$FAKE_FILE_LIST"; exit 0 ;;
  'cargo metadata'*)
    case "$FAIL_MATCH" in *metadata*) echo "metadata unavailable" >&2; exit 9 ;; esac
    printf '{"target_directory":"%s","workspace_root":"%s","workspace_members":["fixture 0.1.0","other 0.1.0"],"packages":[{"id":"fixture 0.1.0","name":"fixture","version":"0.1.0"},{"id":"other 0.1.0","name":"other","version":"0.1.0"}]}\\n' "$FAKE_METADATA_TARGET" "$FAKE_WORKSPACE_ROOT"
    exit 0 ;;
esac
case "$tool $*" in
  *"$FAIL_MATCH"*) echo "intentional failure"; exit 7 ;;
esac
if [ -n "$MUTATE_FILE" ] && [ "$tool" != git ]; then
  echo changed >> "$MUTATE_FILE"
fi
if [ "$MAKE_TARGET" = 1 ] && [ "$tool" = cargo ]; then
  mkdir -p "$CARGO_TARGET_DIR"
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
            "FAKE_TOPLEVEL": str(self.repo),
            "FAKE_COMMON_DIR": str(self.repo / ".git"),
            "FAKE_WORKSPACE_ROOT": str(self.repo),
            "FAKE_METADATA_TARGET": str(self.repo / "cargo-target"),
            "MAKE_TARGET": "0",
        }
        self.target_state_dir = self.base / "target-ownership"

    def tearDown(self) -> None:
        self.temp.cleanup()

    def summary(self, run_id: str = "case") -> dict[str, object]:
        return json.loads((self.repo / "artifacts/local-ci" / run_id / "summary.json").read_text())

    def runner(self, run_id: str, *, root: Path | None = None, env: dict[str, str] | None = None,
               steps: tuple[local_ci.Step, ...] | None = None, adopt_target: bool = False) -> local_ci.LocalCiRunner:
        selected_root = self.repo if root is None else root
        return local_ci.LocalCiRunner(
            selected_root,
            run_id=run_id,
            env=self.env if env is None else env,
            steps=steps or (local_ci.Step("build", ("cargo", "build")),),
            min_free_bytes=0,
            adopt_target=adopt_target,
            target_state_dir=self.target_state_dir,
        )

    def test_low_space_stops_before_tools_and_preserves_files(self) -> None:
        sentinel = self.repo / "keep.hex"
        sentinel.write_bytes(b"original user work")
        runner = local_ci.LocalCiRunner(self.repo, run_id="low-space", env=self.env,
                                        steps=(local_ci.Step("first", ("first",)),))
        with patch.object(local_ci.shutil, "disk_usage", return_value=SimpleNamespace(total=100 * local_ci.GIB, free=100)):
            self.assertEqual(runner.run(), 2)
        summary = self.summary("low-space")
        self.assertEqual(summary["status"], "failed")
        self.assertFalse(summary["stale"])
        self.assertEqual(summary["steps"][0]["status"], "not_run")
        self.assertIn("No files were removed", summary["disk_error"])
        self.assertNotIn("first", self.call_log.read_text())
        self.assertEqual(sentinel.read_bytes(), b"original user work")

    def test_explicit_target_mount_is_probed_separately(self) -> None:
        runner = local_ci.LocalCiRunner(self.repo, run_id="target-space",
            env={**self.env, "CARGO_TARGET_DIR": "separate/missing-target"},
            steps=(local_ci.Step("first", ("first",)),))
        available = SimpleNamespace(total=100 * local_ci.GIB, free=4 * local_ci.GIB)
        full = SimpleNamespace(total=100 * local_ci.GIB, free=100)
        with patch.object(local_ci.shutil, "disk_usage", side_effect=[available, full]):
            self.assertEqual(runner.run(), 2)
        summary = self.summary("target-space")
        probes = summary["disk_headroom"]["probes"]
        self.assertEqual([probe["role"] for probe in probes], ["workspace", "target"])
        self.assertEqual(probes[1]["requested_path"], str(self.repo.resolve() / "separate/missing-target"))
        self.assertEqual(probes[1]["probed_path"], str(self.repo.resolve()))
        self.assertFalse((self.repo / "separate").exists())

    def test_disk_probe_failure_retains_report_without_running_checks(self) -> None:
        runner = local_ci.LocalCiRunner(self.repo, run_id="probe-error", env=self.env,
                                        steps=(local_ci.Step("first", ("first",)),))
        with patch.object(local_ci.shutil, "disk_usage", side_effect=OSError("unavailable volume")):
            self.assertEqual(runner.run(), 2)
        summary = self.summary("probe-error")
        self.assertIn("unavailable volume", summary["disk_error"])
        self.assertEqual(summary["steps"][0]["status"], "not_run")

    def test_headroom_is_rechecked_before_heavy_steps(self) -> None:
        runner = local_ci.LocalCiRunner(self.repo, run_id="shrinking-space", env=self.env,
            steps=(local_ci.Step("first", ("first",)), local_ci.Step("heavy", ("cargo", "build"))))
        available = SimpleNamespace(total=100 * local_ci.GIB, free=4 * local_ci.GIB)
        full = SimpleNamespace(total=100 * local_ci.GIB, free=100)
        with patch.object(local_ci.shutil, "disk_usage", side_effect=[available, available, full]):
            self.assertEqual(runner.run(), 2)
        summary = self.summary("shrinking-space")
        self.assertEqual([step["status"] for step in summary["steps"]], ["passed", "not_run"])
        self.assertNotIn("cargo build", self.call_log.read_text())
        self.assertFalse(summary["stale"])

    def test_quick_headroom_floor_and_invalid_configuration(self) -> None:
        runner = local_ci.LocalCiRunner(self.repo, mode="quick", run_id="quick-space", env=self.env,
                                        steps=(local_ci.Step("first", ("first",)),))
        with patch.object(local_ci.shutil, "disk_usage", return_value=SimpleNamespace(total=1000, free=100)):
            self.assertEqual(runner.run(), 0)
        self.assertEqual(self.summary("quick-space")["status"], "partial")
        for value in (-1, 0.5, True, sys.maxsize + 1):
            with self.assertRaises(ValueError):
                local_ci.LocalCiRunner(self.repo, min_free_bytes=value)

    def test_quick_format_does_not_claim_or_clean_unknown_cargo_target(self) -> None:
        target = Path(self.env["FAKE_METADATA_TARGET"])
        sentinel = target / "debug/deps/foreign.rlib"
        sentinel.parent.mkdir(parents=True)
        sentinel.write_bytes(b"another process owns this cache")
        runner = local_ci.LocalCiRunner(
            self.repo,
            mode="quick",
            run_id="quick-format",
            env=self.env,
            steps=(local_ci.Step("format", ("cargo", "fmt", "--all", "--check")),),
            min_free_bytes=0,
            target_state_dir=self.target_state_dir,
        )
        self.assertEqual(runner.run(), 0)
        summary = self.summary("quick-format")
        self.assertNotIn("cargo_identity", summary)
        self.assertNotIn("target_cache", summary)
        self.assertEqual(sentinel.read_bytes(), b"another process owns this cache")
        self.assertFalse(list(self.target_state_dir.glob("*.owner.json")))

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

    def test_cargo_metadata_resolves_and_pins_exact_target_and_source_identity(self) -> None:
        configured = {**self.env, "CARGO_TARGET_DIR": "relative/ignored-by-metadata"}
        runner = self.runner("identity", env=configured)
        self.assertEqual(runner.run(), 0)
        summary = self.summary("identity")
        self.assertEqual(summary["source_identity"]["workspace_root"], str(self.repo.resolve()))
        self.assertEqual(summary["source_identity"]["git_common_dir"], str((self.repo / ".git").resolve()))
        cargo_identity = summary["cargo_identity"]
        expected_target = Path(self.env["FAKE_METADATA_TARGET"]).resolve()
        self.assertEqual(cargo_identity["target_directory"], str(expected_target))
        self.assertIn("--locked", cargo_identity["metadata_command"])
        self.assertEqual(cargo_identity["target_source"], "environment")
        self.assertEqual(cargo_identity["configured_target_value"], "relative/ignored-by-metadata")
        self.assertIn(f"[target={expected_target}]", self.call_log.read_text())

    def test_unknown_nonempty_target_fails_closed_without_removing_or_using_it(self) -> None:
        target = Path(self.env["FAKE_METADATA_TARGET"])
        sentinel = target / "debug/deps/foreign.rlib"
        sentinel.parent.mkdir(parents=True)
        sentinel.write_bytes(b"foreign cache")
        runner = self.runner("unknown")
        self.assertEqual(runner.run(), 1)
        summary = self.summary("unknown")
        self.assertIn("ownership is unknown", summary["target_cache_error"])
        self.assertEqual(sentinel.read_bytes(), b"foreign cache")
        calls = self.call_log.read_text()
        self.assertNotIn("cargo clean", calls)
        self.assertNotIn("cargo build", calls)

    def test_metadata_failure_is_recorded_and_blocks_build(self) -> None:
        runner = self.runner("metadata-fail", env={**self.env, "FAIL_MATCH": "metadata"})
        self.assertEqual(runner.run(), 2)
        summary = self.summary("metadata-fail")
        self.assertEqual(summary["status"], "failed")
        self.assertEqual(summary["cargo_metadata_error"]["exit_code"], 9)
        self.assertIn("metadata unavailable", summary["cargo_metadata_error"]["stderr"])
        self.assertNotIn("cargo build", self.call_log.read_text())

    def test_explicit_adoption_cleans_only_dev_workspace_packages(self) -> None:
        target = Path(self.env["FAKE_METADATA_TARGET"])
        sentinel = target / "debug/deps/foreign.rlib"
        sentinel.parent.mkdir(parents=True)
        sentinel.write_bytes(b"external dependency")
        runner = self.runner("adopt", adopt_target=True)
        self.assertEqual(runner.run(), 0)
        summary = self.summary("adopt")
        cache = summary["target_cache"]
        self.assertEqual(cache["action"], "adopted")
        self.assertEqual(cache["cleanup_status"], "passed")
        self.assertEqual(cache["cleanup_argv"][2:4], ["--profile", "dev"])
        self.assertEqual(cache["cleanup_argv"][-4:], [
            "--package", "fixture 0.1.0", "--package", "other 0.1.0",
        ])
        self.assertEqual(sentinel.read_bytes(), b"external dependency")
        self.assertIn(f"[target={target.resolve()}]", self.call_log.read_text())

    def test_same_worktree_reuses_target_without_cleanup(self) -> None:
        self.assertEqual(self.runner("same-a").run(), 0)
        self.assertEqual(self.runner("same-b").run(), 0)
        calls = self.call_log.read_text()
        self.assertEqual(calls.count("cargo clean"), 0)
        self.assertEqual(self.summary("same-b")["target_cache"]["action"], "reuse")

    def test_worktree_handoff_cleans_every_workspace_member(self) -> None:
        self.assertEqual(self.runner("handoff-a").run(), 0)
        worktree_b = self.base / "repo-worktree-b"
        worktree_b.mkdir()
        env_b = {
            **self.env,
            "FAKE_TOPLEVEL": str(worktree_b),
            "FAKE_COMMON_DIR": str((self.repo / ".git").resolve()),
            "FAKE_WORKSPACE_ROOT": str(worktree_b),
        }
        runner_b = self.runner("handoff-b", root=worktree_b, env=env_b)
        self.assertEqual(runner_b.run(), 0)
        summary = json.loads(
            (worktree_b / "artifacts/local-ci/handoff-b/summary.json").read_text()
        )
        cache = summary["target_cache"]
        self.assertEqual(cache["action"], "worktree-handoff")
        self.assertEqual(cache["cleanup_status"], "passed")
        self.assertEqual(
            [package["spec"] for package in summary["cargo_identity"]["workspace_packages"]],
            ["fixture 0.1.0", "other 0.1.0"],
        )
        calls = self.call_log.read_text()
        self.assertIn("cargo clean", calls)
        self.assertIn("--package fixture 0.1.0 --package other 0.1.0", calls)

    def test_replaced_target_fails_closed_before_cross_worktree_cleanup(self) -> None:
        target = Path(self.env["FAKE_METADATA_TARGET"]).resolve()
        target.mkdir(parents=True)
        env_a = {**self.env, "MAKE_TARGET": "1"}
        self.assertEqual(self.runner("replace-a", env=env_a).run(), 0)
        sentinel = target / "foreign-preserve.me"
        sentinel.write_text("do not erase", encoding="utf-8")
        old_target = self.base / "old-target"
        target.rename(old_target)
        target.mkdir()
        (target / "foreign-preserve.me").write_text("new unowned target", encoding="utf-8")

        worktree_b = self.base / "repo-worktree-b"
        worktree_b.mkdir()
        env_b = {
            **self.env,
            "FAKE_TOPLEVEL": str(worktree_b),
            "FAKE_COMMON_DIR": str((self.repo / ".git").resolve()),
            "FAKE_WORKSPACE_ROOT": str(worktree_b),
        }
        calls_before_b = self.call_log.read_text()
        self.assertEqual(self.runner("replace-b", root=worktree_b, env=env_b).run(), 1)
        summary = json.loads((worktree_b / "artifacts/local-ci/replace-b/summary.json").read_text())
        self.assertIn("directory identity changed", summary["target_cache_error"])
        self.assertEqual((target / "foreign-preserve.me").read_text(encoding="utf-8"), "new unowned target")
        calls = self.call_log.read_text()[len(calls_before_b):]
        self.assertNotIn("cargo clean", calls)
        self.assertNotIn("cargo build", calls)

    def test_failed_handoff_clean_keeps_previous_owner_and_skips_build(self) -> None:
        self.assertEqual(self.runner("clean-fail-a").run(), 0)
        previous_owner = json.loads(next(self.target_state_dir.glob("*.owner.json")).read_text())
        calls_before_handoff = self.call_log.read_text()
        worktree_b = self.base / "repo-worktree-b"
        worktree_b.mkdir()
        env_b = {
            **self.env,
            "FAIL_MATCH": "clean",
            "FAKE_TOPLEVEL": str(worktree_b),
            "FAKE_COMMON_DIR": str((self.repo / ".git").resolve()),
            "FAKE_WORKSPACE_ROOT": str(worktree_b),
        }
        runner_b = self.runner("clean-fail-b", root=worktree_b, env=env_b)
        self.assertEqual(runner_b.run(), 1)
        current_owner = json.loads(next(self.target_state_dir.glob("*.owner.json")).read_text())
        self.assertEqual(current_owner, previous_owner)
        summary = json.loads(
            (worktree_b / "artifacts/local-ci/clean-fail-b/summary.json").read_text()
        )
        self.assertEqual(summary["target_cache"]["cleanup_status"], "failed")
        calls = self.call_log.read_text()[len(calls_before_handoff):]
        self.assertIn("cargo clean", calls)
        self.assertNotIn("cargo build", calls)

    def test_target_lock_contention_refuses_before_cargo_build(self) -> None:
        target = Path(self.env["FAKE_METADATA_TARGET"]).resolve()
        lock_path, _ = local_ci.target_state_paths(target, self.target_state_dir)
        held = local_ci.CargoTargetLock(
            lock_path,
            {"run_id": "holder", "workspace_root": str(self.repo), "target_directory": str(target)},
        )
        held.acquire()
        try:
            self.assertEqual(self.runner("contended").run(), 1)
        finally:
            held.release()
        self.assertTrue(lock_path.exists(), "persistent lock inode must not be unlinked on release")
        summary = self.summary("contended")
        self.assertIn("already in use", summary["target_cache_error"])
        calls = self.call_log.read_text()
        self.assertNotIn("cargo build", calls)

    def test_lock_failures_release_the_file_and_preserve_useful_errors(self) -> None:
        target = Path(self.env["FAKE_METADATA_TARGET"]).resolve()
        path, _ = local_ci.target_state_paths(target, self.target_state_dir)
        denied = local_ci.CargoTargetLock(path, {"run_id": "denied"})
        with patch.object(local_ci.fcntl, "flock", side_effect=OSError("lock denied")):
            with self.assertRaisesRegex(local_ci.TargetCacheError, "lock denied"):
                denied.acquire()
        self.assertIsNone(denied.file)

        failed_holder = local_ci.CargoTargetLock(path, {"run_id": "write-failed"})
        with patch.object(failed_holder, "_write", side_effect=OSError("holder write denied")):
            with self.assertRaisesRegex(local_ci.TargetCacheError, "holder write denied"):
                failed_holder.acquire()
        self.assertIsNone(failed_holder.file)

        with patch.object(local_ci.CargoTargetLock, "_write", side_effect=OSError("runner holder write denied")):
            self.assertEqual(self.runner("runner-lock-write-failed").run(), 1)
        failed_summary = self.summary("runner-lock-write-failed")
        self.assertIn("runner holder write denied", failed_summary["target_cache_error"])

        holder = local_ci.CargoTargetLock(path, {"run_id": "release-write-failed"})
        holder.acquire()
        with patch.object(holder, "_write", side_effect=OSError("release write denied")):
            release_error = holder.release()
        self.assertIn("release write denied", release_error or "")
        next_holder = local_ci.CargoTargetLock(path, {"run_id": "after-release-failure"})
        next_holder.acquire()
        next_holder.release()

        original_release = local_ci.CargoTargetLock.release

        def release_with_unwritable_status(lock: local_ci.CargoTargetLock) -> str | None:
            with patch.object(lock, "_write", side_effect=OSError("runner release write denied")):
                return original_release(lock)

        with patch.object(local_ci.CargoTargetLock, "release", release_with_unwritable_status):
            self.assertEqual(self.runner("runner-lock-release-failed").run(), 0)
        release_summary = self.summary("runner-lock-release-failed")
        self.assertIn("runner release write denied", release_summary["target_lock_release_error"])

    def test_malformed_owner_marker_fails_closed_without_adoption(self) -> None:
        target = Path(self.env["FAKE_METADATA_TARGET"]).resolve()
        target.mkdir(parents=True)
        (target / "debug").mkdir()
        _, owner_path = local_ci.target_state_paths(target, self.target_state_dir)
        owner_path.parent.mkdir(parents=True)
        owner_path.write_text("{malformed", encoding="utf-8")
        self.assertEqual(self.runner("bad-owner").run(), 1)
        summary = self.summary("bad-owner")
        self.assertIn("marker is malformed", summary["target_cache_error"])
        self.assertNotIn("cargo clean", self.call_log.read_text())
        self.assertNotIn("cargo build", self.call_log.read_text())

    @unittest.skipUnless(shutil.which("cargo") and shutil.which("git"), "requires real Cargo and Git")
    def test_real_cargo_worktree_handoff_cleans_only_workspace_dev_artifacts(self) -> None:
        """Exercise the API-mismatch handoff against Cargo in an isolated temporary target."""
        root = self.base / "real-workspace"
        api = root / "crates/fixture-api"
        app = root / "crates/fixture-app"
        helper = root / "external-helper"
        for directory in (api / "src", app / "src", helper / "src"):
            directory.mkdir(parents=True)
        (root / "Cargo.toml").write_text(
            '[workspace]\nmembers = ["crates/*"]\nexclude = ["external-helper"]\n'
            'resolver = "2"\n',
            encoding="utf-8",
        )
        (root / ".gitignore").write_text("/artifacts/\n", encoding="utf-8")
        (api / "Cargo.toml").write_text(
            '[package]\nname = "fixture-api"\nversion = "0.1.0"\nedition = "2021"\n',
            encoding="utf-8",
        )
        (api / "src/lib.rs").write_text("pub fn value() -> u32 { 7 }\n", encoding="utf-8")
        (helper / "Cargo.toml").write_text(
            '[package]\nname = "external-helper"\nversion = "0.1.0"\nedition = "2021"\n',
            encoding="utf-8",
        )
        (helper / "src/lib.rs").write_text("pub fn helper() -> u32 { 1 }\n", encoding="utf-8")
        (app / "Cargo.toml").write_text(
            '[package]\nname = "fixture-app"\nversion = "0.1.0"\nedition = "2021"\n'
            '[dependencies]\nfixture-api = { path = "../fixture-api" }\n'
            'external-helper = { path = "../../external-helper" }\n',
            encoding="utf-8",
        )
        (app / "src/main.rs").write_text(
            "fn main() { println!(\"{}\", fixture_api::value() + external_helper::helper()); }\n",
            encoding="utf-8",
        )

        def git(*args: str, cwd: Path = root) -> str:
            result = subprocess.run(
                ["git", *args], cwd=cwd, capture_output=True, text=True, check=False
            )
            if result.returncode != 0:
                raise AssertionError(f"git {' '.join(args)} failed: {result.stderr}")
            return result.stdout.strip()

        git("init", "-q")
        git("config", "user.email", "local-ci-test@example.invalid")
        git("config", "user.name", "local ci fixture")
        locked = subprocess.run(
            ["cargo", "generate-lockfile", "--offline", "--manifest-path", str(root / "Cargo.toml")],
            cwd=root,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(locked.returncode, 0, locked.stderr)
        git("add", ".")
        git("commit", "-qm", "fixture base")

        worktree_b = self.base / "real-worktree-b"
        git("worktree", "add", "-qb", "fixture-b", str(worktree_b))
        (worktree_b / "crates/fixture-api/src/lib.rs").write_text(
            "pub fn value() -> &'static str { \"changed api\" }\n", encoding="utf-8"
        )
        (worktree_b / "crates/fixture-app/src/main.rs").write_text(
            "fn main() { println!(\"{} {}\", fixture_api::value(), external_helper::helper()); }\n",
            encoding="utf-8",
        )
        git("add", ".", cwd=worktree_b)
        git("commit", "-qm", "change workspace api", cwd=worktree_b)

        target = self.base / "isolated-cargo-target"
        state = self.base / "real-target-ownership"
        env = dict(os.environ)
        env.update({"CARGO_TARGET_DIR": str(target), "CARGO_INCREMENTAL": "0"})
        real_cargo = shutil.which("cargo")
        self.assertIsNotNone(real_cargo)
        steps = (
            local_ci.Step("debug-build", ("cargo", "build", "--offline", "--locked")),
            local_ci.Step("release-build", ("cargo", "build", "--offline", "--locked", "--release")),
        )
        run_a = local_ci.LocalCiRunner(
            root, run_id="real-a", env=env, steps=steps, min_free_bytes=0, target_state_dir=state
        )
        self.assertEqual(run_a.run(), 0)
        release_binary = target / "release/fixture-app"
        release_digest = hashlib.sha256(release_binary.read_bytes()).hexdigest()
        helper_artifacts = sorted((target / "debug/deps").glob("libexternal_helper-*.rlib"))
        self.assertTrue(helper_artifacts, "external path dependency artifact was not produced")
        helper_digests = {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in helper_artifacts}
        api_artifacts = sorted((target / "debug/deps").glob("libfixture_api-*.rlib"))
        self.assertTrue(api_artifacts, "old workspace API artifact was not produced")

        wrapper_dir = self.base / "cargo-wrapper"
        wrapper_dir.mkdir()
        cargo_wrapper = wrapper_dir / "cargo"
        cargo_wrapper.write_text(
            "#!/usr/bin/env python3\n"
            "import glob, os, subprocess, sys\n"
            f"real = {str(real_cargo)!r}\n"
            "if os.environ.get('ASSERT_WORKSPACE_CLEAN') == '1' and sys.argv[1:2] == ['build']:\n"
            "    stale = glob.glob(os.path.join(os.environ['CARGO_TARGET_DIR'], 'debug/deps/libfixture_api-*.rlib'))\n"
            "    if stale:\n"
            "        print('stale workspace API artifacts survived handoff: ' + ', '.join(stale), file=sys.stderr)\n"
            "        raise SystemExit(23)\n"
            "os.execv(real, [real, *sys.argv[1:]])\n",
            encoding="utf-8",
        )
        cargo_wrapper.chmod(0o755)
        env_b_runtime = {
            **env,
            "PATH": f"{wrapper_dir}{os.pathsep}{env.get('PATH', '')}",
            "ASSERT_WORKSPACE_CLEAN": "1",
        }

        run_b = local_ci.LocalCiRunner(
            worktree_b,
            run_id="real-b",
            env=env_b_runtime,
            steps=(steps[0],),
            min_free_bytes=0,
            target_state_dir=state,
        )
        self.assertEqual(run_b.run(), 0)
        summary_b = json.loads((worktree_b / "artifacts/local-ci/real-b/summary.json").read_text())
        self.assertEqual(summary_b["target_cache"]["action"], "worktree-handoff")
        self.assertEqual(summary_b["target_cache"]["cleanup_status"], "passed")
        self.assertIn("--profile", summary_b["target_cache"]["cleanup_argv"])
        self.assertEqual(summary_b["target_cache"]["cleanup_argv"][2], "--profile")
        self.assertEqual(summary_b["target_cache"]["cleanup_argv"][3], "dev")
        self.assertEqual(summary_b["target_cache"]["cleanup_argv"][5], str(target.resolve()))
        cleanup_argv = summary_b["target_cache"]["cleanup_argv"]
        cleanup_packages = [cleanup_argv[index + 1] for index, arg in enumerate(cleanup_argv[:-1]) if arg == "--package"]
        self.assertTrue(any("/fixture-api#0.1.0" in package for package in cleanup_packages), cleanup_packages)
        self.assertTrue(any("/fixture-app#0.1.0" in package for package in cleanup_packages), cleanup_packages)
        cleanup_log = worktree_b / summary_b["target_cache"]["cleanup_log"]
        self.assertIn("Removed", cleanup_log.read_text(encoding="utf-8"))
        self.assertEqual(
            hashlib.sha256(release_binary.read_bytes()).hexdigest(), release_digest,
            "dev handoff cleanup changed the release artifact",
        )
        surviving_helpers = {path.name: hashlib.sha256(path.read_bytes()).hexdigest()
                             for path in (target / "debug/deps").glob("libexternal_helper-*.rlib")}
        self.assertEqual(surviving_helpers, helper_digests,
                         "dev handoff cleanup changed an external path dependency artifact")
        print(
            "real cargo handoff: workspace dev outputs rebuilt; external path dependency and "
            f"release binary preserved (target={target})",
            flush=True,
        )


if __name__ == "__main__":
    unittest.main()
