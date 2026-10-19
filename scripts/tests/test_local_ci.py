"""Behavior checks for the local CI runner using fake executables."""

from __future__ import annotations

import importlib.util
import ast
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

    def release_attempt(self, run_id, *, adopt=False):
        runner = self.runner(run_id)
        with runner.guarded_release(adopt=adopt) as report:
            target = Path(runner.cargo_identity["target_directory"])
            target.mkdir(exist_ok=True)
            artifact = target / "release-fixture"
            artifact.write_bytes(b"optimized fixture")
            argv = ["cargo", "build", "--release"]
            report["build_argv"] = argv
            report["build_exit_code"] = 0
            report["artifact"] = runner.identify_release_artifact(artifact, argv)
        return runner, report

    def test_release_unknown_pending_and_explicit_recovery(self):
        target = self.repo / "cargo-target"
        target.mkdir()
        (target / "untouched").write_bytes(b"history")
        with self.assertRaisesRegex(local_ci.TargetCacheError, "ownership is unknown"):
            self.release_attempt("unknown")
        runner, report = self.release_attempt("adopted", adopt=True)
        self.assertEqual(report["cleanup_argv"][3], "release")
        self.assertEqual((target / "untouched").read_bytes(), b"history")
        owner = Path(report["owner_path"])
        record = json.loads(owner.read_text())
        record["state"] = "pending"
        owner.write_text(json.dumps(record))
        with self.assertRaisesRegex(local_ci.TargetCacheError, "pending"):
            self.release_attempt("pending")
        self.release_attempt("recovered", adopt=True)
        self.assertIsNone(runner.target_lock)

    def test_release_cleanup_failure_and_interruption_keep_pending(self):
        _, report = self.release_attempt("first")
        owner = Path(report["owner_path"])
        failing = self.runner("failed")
        failing.env["FAIL_MATCH"] = "clean"
        with self.assertRaisesRegex(local_ci.TargetCacheError, "cleanup failed"):
            with failing.guarded_release(adopt=True):
                self.fail("failed cleanup entered build")
        self.assertEqual(json.loads(owner.read_text())["state"], "pending")
        interrupted = self.runner("interrupted")
        with patch.object(local_ci.subprocess, "run", wraps=subprocess.run) as command:
            original = command._mock_wraps
            def invoke(argv, **kwargs):
                if argv[:2] == ["cargo", "clean"]:
                    raise local_ci.InterruptedRun()
                return original(argv, **kwargs)
            command.side_effect = invoke
            with self.assertRaises(local_ci.InterruptedRun):
                with interrupted.guarded_release(adopt=True):
                    self.fail("interrupted cleanup entered build")
        self.assertEqual(json.loads(owner.read_text())["state"], "pending")
        self.assertIsNone(interrupted.target_lock)
        self.release_attempt("after-interruption", adopt=True)

    def test_release_and_dev_contend_on_same_lock_and_summary_failure_unlocks(self):
        runner = self.runner("holder")
        with self.assertRaisesRegex(local_ci.TargetCacheError, "identification"):
            with runner.guarded_release():
                self.assertEqual(self.runner("denied-dev").run(), 1)
                with self.assertRaisesRegex(local_ci.TargetCacheError, "already in use"):
                    with self.runner("denied-release").guarded_release():
                        self.fail("contender acquired target")
        other = self.runner("summary-failure")
        with patch.object(other, "_write_summary", side_effect=OSError("summary unavailable")):
            with self.assertRaisesRegex(OSError, "summary unavailable"):
                with other.guarded_release(adopt=True):
                    raise ValueError("build refused")
        self.assertIsNone(other.target_lock)
        self.release_attempt("after-summary-failure", adopt=True)

    def test_release_malformed_recreated_target_and_changed_source_refuse(self):
        runner, report = self.release_attempt("first")
        owner = Path(report["owner_path"])
        owner.write_text('{"schema_version":1}')
        with self.assertRaisesRegex(local_ci.TargetCacheError, "malformed"):
            self.release_attempt("malformed")
        _, report = self.release_attempt("recover", adopt=True)
        target = self.repo / "cargo-target"
        target.rename(self.repo / "retained-target")
        target.mkdir()
        with self.assertRaisesRegex(local_ci.TargetCacheError, "identity"):
            self.release_attempt("recreated")
        self.release_attempt("recreated-recover", adopt=True)
        modified = self.runner("modified")
        original = modified._capture_repository_state
        def capture(key):
            result = original(key)
            if key == "release_end_tree":
                modified.summary[key]["commit"] = "changed"
            return result
        with patch.object(modified, "_capture_repository_state", side_effect=capture):
            with self.assertRaisesRegex(local_ci.TargetCacheError, "source changed"):
                with modified.guarded_release() as report:
                    artifact = target / "release-fixture"
                    report["build_argv"] = ["cargo", "build", "--release"]
                    report["build_exit_code"] = 0
                    report["artifact"] = modified.identify_release_artifact(artifact, report["build_argv"])
        self.assertEqual(json.loads(owner.read_text())["state"], "pending")

    def test_release_external_name_collision_refuses_before_marker_or_cleanup(self):
        runner = self.runner("collision")
        original = local_ci.command_output
        def command(argv, cwd, env):
            result = original(argv, cwd, env)
            if argv[:2] == ["cargo", "metadata"] and "--no-deps" not in argv:
                metadata = json.loads(result.stdout)
                metadata["packages"].append({"id":"external fixture 0.2.0", "name":"fixture", "version":"0.2.0"})
                result.stdout = json.dumps(metadata)
            return result
        with patch.object(local_ci, "command_output", side_effect=command):
            with self.assertRaisesRegex(local_ci.TargetCacheError, "overlap"):
                with runner.guarded_release():
                    self.fail("ambiguous clean selection accepted")
        self.assertFalse(list(self.target_state_dir.glob("*.release-owner.json")))
        self.assertNotIn("cargo clean", self.call_log.read_text())

    def test_release_artifact_mutation_and_consumer_failure_stay_pending(self):
        runner = self.runner("mutated-artifact")
        with self.assertRaisesRegex(local_ci.TargetCacheError, "artifact changed"):
            with runner.guarded_release() as report:
                target = Path(runner.cargo_identity["target_directory"])
                target.mkdir()
                artifact = target / "fixture"
                artifact.write_bytes(b"original")
                report["build_argv"] = ["cargo", "build", "--release"]
                report["build_exit_code"] = 0
                report["artifact"] = runner.identify_release_artifact(artifact, report["build_argv"])
                artifact.write_bytes(b"changed")
        self.assertEqual(json.loads(Path(report["owner_path"]).read_text())["state"], "pending")

        recovering = self.runner("failed-consumer")
        with self.assertRaisesRegex(ValueError, "staging refused"):
            with recovering.guarded_release(adopt=True):
                raise ValueError("staging refused")
        self.assertIsNone(recovering.target_lock)
        self.assertEqual(json.loads(Path(report["owner_path"]).read_text())["state"], "pending")

    def test_release_ancestor_alias_accepts_but_target_symlinks_and_escape_refuse(self):
        runner = self.runner("path-alias")
        ancestor_alias = self.base / "platform-alias"
        ancestor_alias.symlink_to(self.repo, target_is_directory=True)
        with runner.guarded_release() as report:
            target = Path(runner.cargo_identity["target_directory"])
            (target / "release").mkdir(parents=True)
            artifact = target / "release/fixture"
            artifact.write_bytes(b"actual optimized artifact")
            argv = ["cargo", "build", "--release"]
            alias_path = ancestor_alias / "cargo-target/release/fixture"
            identified = runner.identify_release_artifact(alias_path, argv)
            self.assertEqual(identified, runner.identify_release_artifact(artifact, argv))
            self.assertEqual(identified["path"], str(artifact.resolve()))
            (target / "artifact-link").symlink_to(artifact)
            (target / "directory-link").symlink_to(target / "release", target_is_directory=True)
            (target / "root-link").symlink_to(target, target_is_directory=True)
            outside = self.base / "outside-artifact"
            outside.write_bytes(b"outside target")
            (target / "escape").symlink_to(outside)
            for rejected in (target / "artifact-link", target / "directory-link/fixture",
                             target / "root-link/release/fixture", target / "escape", outside,
                             ancestor_alias / "cargo-target/directory-link/fixture"):
                with self.subTest(path=rejected):
                    with self.assertRaises(local_ci.TargetCacheError):
                        runner.identify_release_artifact(rejected, argv)
            report.update(build_argv=argv, build_exit_code=0, artifact=identified)
        self.assertEqual(report["state"], "ready")

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

    def test_dependency_guard_runs_without_python311_tomllib(self) -> None:
        guard = SCRIPT.with_name("check-dependency-patches.py")
        ast.parse(guard.read_text(), feature_version=(3, 9))
        # Python 3.9 has no tomllib. An isolated stdlib-only subprocess with that
        # import unavailable reproduces the original guard failure on any host.
        code = """import builtins, runpy, sys
original_import = builtins.__import__
def without_tomllib(name, *args, **kwargs):
    if name == 'tomllib':
        raise ModuleNotFoundError('No module named tomllib')
    return original_import(name, *args, **kwargs)
builtins.__import__ = without_tomllib
runpy.run_path(sys.argv[1], run_name='__main__')
"""
        result = subprocess.run([sys.executable, "-I", "-S", "-c", code, str(guard)],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("dependency patches passed", result.stdout)

    def test_full_pipeline_claims_fresh_target_before_python_cargo_wrapper(self) -> None:
        target = Path(self.env["FAKE_METADATA_TARGET"])
        _lock, owner = local_ci.target_state_paths(target.resolve(), self.target_state_dir)
        python = self.bin / "python3"
        python.write_text(FAKE_TOOL.replace(
            'echo "completed $tool $*"',
            'if [ "$1" = scripts/check-dependency-future-lints.py ]; then\n'
            '  [ -f "$EXPECTED_OWNER" ] || exit 12\n'
            '  /bin/mkdir -p "$CARGO_TARGET_DIR/debug"\n'
            '  echo fixture > "$CARGO_TARGET_DIR/debug/fixture.rlib"\n'
            'fi\necho "completed $tool $*"',
        ))
        runner = self.runner("python-cargo", env={**self.env, "EXPECTED_OWNER": str(owner)},
                             steps=local_ci.FULL_STEPS)
        self.assertEqual(runner.run(), 0)
        summary = self.summary("python-cargo")
        self.assertEqual(summary["target_cache"]["action"], "claim-empty")
        self.assertTrue((target / "debug/fixture.rlib").is_file())
        self.assertNotIn("cargo clean", self.call_log.read_text())

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


    @unittest.skipUnless(shutil.which("cargo") and shutil.which("git"), "requires real Cargo and Git")
    def test_real_cargo_vendored_api_change_with_older_mtime_rebuilds(self) -> None:
        """A changed vendored struct must not reuse metadata from an older checkout."""
        root = self.base / "vendored-workspace"
        vendor = root / "third_party/vendor-api"
        api = root / "crates/fixture-api"
        app = root / "crates/fixture-app"
        helper = root / "external-helper"
        for directory in (vendor, api, app, helper):
            (directory / "src").mkdir(parents=True)
        (root / "Cargo.toml").write_text(
            '[workspace]\nmembers=["crates/*"]\n'
            'exclude=["external-helper", "third_party/vendor-api"]\nresolver="2"\n'
        )
        (root / ".gitignore").write_text("/artifacts/\n")
        for directory, name in ((vendor, "vendor-api"), (api, "fixture-api"),
                                (app, "fixture-app"), (helper, "external-helper")):
            (directory / "Cargo.toml").write_text(
                f'[package]\nname="{name}"\nversion="0.1.0"\nedition="2021"\n'
            )
        with (api / "Cargo.toml").open("a") as manifest:
            manifest.write('[dependencies]\nvendor-api={path="../../third_party/vendor-api"}\n')
        with (app / "Cargo.toml").open("a") as manifest:
            manifest.write('[dependencies]\nfixture-api={path="../fixture-api"}\n'
                           'external-helper={path="../../external-helper"}\n')
        (vendor / "src/lib.rs").write_text("pub struct Sample { pub old_field: u32 }\n")
        (api / "src/lib.rs").write_text(
            "pub fn value() -> vendor_api::Sample { vendor_api::Sample { old_field: 7 } }\n"
        )
        (app / "src/main.rs").write_text(
            'fn main() { println!("{}", fixture_api::value().old_field + external_helper::helper()); }\n'
        )
        (helper / "src/lib.rs").write_text("pub fn helper() -> u32 { 1 }\n")

        def command(argv, cwd=root):
            result = subprocess.run(argv, cwd=cwd, capture_output=True, text=True, check=False)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            return result.stdout.strip()

        command(["git", "init", "-q"])
        command(["git", "config", "user.email", "local-ci-test@example.invalid"])
        command(["git", "config", "user.name", "local ci fixture"])
        command(["cargo", "generate-lockfile", "--offline"])
        command(["git", "add", "."])
        command(["git", "commit", "-qm", "vendored fixture base"])
        worktree_b = self.base / "vendored-worktree-b"
        command(["git", "worktree", "add", "-qb", "vendored-b", str(worktree_b)])
        target = self.base / "vendored-cargo-target"
        state = self.base / "vendored-target-owner"
        env = {**os.environ, "CARGO_TARGET_DIR": str(target), "CARGO_INCREMENTAL": "0"}
        steps = (
            local_ci.Step("debug-build", ("cargo", "build", "--offline", "--locked")),
            local_ci.Step("release-build", ("cargo", "build", "--offline", "--locked", "--release")),
        )
        # Preserve raw runner logs before TemporaryDirectory teardown, including on failure.
        evidence_dir = os.environ.get("HEX_PRIVATE_CARGO_EVIDENCE")
        try:
            runner_a = local_ci.LocalCiRunner(
                root, run_id="vendor-a", env=env, steps=steps,
                min_free_bytes=0, target_state_dir=state,
            )
            self.assertEqual(runner_a.run(), 0)
            self.assertEqual(command([str(target / "debug/fixture-app")]), "8")
            release = target / "release/fixture-app"
            release_hash = hashlib.sha256(release.read_bytes()).hexdigest()
            helper_hashes = {
                path.name: hashlib.sha256(path.read_bytes()).hexdigest()
                for path in (target / "debug/deps").glob("libexternal_helper-*.rlib")
            }
            self.assertTrue(helper_hashes)
            vendor_artifacts = list((target / "debug/deps").glob("libvendor_api-*.rlib"))
            self.assertTrue(vendor_artifacts)
            old_timestamp = min(path.stat().st_mtime_ns for path in vendor_artifacts) - 10_000_000_000
            vendor_b = worktree_b / "third_party/vendor-api/src/lib.rs"
            vendor_b.write_text("pub struct Sample { pub new_field: u32 }\n")
            (worktree_b / "crates/fixture-api/src/lib.rs").write_text(
                "pub fn value() -> vendor_api::Sample { vendor_api::Sample { new_field: 17 } }\n"
            )
            (worktree_b / "crates/fixture-app/src/main.rs").write_text(
                'fn main() { println!("{}", fixture_api::value().new_field + external_helper::helper()); }\n'
            )
            command(["git", "add", "."], cwd=worktree_b)
            command(["git", "commit", "-qm", "change vendored struct field"], cwd=worktree_b)
            os.utime(vendor_b, ns=(old_timestamp, old_timestamp))
            self.assertLess(vendor_b.stat().st_mtime_ns, min(path.stat().st_mtime_ns for path in vendor_artifacts))
            provenance = {
                "source_sha256": hashlib.sha256(vendor_b.read_bytes()).hexdigest(),
                "source_mtime_ns": vendor_b.stat().st_mtime_ns,
                "old_artifact_mtime_ns": {path.name: path.stat().st_mtime_ns for path in vendor_artifacts},
                "base_commit": command(["git", "rev-parse", "HEAD"]),
                "changed_commit": command(["git", "rev-parse", "HEAD"], cwd=worktree_b),
                "target": str(target),
            }
            (worktree_b / "artifacts").mkdir(exist_ok=True)
            (worktree_b / "artifacts/source-provenance.json").write_text(json.dumps(provenance, indent=2))
            runner_b = local_ci.LocalCiRunner(
                worktree_b, run_id="vendor-b", env=env, steps=(steps[0],),
                min_free_bytes=0, target_state_dir=state,
            )
            result_b = runner_b.run()
            self.assertEqual(hashlib.sha256(release.read_bytes()).hexdigest(), release_hash)
            self.assertEqual({
                path.name: hashlib.sha256(path.read_bytes()).hexdigest()
                for path in (target / "debug/deps").glob("libexternal_helper-*.rlib")
            }, helper_hashes, "unrelated external dependency changed")
            self.assertEqual(result_b, 0, "real Cargo rejected changed vendored struct; inspect vendor-b log")
            self.assertEqual(command([str(target / "debug/fixture-app")]), "18")
            # Same-worktree checkout changes are equally unsafe with old mtimes.
            vendor_b.write_text("pub struct Sample { pub third_field: u32 }\n")
            (worktree_b / "crates/fixture-api/src/lib.rs").write_text(
                "pub fn value() -> vendor_api::Sample { vendor_api::Sample { third_field: 27 } }\n"
            )
            (worktree_b / "crates/fixture-app/src/main.rs").write_text(
                'fn main() { println!("{}", fixture_api::value().third_field + external_helper::helper()); }\n'
            )
            os.utime(vendor_b, ns=(old_timestamp, old_timestamp))
            runner_c = local_ci.LocalCiRunner(
                worktree_b, run_id="vendor-c", env=env, steps=(steps[0],),
                min_free_bytes=0, target_state_dir=state,
            )
            self.assertEqual(runner_c.run(), 0)
            self.assertEqual(command([str(target / "debug/fixture-app")]), "28")
            self.assertEqual(runner_c.summary["target_cache"]["action"], "reuse")
            self.assertEqual(len(runner_c.summary["target_cache"]["changed_path_dependencies"]), 1)
            runner_d = local_ci.LocalCiRunner(
                worktree_b, run_id="vendor-d", env=env, steps=(steps[0],),
                min_free_bytes=0, target_state_dir=state,
            )
            self.assertEqual(runner_d.run(), 0)
            self.assertEqual(runner_d.summary["target_cache"]["changed_path_dependencies"], [])
            self.assertIsNone(runner_d.summary["target_cache"]["cleanup_argv"])
            self.assertEqual(hashlib.sha256(release.read_bytes()).hexdigest(), release_hash)
            self.assertEqual({
                path.name: hashlib.sha256(path.read_bytes()).hexdigest()
                for path in (target / "debug/deps").glob("libexternal_helper-*.rlib")
            }, helper_hashes)
        finally:
            if evidence_dir:
                destination = Path(evidence_dir)
                destination.mkdir(parents=True, exist_ok=True)
                for label, checkout in (("a", root), ("b", worktree_b)):
                    if (checkout / "artifacts").exists():
                        shutil.copytree(checkout / "artifacts", destination / label, dirs_exist_ok=True)
                if state.exists():
                    shutil.copytree(state, destination / "ownership", dirs_exist_ok=True)
                fingerprints = target / "debug/.fingerprint"
                if fingerprints.exists():
                    shutil.copytree(fingerprints, destination / "fingerprints", dirs_exist_ok=True)


    def path_dependency_fixture(self):
        root = self.repo / "third_party/vendor"
        root.mkdir(parents=True)
        (root / "Cargo.toml").write_text('[package]\nname="vendor"\nversion="0.1.0"\n')
        (root / "lib.rs").write_text("pub const VERSION: u32 = 1;\n")
        package = {"id": "vendor 0.1.0", "name": "vendor", "version": "0.1.0",
                   "source": None, "manifest_path": str(root / "Cargo.toml")}
        original = local_ci.command_output
        def metadata(argv, cwd, env):
            result = original(argv, cwd, env)
            if argv[:2] == ["cargo", "metadata"] and "--no-deps" not in argv:
                value = json.loads(result.stdout)
                value["packages"].append(package)
                result.stdout = json.dumps(value)
            return result
        return root, package, patch.object(local_ci, "command_output", side_effect=metadata)

    def test_changed_dependency_cleanup_failure_preserves_old_identity_and_skips_build(self):
        root, _, metadata = self.path_dependency_fixture()
        with metadata:
            self.assertEqual(self.runner("dep-first").run(), 0)
            owner = next(self.target_state_dir.glob("*.owner.json"))
            before = owner.read_bytes()
            (root / "lib.rs").write_text("pub const VERSION: u32 = 2;\n")
            calls_before = self.call_log.read_text()
            self.assertEqual(self.runner("dep-failed", env={**self.env, "FAIL_MATCH": "clean"}).run(), 1)
            self.assertEqual(owner.read_bytes(), before)
            self.assertNotIn("cargo build", self.call_log.read_text()[len(calls_before):])
            self.assertEqual(self.runner("dep-recover").run(), 0)
            self.assertEqual(len(self.summary("dep-recover")["target_cache"]["changed_path_dependencies"]), 1)

    def test_legacy_dev_marker_invalidates_only_known_local_dependencies_once(self):
        _, _, metadata = self.path_dependency_fixture()
        with metadata:
            self.assertEqual(self.runner("dep-prime").run(), 0)
            owner = next(self.target_state_dir.glob("*.owner.json"))
            value = json.loads(owner.read_text())
            del value["path_dependencies"]
            owner.write_text(json.dumps(value))
            self.assertEqual(self.runner("dep-legacy").run(), 0)
            self.assertEqual(self.summary("dep-legacy")["target_cache"]["cleanup_argv"][-2:],
                             ["--package", "vendor 0.1.0"])
            self.assertEqual(self.runner("dep-known").run(), 0)
            self.assertIsNone(self.summary("dep-known")["target_cache"]["cleanup_argv"])

    def test_dependency_content_change_during_build_is_stale_even_outside_git_inventory(self):
        root, _, metadata = self.path_dependency_fixture()
        with metadata:
            runner = self.runner("dep-mutating", env={**self.env, "MUTATE_FILE": str(root / "lib.rs")})
            self.assertEqual(runner.run(), 3)
            self.assertTrue(self.summary("dep-mutating")["path_dependencies_stale"])
            self.assertTrue(self.summary("dep-mutating")["stale"])

    def test_dependency_clean_name_collision_refuses_before_marker_or_cleanup(self):
        _, package, metadata = self.path_dependency_fixture()
        with metadata:
            self.assertEqual(self.runner("dep-before-collision").run(), 0)
            owner = next(self.target_state_dir.glob("*.owner.json"))
            before = owner.read_bytes()
            # Cargo clean ignores ID qualifiers: a changed local package may not
            # select a registry/workspace package with the same name as collateral.
            package["name"] = "fixture"
            calls_before = self.call_log.read_text()
            self.assertEqual(self.runner("dep-collision").run(), 1)
            self.assertIn("overlap", self.summary("dep-collision")["target_cache_error"])
            self.assertEqual(owner.read_bytes(), before)
            self.assertNotIn("cargo clean", self.call_log.read_text()[len(calls_before):])

    def test_malformed_dependency_identity_refuses_and_preserves_marker(self):
        _, _, metadata = self.path_dependency_fixture()
        with metadata:
            self.assertEqual(self.runner("dep-before-malformed").run(), 0)
            owner = next(self.target_state_dir.glob("*.owner.json"))
            value = json.loads(owner.read_text())
            value["path_dependencies"] = {"vendor": "not a digest"}
            owner.write_text(json.dumps(value))
            before = owner.read_bytes()
            self.assertEqual(self.runner("dep-malformed").run(), 1)
            self.assertEqual(owner.read_bytes(), before)
            self.assertIn("malformed", self.summary("dep-malformed")["target_cache_error"])

    def test_directory_symlink_dependency_refuses_before_cache_mutation(self):
        root, _, metadata = self.path_dependency_fixture()
        (root / "recursive").symlink_to(root, target_is_directory=True)
        with metadata:
            self.assertEqual(self.runner("dep-symlink").run(), 1)
            self.assertIn("directory symlink", self.summary("dep-symlink")["target_cache_error"])
            self.assertFalse(list(self.target_state_dir.glob("*.owner.json")))
            self.assertNotIn("cargo clean", self.call_log.read_text())


    def test_unknown_target_refuses_before_dependency_resolution(self):
        target = Path(self.env["FAKE_METADATA_TARGET"])
        target.mkdir()
        sentinel = target / "foreign"
        sentinel.write_bytes(b"do not mutate")
        with patch.object(local_ci.LocalCiRunner, "_resolve_path_dependencies",
                          side_effect=AssertionError("resolved foreign target dependencies")):
            self.assertEqual(self.runner("dep-unknown").run(), 1)
        self.assertEqual(sentinel.read_bytes(), b"do not mutate")
        self.assertFalse(list(self.target_state_dir.glob("*.owner.json")))

    def test_contended_target_refuses_before_dependency_resolution(self):
        target = Path(self.env["FAKE_METADATA_TARGET"]).resolve()
        lock_path, _ = local_ci.target_state_paths(target, self.target_state_dir)
        holder = local_ci.CargoTargetLock(lock_path, {"run_id": "dep-holder"})
        holder.acquire()
        try:
            with patch.object(local_ci.LocalCiRunner, "_resolve_path_dependencies",
                              side_effect=AssertionError("resolved actively owned target dependencies")):
                self.assertEqual(self.runner("dep-contended").run(), 1)
        finally:
            holder.release()
        self.assertNotIn("cargo clean", self.call_log.read_text())


if __name__ == "__main__":
    unittest.main()
