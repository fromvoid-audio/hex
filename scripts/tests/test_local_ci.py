"""Behavior checks for the local CI runner using fake executables."""

from __future__ import annotations

import importlib.util
from contextlib import redirect_stderr, redirect_stdout
import io
import errno
import signal
import ast
import hashlib
import json
import os
import py_compile
from pathlib import Path
import shutil
import sys
import subprocess
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch


SCRIPT = Path(__file__).resolve().parents[1] / "local-ci.py"
SPEC = importlib.util.spec_from_file_location("local_ci", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("could not load local-ci.py")
local_ci = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = local_ci
_runner_source = SCRIPT.read_bytes()
local_ci._DELIVERY_RUNNER_SOURCE_SHA256 = hashlib.sha256(_runner_source).hexdigest()
exec(compile(_runner_source, str(SCRIPT), "exec", dont_inherit=True), local_ci.__dict__)


def install_delivery_source_fixture(root: Path) -> None:
    """Install independent copies of the actual delivery input sources."""
    scripts = root / "scripts"
    scripts.mkdir(parents=True, exist_ok=True)
    for name in ("hex-test-build.py", "local-ci.py", "ci_plan.py", "python_test_plan.py", "build_inputs.py",
                 "ci_checkpoint.py", "ci_target_storage.py"):
        (scripts / name).write_bytes((SCRIPT.parent / name).read_bytes())


def load_delivery_source_fixture(name):
    path = SCRIPT.with_name("hex-test-build.py")
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    source = path.read_bytes()
    module._EXECUTED_DELIVERY_SOURCE_SHA256 = hashlib.sha256(source).hexdigest()
    exec(compile(source, str(path), "exec", dont_inherit=True), module.__dict__)
    return module


FAKE_TOOL = """#!/bin/sh
tool=${0##*/}
printf '%s %s [target=%s]\\n' "$tool" "$*" "$CARGO_TARGET_DIR" >> "$CALL_LOG"
case "$tool $*" in
  'git rev-parse --show-toplevel') echo "$FAKE_TOPLEVEL"; exit 0 ;;
  'git rev-parse --git-common-dir') echo "$FAKE_COMMON_DIR"; exit 0 ;;
  git\\ rev-parse*) echo deadbeef; exit 0 ;;
  git\\ status*) exit 0 ;;
  git\\ ls-files*) printf '%s\\0' "$FAKE_FILE_LIST"; exit 0 ;;
  'cargo metadata'*)
    case "$FAIL_MATCH" in *metadata*) echo "metadata unavailable" >&2; exit 9 ;; esac
    if [ "$METADATA_WRITE_TARGET_FILE" = 1 ]; then
      /bin/mkdir -p "$CARGO_TARGET_DIR"
      printf 'cargo metadata side effect\n' > "$CARGO_TARGET_DIR/.rustc_info.json"
    fi
    printf '{"target_directory":"%s","workspace_root":"%s","workspace_members":["fixture 0.1.0","other 0.1.0"],"packages":[{"id":"fixture 0.1.0","name":"fixture","version":"0.1.0","manifest_path":"%s/Cargo.toml"},{"id":"other 0.1.0","name":"other","version":"0.1.0","manifest_path":"%s/other/Cargo.toml"}]}\\n' "$FAKE_METADATA_TARGET" "$FAKE_WORKSPACE_ROOT" "$FAKE_WORKSPACE_ROOT" "$FAKE_WORKSPACE_ROOT"
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


def bounded_checkpoint_case(function):
    """Keep subprocess-heavy checkpoint fixtures within the shard budget."""
    function.__hex_shard_bounded__ = True
    return function


class CargoTargetLockTests(unittest.TestCase):
    def test_existing_lock_contents_survive_acquisition_contention_and_release(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "target.lock"
            original = b"unowned private bytes\n"
            path.write_bytes(original)
            first = local_ci.CargoTargetLock(path, {"run_id": "first"})
            second = local_ci.CargoTargetLock(path, {"run_id": "second"})
            child_code = ("import fcntl,sys\n"
                          "with open(sys.argv[1], 'rb') as lock:\n"
                          " try: fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)\n"
                          " except BlockingIOError: sys.exit(42)\n")
            try:
                first.acquire()
                self.assertEqual(path.read_bytes(), original)
                self.assertEqual(subprocess.run([sys.executable, "-c", child_code, str(path)],
                                                check=False).returncode, 42)
                with self.assertRaisesRegex(local_ci.TargetCacheError, "already in use") as error:
                    second.acquire()
                self.assertNotIn(original.decode().strip(), str(error.exception))
                self.assertEqual(path.read_bytes(), original)
            finally:
                self.assertIsNone(first.release())
                self.assertIsNone(second.release())
            self.assertEqual(path.read_bytes(), original)
            self.assertEqual(subprocess.run([sys.executable, "-c", child_code, str(path)],
                                            check=False).returncode, 0)

    def test_symlink_substitution_is_refused_at_open(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "target.lock"
            victim = Path(directory) / "user-file"
            victim.write_bytes(b"preserve")
            path.symlink_to(victim)
            lock = local_ci.CargoTargetLock(path, {"run_id": "test"})
            self.addCleanup(lock.release)
            original_open = Path.open
            def replace_after_open(selected, *args, **kwargs):
                stream = original_open(selected, *args, **kwargs)
                if selected == path:
                    path.unlink()
                    os.link(victim, path)
                return stream
            # A same-inode hard link defeats a post-open stat after following
            # the symlink; the open itself must refuse it.
            with patch.object(Path, "is_symlink", return_value=False), \
                    patch.object(Path, "open", replace_after_open):
                with self.assertRaises(local_ci.TargetCacheError):
                    lock.acquire()
            self.assertIsNone(lock.file)
            self.assertEqual(victim.read_bytes(), b"preserve")

    @unittest.skipUnless(hasattr(os, "mkfifo"), "requires FIFO support")
    def test_nonregular_lock_is_refused_without_blocking_or_replacement(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "target.lock"
            os.mkfifo(path)
            lock = local_ci.CargoTargetLock(path, {"run_id": "test"})
            with self.assertRaises(local_ci.TargetCacheError):
                lock.acquire()
            self.assertIsNone(lock.file)
            self.assertTrue(path.exists())


class FreshTargetReservationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        install_delivery_source_fixture(self.root)
        self.state = self.root / "locks"
        self.target = self.root / "cargo-target"

    def reserve(self, target=None):
        return local_ci.FreshTargetReservation.acquire(
            self.root, str(self.target if target is None else target), {"run_id": "fixture"}, state_dir=self.state)

    def test_absent_target_refuses_before_staging_can_acquire_an_unrelated_directory(self):
        victim = self.root / "unrelated-empty"
        victim.mkdir()
        original_identity = victim.stat().st_ino
        created = []
        opened = []
        real_mkdir = os.mkdir
        real_open = os.open

        def track_staging(path, *args, **kwargs):
            if os.fspath(path).startswith(".hex-cargo-target-"):
                created.append(path)
            return real_mkdir(path, *args, **kwargs)

        def track_open(*args, **kwargs):
            descriptor = real_open(*args, **kwargs)
            opened.append(descriptor)
            return descriptor

        with patch.object(local_ci.os, "mkdir", side_effect=track_staging), \
                patch.object(local_ci.os, "open", side_effect=track_open), \
                self.assertRaisesRegex(local_ci.TargetCacheError, "create an empty directory"):
            with self.reserve() as reservation:
                reservation.revalidate(str(self.target))
        self.assertEqual(created, [])
        self.assertTrue(opened)
        for descriptor in set(opened):
            with self.assertRaises(OSError):
                os.fstat(descriptor)
        self.assertEqual(victim.stat().st_ino, original_identity)
        self.assertFalse(self.target.exists())
        self.target.mkdir()
        with self.reserve() as recovered:
            recovered.revalidate(str(self.target))

    def test_empty_target_holds_exact_lock_until_close(self):
        self.target.mkdir()
        with self.reserve() as reservation:
            reservation.revalidate(str(self.target))
            with self.assertRaisesRegex(local_ci.TargetCacheError, "already in use"):
                self.reserve()
            with self.assertRaisesRegex(local_ci.TargetCacheError, "differs"):
                reservation.revalidate(str(self.root / "other"))
        with self.assertRaisesRegex(local_ci.TargetCacheError, "no longer live"):
            reservation.revalidate(str(self.target))

    def test_relative_alias_symlink_and_missing_parent_refuse(self):
        self.target.mkdir()
        alias = self.root / "alias"
        alias.symlink_to(self.target, target_is_directory=True)
        parent_alias = self.root / "parent-alias"
        parent_alias.symlink_to(self.root, target_is_directory=True)
        for value in ("relative", "//tmp/alias", str(self.target) + "\0bad", str(alias), str(parent_alias / "cargo-target"),
                      str(self.root / "missing" / "target"), str(self.target) + "/."):
            with self.subTest(value=value), self.assertRaises(local_ci.TargetCacheError):
                self.reserve(value)
        self.assertFalse(any(self.target.iterdir()))

    def test_metadata_entries_and_target_presence_changes_refuse(self):
        self.target.mkdir()
        with self.reserve() as reservation:
            (self.target / ".rustc_info.json").write_text("metadata")
            with self.assertRaisesRegex(local_ci.TargetCacheError, "not empty"):
                reservation.revalidate(str(self.target))
            (self.target / ".rustc_info.json").unlink()
            with self.assertRaisesRegex(local_ci.TargetCacheError, "no longer live"):
                reservation.revalidate(str(self.target))
        with self.reserve() as replacement:
            replacement.revalidate(str(self.target))
        self.target.rmdir()
        with self.assertRaisesRegex(local_ci.TargetCacheError, "create an empty directory"):
            self.reserve()
        self.assertFalse(self.target.exists())

    def test_replacement_of_target_or_ancestor_refuses(self):
        parent = self.root / "parent"
        parent.mkdir()
        target = parent / "target"
        target.mkdir()
        with self.reserve(target) as reservation:
            target.rename(parent / "old-target")
            target.mkdir()
            with self.assertRaisesRegex(local_ci.TargetCacheError, "identity"):
                reservation.revalidate(str(target))
        target.rmdir()
        (parent / "old-target").rename(target)
        with self.reserve(target) as reservation:
            parent.rename(self.root / "old-parent")
            parent.mkdir()
            (parent / "target").mkdir()
            with self.assertRaisesRegex(local_ci.TargetCacheError, "identity"):
                reservation.revalidate(str(target))

    def test_source_change_revokes_reservation(self):
        self.target.mkdir()
        with self.reserve() as reservation:
            helper = self.root / "scripts/ci_plan.py"
            helper.write_bytes(helper.read_bytes() + b"\n# changed\n")
            with self.assertRaisesRegex(local_ci.TargetCacheError, "source mismatch"):
                reservation.revalidate(str(self.target))
            self.assertIsNone(reservation._lock.file)
            self.assertEqual(reservation._handles, [])

    def test_failure_closes_descriptors_and_preserves_target_and_owner(self):
        self.target.mkdir()
        sentinel = self.target / "keep"
        sentinel.write_bytes(b"preserve")
        _, owner = local_ci.target_state_paths(self.target, self.state)
        owner.parent.mkdir(parents=True)
        opened = []
        original = os.open
        def record(*args, **kwargs):
            descriptor = original(*args, **kwargs)
            opened.append(descriptor)
            return descriptor
        with patch.object(local_ci.os, "open", side_effect=record):
            with self.assertRaisesRegex(local_ci.TargetCacheError, "not empty"):
                self.reserve()
        self.assertTrue(opened)
        for descriptor in opened:
            with self.assertRaises(OSError):
                os.fstat(descriptor)
        self.assertEqual(sentinel.read_bytes(), b"preserve")
        self.assertFalse(owner.exists())
        sentinel.unlink()
        owner.write_bytes(b"existing owner")
        with self.assertRaisesRegex(local_ci.TargetCacheError, "owner marker"):
            self.reserve()
        self.assertEqual(owner.read_bytes(), b"existing owner")
        owner.unlink()
        with self.reserve() as reservation:
            self.assertTrue(reservation._handles)
            reservation._lock.release()  # Unusual external unlock invalidates authority.
            with self.assertRaisesRegex(local_ci.TargetCacheError, "no longer live"):
                reservation.revalidate(str(self.target))

    def test_owner_markers_revoke_fresh_authority(self):
        self.target.mkdir()
        _, owner = local_ci.target_state_paths(self.target, self.state)
        release = owner.with_name(owner.name.replace(".owner.json", ".release-owner.json"))
        for marker in (owner, release):
            with self.subTest(marker=marker.name):
                with self.reserve() as reservation:
                    marker.write_bytes(b"other owner")
                    with self.assertRaisesRegex(local_ci.TargetCacheError, "owner marker"):
                        reservation.revalidate(str(self.target))
                self.assertEqual(marker.read_bytes(), b"other owner")
                marker.unlink()

    def test_existing_release_marker_and_dangling_marker_refuse(self):
        self.target.mkdir()
        _, owner = local_ci.target_state_paths(self.target, self.state)
        release = owner.with_name(owner.name.replace(".owner.json", ".release-owner.json"))
        self.state.mkdir(parents=True)
        for dangling in (False, True):
            with self.subTest(dangling=dangling):
                if dangling:
                    release.symlink_to(self.root / "missing")
                else:
                    release.write_bytes(b"existing")
                with self.assertRaisesRegex(local_ci.TargetCacheError, "owner marker"):
                    self.reserve()
                self.assertTrue(os.path.lexists(release))
                release.unlink()
                with self.reserve():
                    pass

    def test_replaced_lock_or_state_directory_revokes_authority(self):
        self.target.mkdir()
        for replace_state in (False, True):
            with self.subTest(replace_state=replace_state):
                with self.reserve() as reservation:
                    lock_path = reservation._lock.path
                    if replace_state:
                        self.state.rename(self.root / "old-locks")
                        self.state.mkdir()
                    else:
                        lock_path.unlink()
                    lock_path.write_text("replacement")
                    with self.assertRaisesRegex(local_ci.TargetCacheError, "lock identity"):
                        reservation.revalidate(str(self.target))
                    self.assertIsNone(reservation._lock.file)
                    self.assertEqual(lock_path.read_text(), "replacement")
                if replace_state:
                    lock_path.unlink()
                    self.state.rmdir()
                    (self.root / "old-locks").rename(self.state)

    def test_reserved_metadata_matches_target_and_preserves_live_lock(self):
        self.target.mkdir()
        runner = local_ci.LocalCiRunner(self.root, env={"CARGO_TARGET_DIR": str(self.target)},
                                        target_state_dir=self.state)
        metadata = {"target_directory": str(self.target), "workspace_root": str(self.root),
                    "workspace_members": ["fixture"],
                    "packages": [{"id": "fixture", "name": "fixture", "version": "0.1.0"}]}
        result = subprocess.CompletedProcess([], 0, json.dumps(metadata), "")
        with self.reserve() as reservation, patch.object(local_ci.subprocess, "run", return_value=result):
            self.assertTrue(runner._resolve_cargo_identity(reservation))
            reservation.revalidate(str(self.target))
            self.assertEqual(runner.cargo_identity["target_directory"], str(self.target))
            self.assertIsNone(runner.target_lock)  # Identity resolution grants no custody.

    def test_reserved_metadata_mismatch_or_mutation_refuses(self):
        self.target.mkdir()
        for scenario in ("configured", "selected", "created"):
            with self.subTest(scenario=scenario):
                env_target = self.root / "other" if scenario == "configured" else self.target
                runner = local_ci.LocalCiRunner(self.root, env={"CARGO_TARGET_DIR": str(env_target)},
                                                target_state_dir=self.state)
                metadata = {"target_directory": str(self.root / "other" if scenario == "selected" else self.target),
                            "workspace_root": str(self.root), "workspace_members": ["fixture"],
                            "packages": [{"id": "fixture", "name": "fixture", "version": "0.1.0"}]}
                def run(*args, **kwargs):
                    if scenario == "created":
                        (self.target / "entry").write_text("preserve")
                    return subprocess.CompletedProcess([], 0, json.dumps(metadata), "")
                with self.reserve() as reservation, patch.object(local_ci.subprocess, "run", side_effect=run) as cargo:
                    self.assertFalse(runner._resolve_cargo_identity(reservation))
                    self.assertIsNone(runner.cargo_identity)
                    self.assertIsNone(reservation._lock.file)
                    if scenario == "configured":
                        cargo.assert_not_called()
                if scenario == "created":
                    self.assertEqual((self.target / "entry").read_text(), "preserve")
                    (self.target / "entry").unlink()

    def test_reserved_real_cargo_metadata_on_empty_target(self):
        (self.root / "Cargo.toml").write_text('[package]\nname = "fixture"\nversion = "0.1.0"\nedition = "2021"\n')
        (self.root / "Cargo.lock").write_text('version = 4\n\n[[package]]\nname = "fixture"\nversion = "0.1.0"\n')
        (self.root / "src").mkdir()
        (self.root / "src/lib.rs").write_text("")
        self.target.mkdir()
        env = dict(os.environ, CARGO_TARGET_DIR=str(self.target), CARGO_NET_OFFLINE="true")
        runner = local_ci.LocalCiRunner(self.root, env=env, target_state_dir=self.state)
        with self.reserve() as reservation:
            self.assertTrue(runner._resolve_cargo_identity(reservation), runner.summary.get("cargo_metadata_error"))
            reservation.revalidate(str(self.target))
            self.assertEqual(runner.cargo_identity["target_directory"], str(self.target))

    def test_reserved_metadata_refuses_other_lock_namespace_before_cargo(self):
        self.target.mkdir()
        other_state = self.root / "other-locks"
        runner = local_ci.LocalCiRunner(self.root, env={"CARGO_TARGET_DIR": str(self.target)},
                                        target_state_dir=other_state)
        _, owner = local_ci.target_state_paths(self.target, other_state)
        owner.parent.mkdir(parents=True)
        owner.write_text("existing owner")
        with self.reserve() as reservation, patch.object(local_ci.subprocess, "run") as cargo:
            self.assertFalse(runner._resolve_cargo_identity(reservation))
            cargo.assert_not_called()
            self.assertIsNone(reservation._lock.file)
        self.assertEqual(owner.read_text(), "existing owner")

    def test_failed_or_interrupted_resolution_revokes_prior_identity(self):
        self.target.mkdir()
        runner = local_ci.LocalCiRunner(self.root, env={"CARGO_TARGET_DIR": str(self.target)},
                                        target_state_dir=self.state)
        metadata = {"target_directory": str(self.target), "workspace_root": str(self.root),
                    "workspace_members": ["fixture"],
                    "packages": [{"id": "fixture", "name": "fixture", "version": "0.1.0"}]}
        success = subprocess.CompletedProcess([], 0, json.dumps(metadata), "")
        for interruption in (False, True):
            with self.subTest(interruption=interruption), self.reserve() as reservation:
                with patch.object(local_ci.subprocess, "run", return_value=success):
                    self.assertTrue(runner._resolve_cargo_identity(reservation))
                failure = KeyboardInterrupt() if interruption else subprocess.CompletedProcess([], 1, "", "failure")
                with patch.object(local_ci.subprocess, "run", side_effect=failure if interruption else None,
                                  return_value=None if interruption else failure):
                    if interruption:
                        with self.assertRaises(KeyboardInterrupt):
                            runner._resolve_cargo_identity(reservation)
                    else:
                        self.assertFalse(runner._resolve_cargo_identity(reservation))
                self.assertIsNone(runner.cargo_identity)
                self.assertIsNone(runner.target_owner_path)
                self.assertIsNone(runner._lexical_cargo_target)
                self.assertNotIn("cargo_identity", runner.summary)
                self.assertIsNone(reservation._lock.file)
            with self.reserve():
                pass

    def test_metadata_refusal_reports_cleanup_failure_without_masking_cause(self):
        self.target.mkdir()
        runner = local_ci.LocalCiRunner(self.root, env={"CARGO_TARGET_DIR": str(self.target)},
                                        target_state_dir=self.state)
        reservation = self.reserve()
        release = reservation._lock.release
        def failed_release():
            release()
            return "simulated release failure"
        metadata = {"target_directory": str(self.root / "other"), "workspace_root": str(self.root),
                    "workspace_members": ["fixture"], "packages": []}
        result = subprocess.CompletedProcess([], 0, json.dumps(metadata), "")
        with patch.object(reservation._lock, "release", side_effect=failed_release), \
                patch.object(local_ci.subprocess, "run", return_value=result):
            self.assertFalse(runner._resolve_cargo_identity(reservation))
        self.assertIn("differs", runner.summary["cargo_metadata_error"])
        self.assertEqual(runner.summary["reservation_close_error"], "simulated release failure")
        self.assertIsNone(reservation._lock.file)


class ManualFixtureCleanupTests(unittest.TestCase):
    def test_manual_teardown_restores_runner_sequence_guard(self):
        original = local_ci.LocalCiRunner._step_sequence_error
        fixture = LocalCiRunnerTests()
        fixture.setUp()
        try:
            self.assertIsNot(local_ci.LocalCiRunner._step_sequence_error, original)
        finally:
            fixture.tearDown()
        self.assertIs(local_ci.LocalCiRunner._step_sequence_error, original)


class SuppliedGitScopeTests(unittest.TestCase):
    def test_altered_git_scope_refuses_before_steps_or_target_access(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            env = {**os.environ, "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull}
            def git(*args):
                return subprocess.check_output(["git", *args], cwd=root, env=env, text=True).strip()
            git("init", "-q")
            (root / ".gitignore").write_text("artifacts/\n")
            (root / "README.md").write_text("first\n")
            git("add", ".")
            git("-c", "user.name=fixture", "-c", "user.email=fixture@example.invalid", "commit", "-qm", "base")
            base = git("rev-parse", "HEAD")
            (root / "README.md").write_text("second\n")
            git("add", ".")
            git("-c", "user.name=fixture", "-c", "user.email=fixture@example.invalid", "commit", "-qm", "source")
            plan = {"profile": "merge", "scope": "metadata", "qualifies_merge": True,
                    "base": base, "source": git("rev-parse", "HEAD"),
                    "tree": git("rev-parse", "HEAD^{tree}"), "paths": ["README.md"],
                    "runtime_inputs": local_ci.ci_plan.runtime_inputs(root, env)}
            matching = local_ci.LocalCiRunner(root, mode="merge", env=env, plan=plan, min_free_bytes=0)
            with patch.dict(os.environ, {"GIT_DIR": str(root / "not-the-repository")}):
                self.assertIsNone(matching._git_scope_error())
            self.assertIsNone(local_ci.LocalCiRunner(root, mode="edit", env=env,
                plan={"qualifies_merge": False}, steps=())._git_scope_error())
            for field, value in (("paths", []), ("tree", "0" * 40), ("base", plan["source"])):
                altered = {**plan, field: value}
                altered.pop("plan_sha256", None)
                altered["plan_sha256"] = hashlib.sha256(json.dumps(altered, sort_keys=True).encode()).hexdigest()
                runner = local_ci.LocalCiRunner(root, mode="merge", env=env, plan=altered,
                                                run_id=field, min_free_bytes=0)
                with patch.object(runner, "_execution_policy_error", return_value=None), \
                        patch.object(runner, "_run_step", side_effect=AssertionError("unvalidated steps executed")) as execute, \
                        patch.object(runner, "_resolve_cargo_identity") as target:
                    self.assertEqual(runner.run(), 2)
                self.assertIn("git_scope_error", runner.summary)
                execute.assert_not_called()
                target.assert_not_called()


class RunnerEnvironmentInputTests(unittest.TestCase):
    def test_qualifying_inputs_use_the_runner_git_environment(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory).resolve()
            root = base / "repo"
            root.mkdir()
            clean_config = base / "clean-config"
            clean_config.write_text("")
            excludes = base / "excludes"
            excludes.write_text("untracked.rs\n")
            configured = base / "configured-git"
            configured.write_text(f"[core]\n\texcludesfile = {excludes}\n")
            default_env = {**os.environ, "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": str(clean_config)}
            actual_env = {**default_env, "GIT_CONFIG_GLOBAL": str(configured)}

            def git(*args):
                return subprocess.check_output(["git", *args], cwd=root, env=default_env, text=True).strip()

            git("init", "-q")
            (root / ".gitignore").write_text("artifacts/\n")
            (root / "tracked.rs").write_text("tracked\n")
            git("add", ".gitignore", "tracked.rs")
            git("-c", "user.name=fixture", "-c", "user.email=fixture@example.invalid", "commit", "-qm", "initial")
            source = git("rev-parse", "HEAD")
            (root / "untracked.rs").write_text("untracked\n")
            default_inputs = local_ci.ci_plan.runtime_inputs(root, default_env)
            actual_inputs = local_ci.ci_plan.runtime_inputs(root, actual_env)
            self.assertNotEqual(default_inputs, actual_inputs)
            self.assertEqual(subprocess.check_output(
                ["git", "status", "--porcelain"], cwd=root, env=actual_env, text=True), "")

            for label, inputs in (("mismatch", default_inputs), ("matching", actual_inputs)):
                with self.subTest(label=label), patch.dict(os.environ, default_env, clear=True):
                    plan = {"scope": "full", "profile": "merge", "qualifies_merge": True,
                            "source": source, "runtime_inputs": inputs}
                    runner = local_ci.LocalCiRunner(root, mode="merge", run_id=label, env=actual_env,
                                                   plan=plan, min_free_bytes=0)
                    def stop_before_target():
                        runner.summary["cargo_metadata_error"] = "fixture stopped before target"
                        return False
                    with patch.object(runner, "_execution_policy_error", return_value=None), \
                            patch.object(runner, "_git_scope_error", return_value=None), \
                            patch.object(runner, "_resolve_cargo_identity", side_effect=stop_before_target) as identity, \
                            patch.object(runner, "_run_step") as execute:
                        self.assertEqual(runner.run(), 2)
                    if label == "mismatch":
                        self.assertIn("resolved plan source/inputs changed", runner.summary["metadata_error"])
                        identity.assert_not_called()
                    else:
                        identity.assert_called_once()
                    execute.assert_not_called()


class LocalCiRunnerTests(unittest.TestCase):
    def test_qualifying_plan_refuses_partial_overlays_before_target_access(self):
        self.step_sequence_patch.stop()
        overlays = {
            "edit": {"package": "fixture", "features": "", "target": None, "filter": "one_test"},
            "health": {"python": False, "workspace_inputs": False, "packages": []},
        }
        for mode in ("full", "merge", "release"):
            for key, value in overlays.items():
                with self.subTest(mode=mode, overlay=key):
                    plan = {"scope": "metadata", "profile": mode, "base": "base", "source": "source",
                            "qualifies_merge": True, key: value}
                    plan["plan_sha256"] = hashlib.sha256(json.dumps(plan, sort_keys=True).encode()).hexdigest()
                    runner = local_ci.LocalCiRunner(
                        self.repo, mode=mode, run_id=f"overlay-{mode}-{key}", env=self.env,
                        plan=plan, steps=local_ci.planned_steps(plan), min_free_bytes=0,
                        target_state_dir=self.target_state_dir)
                    with patch.object(runner, "_capture_repository_state", return_value=False) as capture, \
                            patch.object(runner, "_resolve_cargo_identity") as identity, \
                            patch.object(runner, "_run_step") as execute:
                        self.assertEqual(runner.run(), 2)
                    self.assertIn("step_sequence_error", runner.summary)
                    capture.assert_not_called()
                    identity.assert_not_called()
                    execute.assert_not_called()
                    self.assertFalse(Path(self.env["FAKE_METADATA_TARGET"]).exists())
        for key in overlays:
            for index, value in enumerate((None, {}, [], "invalid")):
                with self.subTest(overlay=key, malformed=value):
                    plan = {"scope": "metadata", "profile": "merge", "qualifies_merge": True, key: value}
                    runner = local_ci.LocalCiRunner(
                        self.repo, mode="merge", run_id=f"malformed-{key}-{index}", env=self.env,
                        plan=plan, steps=(), min_free_bytes=0, target_state_dir=self.target_state_dir)
                    with patch.object(runner, "_capture_repository_state") as capture, \
                            patch.object(runner, "_resolve_cargo_identity") as identity, \
                            patch.object(runner, "_run_step") as execute:
                        self.assertEqual(runner.run(), 2)
                    self.assertIn("step_sequence_error", runner.summary)
                    capture.assert_not_called()
                    identity.assert_not_called()
                    execute.assert_not_called()
        for key, value in overlays.items():
            plan = {"scope": "metadata", "profile": key, "base": "base", "source": "source",
                    "qualifies_merge": False, key: value}
            runner = local_ci.LocalCiRunner(self.repo, mode=key, env=self.env, plan=plan)
            self.assertIsNone(runner._step_sequence_error())
            self.assertTrue(runner.steps)

    def test_qualifying_step_sequence_refuses_drift_before_execution_or_target_access(self):
        self.step_sequence_patch.stop()
        expected = local_ci.planned_steps({"scope": "full"})
        first = expected[0]
        cases = {
            "empty": (),
            "missing": expected[1:],
            "added": (*expected, first),
            "reordered": (expected[1], expected[0], *expected[2:]),
            "argv": (local_ci.Step(first.step_id, (*first.argv, "--changed"),
                                   first.uses_cargo_target, first.requires_tests), *expected[1:]),
            "flags": (local_ci.Step(first.step_id, first.argv, not first.uses_cargo_target,
                                    first.requires_tests), *expected[1:]),
            "id": (local_ci.Step("changed", first.argv, first.uses_cargo_target,
                                 first.requires_tests), *expected[1:]),
            "test-flag": (local_ci.Step(first.step_id, first.argv, first.uses_cargo_target,
                                         not first.requires_tests), *expected[1:]),
        }
        for label, steps in cases.items():
            with self.subTest(label=label):
                runner = local_ci.LocalCiRunner(self.repo, mode="full", run_id="sequence-" + label,
                    env=self.env, steps=steps, min_free_bytes=0, target_state_dir=self.target_state_dir)
                with patch.object(runner, "_resolve_cargo_identity") as identity, \
                        patch.object(runner, "_run_step") as execute:
                    self.assertEqual(runner.run(), 2)
                self.assertIn("step_sequence_error", runner.summary)
                identity.assert_not_called()
                execute.assert_not_called()

    def test_qualifying_plan_profile_must_match_runner_before_target_access(self):
        self.step_sequence_patch.stop()
        profiles = ("full", "merge", "release", None, [], 1)
        for mode in ("full", "merge", "release"):
            for profile in profiles:
                if profile == mode:
                    continue
                with self.subTest(mode=mode, profile=profile):
                    plan = {"scope": "metadata", "base": "base", "source": "source",
                            "qualifies_merge": True}
                    if profile is not None:
                        plan["profile"] = profile
                    runner = local_ci.LocalCiRunner(
                        self.repo, mode=mode, run_id=f"profile-{mode}-{profiles.index(profile)}",
                        env=self.env, plan=plan, steps=local_ci.planned_steps(plan),
                        min_free_bytes=0, target_state_dir=self.target_state_dir)
                    with patch.object(runner, "_resolve_cargo_identity") as identity, \
                            patch.object(runner, "_run_step") as execute, \
                            patch.object(runner, "_capture_repository_state", return_value=False) as capture:
                        self.assertEqual(runner.run(), 2)
                    self.assertIn("step_sequence_error", runner.summary)
                    identity.assert_not_called()
                    execute.assert_not_called()
                    capture.assert_not_called()
                    self.assertFalse(Path(self.env["FAKE_METADATA_TARGET"]).exists())
        for mode in ("full", "merge", "release"):
            plan = {"scope": "metadata", "profile": mode, "base": "base", "source": "source",
                    "qualifies_merge": True}
            runner = local_ci.LocalCiRunner(self.repo, mode=mode, env=self.env, plan=plan)
            self.assertIsNone(runner._step_sequence_error())
        self.assertIsNone(local_ci.LocalCiRunner(self.repo, mode="full", env=self.env)._step_sequence_error())
        partial = {"scope": "metadata", "profile": "edit", "qualifies_merge": False}
        self.assertIsNone(local_ci.LocalCiRunner(self.repo, mode="merge", env=self.env,
                                               plan=partial, steps=())._step_sequence_error())

    def test_scoped_step_binding_and_nonqualifying_custom_execution(self):
        self.step_sequence_patch.stop()
        plan = {"scope": "metadata", "profile": "merge", "base": "base", "source": "source", "qualifies_merge": True}
        expected = local_ci.planned_steps(plan)
        matching = local_ci.LocalCiRunner(self.repo, mode="merge", env=self.env, plan=plan, steps=expected)
        self.assertIsNone(matching._step_sequence_error())
        altered = local_ci.LocalCiRunner(self.repo, mode="merge", env=self.env, plan=plan, steps=())
        self.assertIsNotNone(altered._step_sequence_error())
        with patch.object(altered, "_resolve_cargo_identity") as identity, \
                patch.object(altered, "_run_step") as execute:
            self.assertEqual(altered.run(), 2)
        identity.assert_not_called()
        execute.assert_not_called()
        full = local_ci.LocalCiRunner(self.repo, mode="full", env=self.env)
        self.assertIsNone(full._step_sequence_error())
        for mode in ("quick", "component", "custom"):
            with self.subTest(mode=mode):
                partial = local_ci.LocalCiRunner(self.repo, mode=mode, run_id="custom-" + mode, env=self.env,
                    steps=(local_ci.Step("first", ("first",)),), min_free_bytes=0,
                    target_state_dir=self.target_state_dir)
                self.assertEqual(partial.run(), 0)
                self.assertEqual(partial.summary["status"], "partial")
        scoped_partial = local_ci.LocalCiRunner(self.repo, mode="merge", env=self.env,
            plan={"scope": "metadata", "qualifies_merge": False}, steps=())
        self.assertIsNone(scoped_partial._step_sequence_error())

    def test_module_shard_empty_configs_require_aggregate_passes(self):
        (self.bin / "python3").write_text(
            '#!/bin/sh\ncase "$*" in\n'
            '  *"--filter covered::"*) echo "test result: ok. 1 passed; 0 failed; 0 ignored;" ;;\n'
            '  *) echo "no tests matched the selection" ;;\n'
            'esac\n')
        for covered in (False, True):
            with self.subTest(covered=covered):
                steps = tuple(local_ci.Step(
                    f"config-{index}", ("python3", "scripts/test_shards.py", "--shards", "2",
                    "--allow-empty", "--filter", name, "--", "cargo", "test"))
                    for index, name in enumerate(("absent::", "covered::" if covered else "absent::")))
                runner = local_ci.LocalCiRunner(
                    self.repo, mode="merge", run_id=f"module-coverage-{covered}",
                    env=self.env, steps=steps, min_free_bytes=0,
                    target_state_dir=self.target_state_dir)
                self.assertEqual(runner.run(), 0 if covered else 1)
                self.assertEqual("module_scope_error" in runner.summary, not covered)

    def test_log_timing_estimates_label_unknown_and_overlapping_durations(self):
        text = ("    Finished `test` profile target(s) in 1m 2.50s\n"
                "test result: ok. 2 passed; finished in 3.25s\n"
                "test result: FAILED. 1 failed; finished in 0.75s\n"
                "Ran 1 test in 0.500s\n")
        result = local_ci.log_timing_estimates(text, 70.0)
        self.assertEqual(result, {"cargo_build_phase_seconds": 62.5, "rust_harness_seconds": 4.0,
                                  "python_harness_seconds": 0.5, "unclassified_seconds": 3.0})
        self.assertIsNone(local_ci.log_timing_estimates(text, 1.0)["unclassified_seconds"])
        unknown = local_ci.log_timing_estimates("unrecognized tool output", 0.123456)
        self.assertIsNone(unknown["cargo_build_phase_seconds"])
        self.assertIsNone(unknown["rust_harness_seconds"])
        self.assertEqual(unknown["unclassified_seconds"], 0.123456)

    def test_step_timing_uses_monotonic_clock_even_with_constant_utc(self):
        runner = self.runner("clock", steps=(local_ci.Step("first", ("first",)),))
        with patch.object(local_ci, "utc_now", return_value="2026-01-01T00:00:00Z"):
            self.assertEqual(runner.run(), 0)
        summary = self.summary("clock")
        step = summary["steps"][0]
        self.assertEqual(step["started_at"], step["ended_at"])
        self.assertGreater(step["duration_seconds"], 0)
        self.assertAlmostEqual(summary["time_to_first_result_seconds"],
                               step["start_offset_seconds"] + step["duration_seconds"], places=5)
        self.assertGreaterEqual(summary["duration_seconds"], summary["time_to_first_result_seconds"])

    def setUp(self) -> None:
        # Pipeline and checkpoint fixtures exercise other boundaries with short,
        # synthetic steps. Sequence admission has dedicated unmocked tests above.
        self.step_sequence_patch = patch.object(local_ci.LocalCiRunner, "_step_sequence_error", return_value=None)
        self.step_sequence_patch.start()
        self.addCleanup(self.step_sequence_patch.stop)
        # These pipeline fixtures use synthetic Git; scope authentication has
        # its own real-repository regression above.
        self.git_scope_patch = patch.object(local_ci.LocalCiRunner, "_git_scope_error", return_value=None)
        self.git_scope_patch.start()
        self.addCleanup(self.git_scope_patch.stop)
        self.temp = tempfile.TemporaryDirectory()
        # Ordinary fixtures use the same physical identity as the runner.
        # Tests for unsafe lexical aliases construct those paths explicitly.
        self.base = Path(self.temp.name).resolve()
        self.repo = self.base / "repo"
        self.repo.mkdir()
        install_delivery_source_fixture(self.repo)
        self.bin = self.base / "bin"
        self.bin.mkdir()
        self.call_log = self.base / "calls.log"
        for name in ("git", "python3", "cargo", "rustc", "first", "second", "bad"):
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
            "METADATA_WRITE_TARGET_FILE": "0",
        }
        self.target_state_dir = self.base / "target-ownership"
        # Storage probing has its own real-command/metadata tests. Keep cache,
        # ownership and pipeline fixtures independent of the host filesystem.
        storage = patch.object(local_ci.ci_target_storage, "inspect", return_value={
            "platform": "darwin", "status": "identified", "filesystem_type": "apfs"})
        storage.start()
        self.addCleanup(storage.stop)


    def tearDown(self) -> None:
        try:
            self.temp.cleanup()
        finally:
            # Some other test modules construct this fixture manually.
            # unittest invokes cleanups automatically only for its own run().
            self.doCleanups()

    def test_programmatic_source_admission_and_missing_witness_refuse(self):
        for missing_witness in (False, True):
            with self.subTest(missing_witness=missing_witness):
                source = self.repo / "scripts/build_inputs.py"
                original = source.read_bytes()
                if not missing_witness:
                    source.write_bytes(original + b"\n# changed\n")
                with patch.dict(local_ci.__dict__, {"_DELIVERY_RUNNER_SOURCE_SHA256": None} if missing_witness else {}):
                    runner = self.runner(f"source-refusal-{missing_witness}")
                    self.assertEqual(runner.run(), 2)
                    self.assertIn("runner_policy_error", runner.summary)
                    self.assertTrue(all(row["status"] == "not_run" for row in runner.summary["steps"]))
                    with self.assertRaisesRegex(local_ci.TargetCacheError, "source mismatch"):
                        with self.runner(f"release-refusal-{missing_witness}").guarded_release():
                            self.fail("unattested release entered")
                source.write_bytes(original)
        self.assertEqual(self.runner("source-match").run(), 0)

    def test_programmatic_planner_changed_before_snapshot_refuses_without_target_mutation(self):
        planner = self.repo / "scripts/ci_plan.py"
        planner.write_bytes(planner.read_bytes() + b"\n# changed after import\n")
        target = Path(self.env["FAKE_METADATA_TARGET"])
        target.mkdir()
        sentinel = target / "preserve"
        sentinel.write_bytes(b"previous artifact")
        runner = self.runner("planner-before-snapshot")
        with patch.object(runner, "_capture_repository_state", wraps=runner._capture_repository_state) as capture, \
                patch.object(runner, "_resolve_cargo_identity", wraps=runner._resolve_cargo_identity) as identity:
            self.assertEqual(runner.run(), 2)
        capture.assert_not_called()
        identity.assert_not_called()
        self.assertIn("ci_plan.py", runner.summary["runner_policy_error"])
        self.assertEqual(sentinel.read_bytes(), b"previous artifact")
        self.assertIsNone(runner.target_lock)

    def test_programmatic_policy_mutation_after_lock_refuses_owner(self):
        source = self.repo / "scripts/ci_plan.py"
        original = source.read_bytes()
        acquire = local_ci.CargoTargetLock.acquire
        def changed(lock):
            acquire(lock)
            source.write_bytes(original + b"\n# changed under lock\n")
        for release in (False, True):
            with self.subTest(release=release), patch.object(local_ci.CargoTargetLock, "acquire", changed):
                runner = self.runner(f"under-lock-{release}")
                if release:
                    with self.assertRaisesRegex(local_ci.TargetCacheError, "source changed before release lock"):
                        with runner.guarded_release():
                            self.fail("release entered")
                else:
                    self.assertEqual(runner.run(), 2)
                    self.assertIn("source changed before target lock", runner.summary["target_cache_error"])
                self.assertIsNone(runner.target_lock)
                source.write_bytes(original)

    def test_programmatic_mutation_before_cleanup_preserves_owner_and_unlocks(self):
        first = self.runner("cleanup-source-first")
        self.assertEqual(first.run(), 0)
        owner = first.target_owner_path
        before = owner.read_bytes()
        worktree = self.base / "cleanup-source-other"
        worktree.mkdir()
        install_delivery_source_fixture(worktree)
        helper = worktree / "scripts/ci_plan.py"
        env = {**self.env, "FAKE_TOPLEVEL": str(worktree),
               "FAKE_COMMON_DIR": str((self.repo / ".git").resolve()),
               "FAKE_WORKSPACE_ROOT": str(worktree)}
        runner = self.runner("cleanup-source-second", root=worktree, env=env)
        clean_argv = runner._workspace_clean_argv
        def mutate_before_cleanup(*args):
            argv = clean_argv(*args)
            helper.write_bytes(helper.read_bytes() + b"\n# mutated after lock\n")
            return argv
        with patch.object(runner, "_workspace_clean_argv", side_effect=mutate_before_cleanup) as select, \
                patch.object(runner, "_run_target_cleanup") as cleanup:
            self.assertEqual(runner.run(), 2)
            select.assert_called()
            cleanup.assert_not_called()
        self.assertIn("source changed before target cleanup", runner.summary["target_cache_error"])
        self.assertEqual(owner.read_bytes(), before)
        self.assertIsNone(runner.target_lock)

    def test_programmatic_policy_mutation_prevents_final_publication(self):
        source = self.repo / "scripts/ci_plan.py"
        original = source.read_bytes()
        runner = self.runner("final-source")
        run_step = runner._run_step
        def changed(*args):
            result = run_step(*args)
            source.write_bytes(original + b"\n# changed during checks\n")
            return result
        with patch.object(runner, "_run_step", side_effect=changed):
            self.assertEqual(runner.run(), 2)
        self.assertNotEqual(runner.summary["status"], "passed")
        self.assertIn("target_owner_update_error", runner.summary)
        source.write_bytes(original)
        release = self.runner("final-release-source")
        with self.assertRaisesRegex(local_ci.TargetCacheError, "source changed during release"):
            with release.guarded_release() as report:
                target = Path(release.cargo_identity["target_directory"])
                target.mkdir(exist_ok=True)
                artifact = target / "release-fixture"
                artifact.write_bytes(b"optimized fixture")
                argv = ["cargo", "build", "--release"]
                report.update(build_argv=argv, build_exit_code=0,
                              artifact=release.identify_release_artifact(artifact, argv))
                source.write_bytes(original + b"\n# changed during release\n")
        self.assertEqual(json.loads(Path(report["owner_path"]).read_text())["state"], "pending")
        source.write_bytes(original)

    def test_programmatic_mutation_before_release_cleanup_preserves_target(self):
        target = self.repo / "cargo-target"
        target.mkdir()
        retained = target / "retained"
        retained.write_bytes(b"existing artifact")
        source = self.repo / "scripts/ci_plan.py"
        runner = self.runner("release-cleanup-source")
        write_owner = runner._write_release_owner

        def mutate_after_pending(*args):
            write_owner(*args)
            source.write_bytes(source.read_bytes() + b"\n# changed before cleanup\n")

        with patch.object(runner, "_write_release_owner", side_effect=mutate_after_pending):
            with self.assertRaisesRegex(local_ci.TargetCacheError, "before release cleanup"):
                with runner.guarded_release(adopt=True):
                    self.fail("release entered after source mutation")
        self.assertEqual(retained.read_bytes(), b"existing artifact")
        self.assertNotIn("cargo clean", self.call_log.read_text())
        self.assertIsNone(runner.target_lock)
        owner = Path(runner.summary["release_target_cache"]["owner_path"])
        self.assertEqual(json.loads(owner.read_text())["state"], "pending")

    def test_programmatic_mutation_during_release_cleanup_refuses_build(self):
        target = self.repo / "cargo-target"
        target.mkdir()
        (target / "existing").write_bytes(b"artifact")
        source = self.repo / "scripts/ci_plan.py"
        runner = self.runner("release-during-cleanup")
        original_run = subprocess.run

        def mutate_during_cleanup(argv, **kwargs):
            result = original_run(argv, **kwargs)
            if argv[:2] == ["cargo", "clean"]:
                source.write_bytes(source.read_bytes() + b"\n# changed during cleanup\n")
            return result

        with patch.object(local_ci.subprocess, "run", side_effect=mutate_during_cleanup):
            with self.assertRaisesRegex(local_ci.TargetCacheError, "during release preparation"):
                with runner.guarded_release(adopt=True):
                    self.fail("release entered after source mutation")
        self.assertIsNone(runner.target_lock)
        owner = Path(runner.summary["release_target_cache"]["owner_path"])
        self.assertEqual(json.loads(owner.read_text())["state"], "pending")

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

    def _reserved_release_runner(self, name):
        target = self.repo / name
        target.mkdir()
        env = dict(self.env, CARGO_TARGET_DIR=str(target), FAKE_METADATA_TARGET=str(target))
        runner = self.runner(name, env=env)
        self.assertTrue(runner._capture_repository_state("start_tree"),
                        runner.summary.get("repository_error"))
        reservation = local_ci.FreshTargetReservation.acquire(
            runner.root, str(target), {"run_id": name}, state_dir=self.target_state_dir)
        self.addCleanup(reservation.close)
        return runner, reservation, target

    def _release_owner_path(self, target):
        dev = local_ci.target_state_paths(target, self.target_state_dir)[1]
        return dev.with_name(dev.name.removesuffix(".owner.json") + ".release-owner.json")

    def _finish_reserved_release(self, runner, report, target):
        artifact = target / "release" / "release-fixture"
        artifact.parent.mkdir(parents=True, exist_ok=True)
        artifact.write_bytes(b"optimized reserved fixture")
        argv = ["cargo", "build", "--release"]
        report.update(build_argv=argv, build_exit_code=0,
                      artifact=runner.identify_release_artifact(artifact, argv))

    def test_reserved_release_publishes_ready_owner_while_holding_same_lock(self):
        name = "reserved-release-existing"
        runner, reservation, target = self._reserved_release_runner(name)
        lock = reservation._lock
        lock_file = lock.file
        owner = self._release_owner_path(target)
        with runner.guarded_release(reservation=reservation) as report:
            self.assertIs(runner.target_lock, lock)
            self.assertIs(lock.file, lock_file)
            self.assertEqual(report["state"], "pending")
            self.assertEqual(report["cleanup_status"], "not_needed")
            self._finish_reserved_release(runner, report, target)
        record = json.loads(owner.read_text())
        self.assertEqual(record["schema_version"], 1)
        self.assertEqual(record["profile"], "release")
        self.assertEqual(record["state"], "ready")
        self.assertEqual(record["target_directory"], str(target))
        self.assertEqual(record["target_identity"], runner._directory_identity(target))
        self.assertEqual(record["workspace_owner"], {
            key: runner.git_identity[key] for key in ("workspace_root", "git_common_dir")})
        self.assertEqual(record["artifact"]["sha256"], hashlib.sha256(
            b"optimized reserved fixture").hexdigest())
        self.assertEqual(report["state"], "ready")
        self.assertNotIn("cargo clean", self.call_log.read_text())
        self.assertIsNone(runner.target_lock)
        self.assertIsNone(lock.file)

    def test_reserved_release_real_offline_metadata_leaves_target_fresh(self):
        runner, reservation, target = self._reserved_dev_runner(
            "release-real-offline", real_cargo=True)
        runner.env["CARGO_CACHE_RUSTC_INFO"] = "1"
        with runner.guarded_release(reservation=reservation) as report:
            self.assertEqual(runner.env["CARGO_CACHE_RUSTC_INFO"], "0")
            self.assertEqual(list(target.iterdir()), [])
            self._finish_reserved_release(runner, report, target)
        self.assertEqual(runner.env["CARGO_CACHE_RUSTC_INFO"], "1")
        self.assertEqual(json.loads(self._release_owner_path(target).read_text())["state"], "ready")

    def test_reserved_release_refuses_metadata_side_effects_before_publication(self):
        runner, reservation, target = self._reserved_release_runner("reserved-release-metadata-entry")
        scope = runner._release_scope
        foreign = target / ".rustc_info.json"

        def metadata():
            packages = scope()
            foreign.write_bytes(b"unowned cache")
            return packages

        with patch.object(runner, "_release_scope", side_effect=metadata), \
                self.assertRaisesRegex(local_ci.TargetCacheError, "fresh target is not empty"):
            with runner.guarded_release(reservation=reservation):
                self.fail("release admitted unowned metadata output")
        self.assertEqual(foreign.read_bytes(), b"unowned cache")
        self.assertFalse(self._release_owner_path(target).exists())
        self.assertIsNone(runner.target_lock)

    def test_reserved_release_marker_collision_preserves_competitor_and_pending_sentinel(self):
        runner, reservation, target = self._reserved_release_runner("reserved-release-collision")
        owner = self._release_owner_path(target)
        lock = reservation._lock
        pending = local_ci.target_admission_pending_path(lock.path)
        competitor = b"competing release owner\n"
        original_link = os.link

        def collide(source, destination, *args, **kwargs):
            if os.fspath(destination) == owner.name:
                descriptor = os.open(owner, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                with os.fdopen(descriptor, "wb") as stream:
                    stream.write(competitor)
                    stream.flush()
                    os.fsync(stream.fileno())
            return original_link(source, destination, *args, **kwargs)

        with patch.object(local_ci.os, "link", side_effect=collide), \
                self.assertRaises(local_ci.TargetCacheError):
            with runner.guarded_release(reservation=reservation):
                self.fail("release entered after competing owner publication")
        self.assertEqual(owner.read_bytes(), competitor)
        self.assertTrue(pending.exists())
        self.assertNotIn("cargo clean", self.call_log.read_text())
        self.assertIsNone(runner.target_lock)

    def test_reserved_release_refuses_replacements_during_ready_marker_staging(self):
        for replacement in ("target", "owner", "staging"):
            with self.subTest(replacement=replacement):
                runner, reservation, target = self._reserved_release_runner(
                    f"reserved-release-replace-{replacement}")
                owner = self._release_owner_path(target)
                replacement_target = target.with_name(target.name + ".original")
                write_owner = runner._write_release_owner
                fsync = os.fsync
                changed = False

                def sync_then_replace(descriptor):
                    nonlocal changed
                    fsync(descriptor)
                    if changed:
                        return
                    changed = True
                    if replacement == "target":
                        target.rename(replacement_target)
                        target.mkdir()
                    elif replacement == "owner":
                        owner.unlink()
                        owner.write_bytes(b"competing pending owner\n")
                    else:
                        staging, = owner.parent.glob(f"{owner.name}.*.tmp")
                        staging.unlink()
                        staging.write_bytes(b"competing ready staging entry\n")

                def write_with_race(path, payload, **options):
                    with patch.object(local_ci.os, "fsync", side_effect=sync_then_replace):
                        write_owner(path, payload, **options)

                with patch.object(runner, "_write_release_owner", side_effect=write_with_race), \
                        self.assertRaises(local_ci.TargetCacheError):
                    with runner.guarded_release(reservation=reservation) as report:
                        self._finish_reserved_release(runner, report, target)
                self.assertTrue(changed)
                self.assertNotIn("cargo clean", self.call_log.read_text())
                if replacement == "target":
                    self.assertTrue(replacement_target.is_dir())
                    self.assertTrue(target.is_dir())
                    self.assertEqual(json.loads(owner.read_text())["state"], "pending")
                elif replacement == "owner":
                    self.assertEqual(owner.read_bytes(), b"competing pending owner\n")
                else:
                    self.assertEqual(json.loads(owner.read_text())["state"], "pending")
                    staging, = owner.parent.glob(f"{owner.name}.*.tmp")
                    self.assertEqual(staging.read_bytes(), b"competing ready staging entry\n")

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

    def test_release_fresh_target_accepts_only_metadata_created_during_locked_preflight(self):
        target = self.repo / "cargo-target"
        target.mkdir()
        runner = self.runner("fresh-metadata")
        scope = runner._release_scope

        def metadata():
            result = scope()
            (target / ".rustc_info.json").write_text("cargo metadata cache")
            return result

        with patch.object(runner, "_release_scope", side_effect=metadata):
            with runner.guarded_release() as report:
                self.assertEqual(report["cleanup_status"], "not_needed")
                artifact = target / "release-fixture"
                artifact.write_bytes(b"optimized fixture")
                argv = ["cargo", "build", "--release"]
                report.update(build_argv=argv, build_exit_code=0,
                              artifact=runner.identify_release_artifact(artifact, argv))

    def test_release_preserves_existing_or_new_unowned_preflight_files(self):
        target = self.repo / "cargo-target"
        target.mkdir()
        cache = target / ".rustc_info.json"
        cache.write_text("preexisting")
        runner = self.runner("existing-metadata")
        with patch.object(runner, "_release_scope") as scope:
            with self.assertRaisesRegex(local_ci.TargetCacheError, "ownership is unknown"):
                with runner.guarded_release():
                    self.fail("preexisting metadata must not acquire release custody")
            scope.assert_not_called()
        self.assertEqual(cache.read_text(), "preexisting")
        cache.unlink()

        runner = self.runner("concurrent-content")
        scope = runner._release_scope
        def metadata():
            result = scope()
            (target / "foreign").write_text("preserve")
            return result
        with patch.object(runner, "_release_scope", side_effect=metadata):
            with self.assertRaisesRegex(local_ci.TargetCacheError, "changed during preflight"):
                with runner.guarded_release():
                    self.fail("foreign content must not acquire release custody")
        self.assertEqual((target / "foreign").read_text(), "preserve")
        self.assertFalse(list(self.target_state_dir.glob("*.release-owner.json")))

    def test_release_refuses_target_replacement_or_metadata_symlink_during_preflight(self):
        target = self.repo / "cargo-target"
        target.mkdir()
        runner = self.runner("replaced-target")
        scope = runner._release_scope
        def replace_target():
            result = scope()
            target.rename(self.repo / "original-target")
            target.mkdir()
            return result
        with patch.object(runner, "_release_scope", side_effect=replace_target):
            with self.assertRaisesRegex(local_ci.TargetCacheError, "identity changed"):
                with runner.guarded_release():
                    self.fail("replaced target must not acquire release custody")
        target.rmdir()
        (self.repo / "original-target").rename(target)

        runner = self.runner("symlink-metadata")
        scope = runner._release_scope
        foreign = self.repo / "foreign"
        foreign.write_text("preserve")
        def link_metadata():
            result = scope()
            (target / ".rustc_info.json").symlink_to(foreign)
            return result
        with patch.object(runner, "_release_scope", side_effect=link_metadata):
            with self.assertRaisesRegex(local_ci.TargetCacheError, "changed during preflight"):
                with runner.guarded_release():
                    self.fail("metadata symlink must not acquire release custody")
        self.assertEqual(foreign.read_text(), "preserve")
        self.assertFalse(list(self.target_state_dir.glob("*.release-owner.json")))

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

    def _install_cli_policy_fixture(self):
        scripts = self.repo / "scripts"
        scripts.mkdir(exist_ok=True)
        (scripts / "ci_plan.py").write_bytes(local_ci._PLAN_SOURCE_BYTES)
        (scripts / "python_test_plan.py").write_bytes(local_ci.ci_plan._PYTHON_SOURCE_BYTES)
        for name, content in (("build_inputs.py", local_ci._INPUT_SOURCE_BYTES),
                              ("ci_checkpoint.py", local_ci._CHECKPOINT_SOURCE_BYTES),
                              ("ci_target_storage.py", local_ci._STORAGE_SOURCE_BYTES)):
            (scripts / name).write_bytes(content)

    def test_helper_source_load_ignores_timestamp_valid_stale_bytecode(self):
        scripts = self.base / "loader" / "scripts"
        scripts.mkdir(parents=True)
        source_scripts = SCRIPT.parent
        for name in ("local-ci.py", "ci_plan.py", "python_test_plan.py", "build_inputs.py",
                     "ci_checkpoint.py", "ci_target_storage.py"):
            shutil.copyfile(source_scripts / name, scripts / name)
        for name in ("build_inputs.py", "ci_checkpoint.py", "ci_target_storage.py"):
            path = scripts / name
            path.write_bytes(path.read_bytes() + b"\nHELPER_SOURCE_PROBE = 1\n")
            py_compile.compile(str(path), doraise=True,
                               invalidation_mode=py_compile.PycInvalidationMode.TIMESTAMP)
            stamp = path.stat()
            path.write_bytes(path.read_bytes().replace(b"HELPER_SOURCE_PROBE = 1",
                                                       b"HELPER_SOURCE_PROBE = 2"))
            os.utime(path, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
            control_spec = importlib.util.spec_from_file_location("stale_helper_control", path)
            control = importlib.util.module_from_spec(control_spec)
            control_spec.loader.exec_module(control)
            self.assertEqual(control.HELPER_SOURCE_PROBE, 1)
        spec = importlib.util.spec_from_file_location("helper_loader_fixture", scripts / "local-ci.py")
        loaded = importlib.util.module_from_spec(spec)
        with patch.dict(sys.modules, {spec.name: loaded}):
            spec.loader.exec_module(loaded)
        for helper in (loaded.build_inputs, loaded.ci_checkpoint, loaded.ci_target_storage):
            with self.subTest(helper=helper.__name__):
                self.assertEqual(helper.HELPER_SOURCE_PROBE, 2)
        self.assertIsNone(loaded.cli_policy_error(scripts.parent))

    def test_policy_loaders_refuse_symlinks_and_fifos_before_execution(self):
        cases = [(name, kind, "local-ci.py") for name, kind in (
            ("ci_plan.py", "symlink"), ("python_test_plan.py", "symlink"),
            ("build_inputs.py", "symlink"), ("ci_checkpoint.py", "symlink"),
            ("ci_target_storage.py", "symlink"), ("ci_plan.py", "fifo"),
            ("python_test_plan.py", "fifo"), ("build_inputs.py", "fifo"),
            ("ci_plan.py", "directory"), ("python_test_plan.py", "directory"),
            ("build_inputs.py", "directory"), ("ci_checkpoint.py", "directory"),
            ("ci_target_storage.py", "directory"))]
        cases.extend(("python_test_plan.py", kind, "ci_plan.py") for kind in ("symlink", "fifo"))
        for name, kind, entry in cases:
            with self.subTest(name=name, kind=kind, entry=entry), tempfile.TemporaryDirectory() as directory:
                root = Path(directory).resolve()
                scripts = root / "scripts"
                scripts.mkdir()
                for source in ("local-ci.py", "ci_plan.py", "python_test_plan.py", "build_inputs.py",
                               "ci_checkpoint.py", "ci_target_storage.py"):
                    shutil.copyfile(SCRIPT.parent / source, scripts / source)
                helper = scripts / name
                helper.unlink()
                marker = root / "executed"
                if kind == "symlink":
                    payload = root / "payload.py"
                    payload.write_text(f"from pathlib import Path\nPath({str(marker)!r}).write_text('executed')\n")
                    helper.symlink_to(payload)
                elif kind == "fifo":
                    os.mkfifo(helper)
                else:
                    helper.mkdir()
                result = subprocess.run([sys.executable, str(scripts / entry), "--help"],
                                        capture_output=True, timeout=2)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(marker.exists())

    def test_policy_source_descriptors_close_when_wrapping_fails(self):
        self._install_cli_policy_fixture()
        policy = self.repo / "scripts" / "ci_plan.py"
        original_open = os.open
        opened = []

        def track_open(*args, **kwargs):
            descriptor = original_open(*args, **kwargs)
            opened.append(descriptor)
            return descriptor

        with patch.object(os, "open", side_effect=track_open), \
                patch.object(os, "fdopen", side_effect=OSError("fixture wrap failure")):
            with self.assertRaisesRegex(OSError, "fixture wrap failure"):
                local_ci._read_policy_source(policy)
        self.assertEqual(len(opened), 2)
        for descriptor in opened:
            with self.assertRaises(OSError):
                os.fstat(descriptor)

    def test_cli_policy_descriptors_close_when_wrapping_fails(self):
        self._install_cli_policy_fixture()
        original_open = os.open
        opened = []

        def track_open(*args, **kwargs):
            descriptor = original_open(*args, **kwargs)
            opened.append(descriptor)
            return descriptor

        with patch.object(os, "open", side_effect=track_open), \
                patch.object(os, "fdopen", side_effect=OSError("fixture wrap failure")):
            self.assertIn("fixture wrap failure", local_ci.cli_policy_error(self.repo))
        self.assertEqual(len(opened), 2)
        for descriptor in opened:
            with self.assertRaises(OSError):
                os.fstat(descriptor)

    def test_cli_policy_closes_previous_file_before_later_wrap_failure(self):
        self._install_cli_policy_fixture()
        original_fdopen = os.fdopen
        original_close = os.close
        calls = []
        closed = []

        def fail_second(fd, *args, **kwargs):
            calls.append(fd)
            if len(calls) == 2:
                raise OSError("later fixture wrap failure")
            return original_fdopen(fd, *args, **kwargs)

        def track_close(fd):
            closed.append(fd)
            return original_close(fd)

        with patch.object(os, "fdopen", side_effect=fail_second), \
                patch.object(os, "close", side_effect=track_close):
            self.assertIn("later fixture wrap failure", local_ci.cli_policy_error(self.repo))
        self.assertEqual(len(calls), 2)
        self.assertEqual(closed[:2], calls)
        self.assertEqual(len(closed), 3)  # Both files and the scripts directory.
        with self.assertRaises(OSError):
            os.fstat(calls[-1])

    def test_helper_mutation_under_lock_preserves_owner_and_releases_lock(self):
        self._install_cli_policy_fixture()
        first = self.runner("helper-owner-first")
        first._cli_policy_bound = True
        self.assertEqual(first.run(), 0)
        owner = first.target_owner_path
        original_owner = owner.read_bytes()
        helper = self.repo / "scripts" / "ci_checkpoint.py"
        before = helper.read_bytes()
        second = self.runner("helper-owner-second")
        second._cli_policy_bound = True
        acquire = local_ci.CargoTargetLock.acquire
        def mutate_after_lock(lock):
            acquire(lock)
            helper.write_bytes(before + b"\n# changed\n")
        with patch.object(local_ci.CargoTargetLock, "acquire", mutate_after_lock):
            self.assertNotEqual(second.run(), 0)
        self.assertEqual(owner.read_bytes(), original_owner)
        self.assertIsNone(second.target_lock)
        self.assertIn("planner source", second.summary["target_cache_error"])

    def test_helper_mutation_during_cargo_leaves_owner_unchecked(self):
        self._install_cli_policy_fixture()
        first = self.runner("helper-cargo-first")
        first._cli_policy_bound = True
        self.assertEqual(first.run(), 0)
        helper = self.repo / "scripts" / "ci_target_storage.py"
        original = helper.read_bytes()
        runner = self.runner("helper-cargo-mutated")
        runner._cli_policy_bound = True
        original_step = runner._run_step
        def mutate_after_step(step, index):
            result = original_step(step, index)
            helper.write_bytes(original + b"\n# changed during cargo\n")
            return result
        with patch.object(runner, "_run_step", side_effect=mutate_after_step):
            self.assertNotEqual(runner.run(), 0)
        self.assertFalse(json.loads(runner.target_owner_path.read_text())["cache_inputs"]["workspace"]["checked"])
        self.assertIsNone(runner.target_lock)
        self.assertNotEqual(runner.summary["status"], "passed")

    def test_cli_helper_policy_is_checked_against_selected_root(self):
        self._install_cli_policy_fixture()
        other = self.base / "other-source"
        shutil.copytree(self.repo / "scripts", other / "scripts")
        self.assertIsNone(local_ci.cli_policy_error(other))
        path = other / "scripts" / "build_inputs.py"
        path.write_bytes(path.read_bytes() + b"\n# different root\n")
        self.assertIsNotNone(local_ci.cli_policy_error(other))
        self.assertIsNone(local_ci.cli_policy_error(self.repo))
        runner = self.runner("helper-other", root=other)
        runner._cli_policy_bound = True
        self.assertNotEqual(runner.run(), 0)
        self.assertIn("cli_policy_error", runner.summary)

    def test_cli_policy_refuses_change_before_plan_resolution(self):
        self._install_cli_policy_fixture()
        for name in ("ci_plan.py", "python_test_plan.py", "build_inputs.py",
                     "ci_checkpoint.py", "ci_target_storage.py"):
            with self.subTest(name=name):
                policy = self.repo / "scripts" / name
                original = policy.read_bytes()

                def metadata(*_args, **_kwargs):
                    policy.write_bytes(original + b"\n# changed while resolving\n")
                    return SimpleNamespace(stdout="{}")

                with patch.object(local_ci, "ROOT", self.repo), \
                        patch.object(local_ci, "command_output", side_effect=metadata), \
                        patch.object(local_ci.ci_plan, "resolve", side_effect=AssertionError("stale planner was used")) as resolve:
                    with self.assertRaises(SystemExit):
                        local_ci.main(["--profile", "merge", "--base", "base", "--source", "source"])
                    resolve.assert_not_called()
                policy.write_bytes(original)

    def test_cli_policy_matching_source_reaches_plan_resolution(self):
        self._install_cli_policy_fixture()
        with patch.object(local_ci, "ROOT", self.repo), \
                patch.object(local_ci, "command_output", return_value=SimpleNamespace(stdout="{}")), \
                patch.object(local_ci.ci_plan, "resolve", side_effect=ValueError("resolution reached")) as resolve:
            with self.assertRaises(SystemExit):
                local_ci.main(["--profile", "merge", "--base", "base", "--source", "source"])
            resolve.assert_called_once()

    def _isolate_fresh_cli(self):
        state_paths = local_ci.target_state_paths
        def isolated_paths(target, state_dir=None):
            return state_paths(target, state_dir or self.target_state_dir)

        paths = patch.object(local_ci, "target_state_paths", side_effect=isolated_paths)
        paths.start()
        self.addCleanup(paths.stop)

    def test_fresh_target_cli_refuses_absent_before_any_metadata(self):
        self._isolate_fresh_cli()
        target = self.repo / "fresh-cli-absent"
        for scoped in (False, True):
            arguments = ["--fresh-target"]
            if scoped:
                arguments.extend(["--profile", "merge", "--base", "base", "--source", "source"])
            with self.subTest(scoped=scoped), patch.object(local_ci, "ROOT", self.repo), \
                    patch.dict(os.environ, {"CARGO_TARGET_DIR": str(target)}), \
                    patch.object(local_ci, "command_output") as metadata, \
                    patch.object(local_ci, "LocalCiRunner") as runner, \
                    redirect_stderr(io.StringIO()) as errors:
                with self.assertRaises(SystemExit) as refused:
                    local_ci.main(arguments)
            self.assertEqual(refused.exception.code, 2)
            self.assertIn("create an empty directory", errors.getvalue())
            metadata.assert_not_called()
            runner.assert_not_called()
            self.assertFalse(target.exists())

    def test_fresh_target_scoped_metadata_is_locked_and_target_bound(self):
        self._isolate_fresh_cli()
        target = self.base / "fresh-scoped-target"
        target.mkdir()
        foreign = self.base / "foreign-target"
        foreign.mkdir()
        lock_path, _ = local_ci.target_state_paths(target)
        observations = []

        def metadata(*_args, **_kwargs):
            contender = local_ci.CargoTargetLock(lock_path, {"run_id": "metadata-contender"})
            with self.assertRaisesRegex(local_ci.TargetCacheError, "already in use"):
                contender.acquire()
            observations.append("locked")
            return subprocess.CompletedProcess([], 0, stdout=json.dumps({
                "target_directory": str(foreign), "workspace_root": str(self.repo),
                "workspace_members": [], "packages": [],
            }))

        with patch.object(local_ci, "ROOT", self.repo), \
                patch.dict(os.environ, {"CARGO_TARGET_DIR": str(target)}), \
                patch.object(local_ci, "command_output", side_effect=metadata), \
                patch.object(local_ci.ci_plan, "resolve") as resolve:
            with self.assertRaises(SystemExit):
                local_ci.main(["--fresh-target", "--profile", "merge", "--base", "base", "--source", "source"])
            resolve.assert_not_called()
        self.assertEqual(observations, ["locked"])
        self.assertFalse(list(target.iterdir()))

    def test_fresh_target_scoped_metadata_refuses_preexisting_lock(self):
        self._isolate_fresh_cli()
        target = self.base / "fresh-scoped-contended"
        target.mkdir()
        lock_path, _ = local_ci.target_state_paths(target)
        owner = local_ci.CargoTargetLock(lock_path, {"run_id": "existing-owner"})
        owner.acquire()
        self.addCleanup(owner.release)
        metadata = Mock(return_value=SimpleNamespace(stdout="{}"))
        with patch.object(local_ci, "ROOT", self.repo), \
                patch.dict(os.environ, {"CARGO_TARGET_DIR": str(target)}), \
                patch.object(local_ci, "command_output", metadata):
            with self.assertRaises(SystemExit):
                local_ci.main(["--fresh-target", "--profile", "merge", "--base", "base", "--source", "source"])
        metadata.assert_not_called()

    def test_fresh_target_full_metadata_side_effect_refuses_before_build(self):
        self._isolate_fresh_cli()
        target = self.repo / "fresh-full-target"
        target.mkdir()
        env = dict(self.env, CARGO_TARGET_DIR=str(target), FAKE_METADATA_TARGET=str(target),
                   METADATA_WRITE_TARGET_FILE="1")
        lock_path, owner_path = local_ci.target_state_paths(target)
        original_subprocess_run = subprocess.run
        observations = []

        def checked_subprocess_run(argv, *args, **kwargs):
            if argv[:2] == ["cargo", "metadata"]:
                contender = local_ci.CargoTargetLock(lock_path, {"run_id": "metadata-contender"})
                with self.assertRaisesRegex(local_ci.TargetCacheError, "already in use"):
                    contender.acquire()
                observations.append("locked-before-metadata")
            return original_subprocess_run(argv, *args, **kwargs)

        runners = []
        runner_class = local_ci.LocalCiRunner

        def make_runner(**options):
            runner = runner_class(self.repo, target_state_dir=self.target_state_dir, **options)
            runners.append(runner)
            return runner

        with patch.object(local_ci, "ROOT", self.repo), \
                patch.dict(os.environ, env), \
                patch.object(local_ci, "planned_steps", return_value=(
                    local_ci.Step("one-build", ("cargo", "build")),)), \
                patch.object(local_ci.subprocess, "run", side_effect=checked_subprocess_run), \
                patch.object(local_ci, "LocalCiRunner", side_effect=make_runner):
            with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                exit_code = local_ci.main(["--fresh-target"])

        self.assertEqual(exit_code, 2)
        self.assertEqual(observations, ["locked-before-metadata"])
        self.assertEqual(len(runners), 1)
        runner = runners[0]
        self.assertIn("cargo_metadata_error", runner.summary)
        self.assertTrue((target / ".rustc_info.json").is_file())
        self.assertFalse(owner_path.exists())
        self.assertTrue(all(entry["status"] == "not_run" for entry in runner.summary["steps"]))
        self.assertIsNone(runner.target_lock)

    def test_fresh_target_cli_admits_full_and_scoped_under_one_lock(self):
        self._isolate_fresh_cli()
        runner_class = local_ci.LocalCiRunner
        run = subprocess.run
        for scoped in (False, True):
            with self.subTest(scoped=scoped):
                target = self.repo / f"fresh-cli-{scoped}"
                target.mkdir()
                lock_path, owner_path = local_ci.target_state_paths(target)
                observed = []
                runners = []

                def check_metadata(argv, *args, **options):
                    if argv[:2] == ["cargo", "metadata"]:
                        contender = local_ci.CargoTargetLock(lock_path, {"run_id": "contender"})
                        with self.assertRaisesRegex(local_ci.TargetCacheError, "already in use"):
                            contender.acquire()
                        observed.append(options["env"].get("CARGO_CACHE_RUSTC_INFO"))
                    return run(argv, *args, **options)

                def make_runner(**options):
                    runner = runner_class(self.repo, target_state_dir=self.target_state_dir, **options)
                    runners.append(runner)
                    return runner

                arguments = ["--fresh-target"]
                if scoped:
                    arguments.extend(["--profile", "edit", "--base", "base", "--source", "source"])
                env = dict(self.env, CARGO_TARGET_DIR=str(target), FAKE_METADATA_TARGET=str(target),
                           CARGO_CACHE_RUSTC_INFO="1")
                steps = (local_ci.Step("build", ("cargo", "build")),)
                with patch.object(local_ci, "ROOT", self.repo), patch.dict(os.environ, env), \
                        patch.object(local_ci.ci_plan, "resolve", return_value={"qualifies_merge": False}), \
                        patch.object(local_ci, "planned_steps", return_value=steps), \
                        patch.object(local_ci, "LocalCiRunner", side_effect=make_runner), \
                        patch.object(local_ci.subprocess, "run", side_effect=check_metadata), \
                        redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                    self.assertEqual(local_ci.main(arguments), 0)

                self.assertEqual(observed, ["0"] * (3 if scoped else 2) + ["1"])
                runner, = runners
                cache = runner.summary["target_cache"]
                self.assertEqual(cache["action"], "claim-fresh")
                self.assertEqual(cache["target_identity"], runner._directory_identity(target))
                self.assertEqual(runner._load_target_owner(owner_path, target)["dependency_inputs"],
                                 cache["dependency_inputs"])
                self.assertIsNone(runner.target_lock)
        self.assertNotIn("cargo clean", self.call_log.read_text())

    def test_direct_fresh_target_rejects_runner_source_drift_before_cargo(self):
        scripts = self.repo / "scripts"
        planner = scripts / "ci_plan.py"
        planner.write_bytes(planner.read_bytes() + b'''
with Path(__file__).with_name("local-ci.py").open("ab") as runner:
    runner.write(b"\\n# changed after direct runner compilation\\n")
''')
        target = self.repo / "fresh-direct"
        result = subprocess.run([sys.executable, str(scripts / "local-ci.py"), "--fresh-target"],
                                cwd=self.repo, env=dict(self.env, CARGO_TARGET_DIR=str(target)),
                                capture_output=True, text=True, timeout=8)
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn("executed runner differs from candidate source", result.stderr)
        self.assertFalse(target.exists())
        self.assertFalse(self.call_log.exists())

    def test_fresh_target_run_admits_real_offline_metadata(self):
        fixture, reservation, target = self._reserved_dev_runner(
            "fresh-real-run", real_cargo=True)
        step = local_ci.Step("probe", (sys.executable, "-c", "pass"), uses_cargo_target=True)
        runner = local_ci.LocalCiRunner(self.repo, mode="quick", run_id="real-fresh-ci",
                                        env=fixture.env, steps=(step,), min_free_bytes=0,
                                        target_state_dir=self.target_state_dir)
        self.assertEqual(runner.run(reservation=reservation), 0)
        cache = runner.summary["target_cache"]
        self.assertEqual(cache["action"], "claim-fresh")
        self.assertEqual(cache["target_identity"], runner._directory_identity(target))
        self.assertEqual(runner._load_target_owner(runner.target_owner_path, target)["dependency_inputs"],
                         cache["dependency_inputs"])
        self.assertIsNone(runner.target_lock)

    def test_fresh_target_runner_claim_keeps_reservation_lock_without_cleanup(self):
        target = self.repo / "fresh-claim-success"
        target.mkdir()
        env = dict(self.env, CARGO_TARGET_DIR=str(target), FAKE_METADATA_TARGET=str(target))
        runner = local_ci.LocalCiRunner(
            self.repo, mode="quick", run_id="fresh-claim-success", env=env,
            steps=(local_ci.Step("build", ("cargo", "build")),), min_free_bytes=0,
            target_state_dir=self.target_state_dir)
        reservation = local_ci.FreshTargetReservation.acquire(
            self.repo, str(target), {"run_id": "fresh-claim-success"}, state_dir=self.target_state_dir)
        self.addCleanup(reservation.close)
        lock = reservation._lock
        transferred = []
        original_claim = runner._claim_fresh_dev_owner

        def record_claim(current):
            identity = original_claim(current)
            transferred.append(runner.target_lock)
            return identity

        with patch.object(runner, "_claim_fresh_dev_owner", side_effect=record_claim):
            self.assertEqual(runner.run(reservation=reservation), 0)

        cache = runner.summary["target_cache"]
        self.assertEqual(cache["action"], "claim-fresh")
        self.assertIsNone(cache["cleanup_argv"])
        self.assertEqual(cache["cleanup_status"], "not_needed")
        self.assertEqual(transferred, [lock])
        self.assertFalse(local_ci.target_admission_pending_path(lock.path).exists())
        self.assertIn("cargo build", self.call_log.read_text())
        self.assertNotIn("cargo clean", self.call_log.read_text())

    def test_cli_policy_refuses_change_during_plan_resolution(self):
        self._install_cli_policy_fixture()
        policy = self.repo / "scripts" / "python_test_plan.py"

        def resolve(*_args, **_kwargs):
            policy.write_bytes(policy.read_bytes() + b"\n# changed while planning\n")
            return {}

        with patch.object(local_ci, "ROOT", self.repo), \
                patch.object(local_ci, "command_output", return_value=SimpleNamespace(stdout="{}")), \
                patch.object(local_ci.ci_plan, "resolve", side_effect=resolve), \
                patch.object(local_ci, "LocalCiRunner") as runner:
            with self.assertRaises(SystemExit):
                local_ci.main(["--profile", "merge", "--base", "base", "--source", "source"])
            runner.assert_not_called()

    def test_cli_policy_rechecks_after_non_cargo_steps(self):
        self._install_cli_policy_fixture()
        policy = self.repo / "scripts" / "python_test_plan.py"
        runner = self.runner("cli-policy-final", env={**self.env, "MUTATE_FILE": str(policy)},
                             steps=(local_ci.Step("first", ("first",)),))
        runner._cli_policy_bound = True
        self.assertNotEqual(runner.run(), 0)
        self.assertIn("cli_policy_error", runner.summary)
        self.assertNotEqual(runner.summary["status"], "passed")

    def test_cli_policy_missing_mutated_and_symlink_refuse(self):
        self._install_cli_policy_fixture()
        self.assertIsNone(local_ci.cli_policy_error(self.repo))
        scripts = self.repo / "scripts"
        for name in ("ci_plan.py", "python_test_plan.py", "build_inputs.py",
                     "ci_checkpoint.py", "ci_target_storage.py"):
            path = scripts / name
            original = path.read_bytes()
            path.write_bytes(original + b"\n# mutation\n")
            self.assertIsNotNone(local_ci.cli_policy_error(self.repo))
            path.unlink()
            self.assertIsNotNone(local_ci.cli_policy_error(self.repo))
            target = scripts / (name + ".target")
            target.write_bytes(original)
            path.symlink_to(target.name)
            self.assertIsNotNone(local_ci.cli_policy_error(self.repo))
            path.unlink()
            target.unlink()
            path.write_bytes(original)
        moved = self.repo / "real-scripts"
        scripts.rename(moved)
        scripts.symlink_to(moved.name, target_is_directory=True)
        self.assertIsNotNone(local_ci.cli_policy_error(self.repo))

    def test_cli_policy_rechecks_before_snapshot_and_under_lock(self):
        self._install_cli_policy_fixture()
        policy = self.repo / "scripts" / "ci_plan.py"
        original = policy.read_bytes()
        before = self.runner("cli-policy-before")
        before._cli_policy_bound = True
        policy.write_bytes(original + b"\n# changed after import\n")
        self.assertNotEqual(before.run(), 0)
        self.assertIn("cli_policy_error", before.summary)
        self.assertNotIn("cargo build", self.call_log.read_text() if self.call_log.exists() else "")

        policy.write_bytes(original)
        locked = self.runner("cli-policy-lock")
        locked._cli_policy_bound = True
        acquire = local_ci.CargoTargetLock.acquire
        def mutate_after_lock(lock):
            acquire(lock)
            policy.write_bytes(original + b"\n# changed before locked validation\n")
        with patch.object(local_ci.CargoTargetLock, "acquire", mutate_after_lock):
            self.assertNotEqual(locked.run(), 0)
        self.assertIn("CLI planner source changed before target lock", locked.summary["target_cache_error"])
        self.assertFalse(locked.target_owner_path.exists())
        self.assertNotIn("cargo build", self.call_log.read_text())

    def test_cli_policy_matching_run_passes_and_lock_refusal_preserves_owner(self):
        self._install_cli_policy_fixture()
        first = self.runner("cli-policy-matching")
        first._cli_policy_bound = True
        self.assertEqual(first.run(), 0)
        owner = first.target_owner_path
        original_owner = owner.read_bytes()
        policy = self.repo / "scripts" / "python_test_plan.py"
        second = self.runner("cli-policy-owner")
        second._cli_policy_bound = True
        acquire = local_ci.CargoTargetLock.acquire

        def mutate_after_lock(lock):
            acquire(lock)
            policy.write_bytes(policy.read_bytes() + b"\n# changed after lock\n")

        with patch.object(local_ci.CargoTargetLock, "acquire", mutate_after_lock):
            self.assertNotEqual(second.run(), 0)
        self.assertEqual(owner.read_bytes(), original_owner)
        self.assertIn("CLI planner source changed before target lock", second.summary["target_cache_error"])

    def test_cli_policy_atomic_replacement_during_cargo_keeps_owner_unchecked(self):
        self._install_cli_policy_fixture()
        first = self.runner("cli-policy-first")
        first._cli_policy_bound = True
        self.assertEqual(first.run(), 0)
        policy = self.repo / "scripts" / "ci_plan.py"
        original = policy.read_bytes()
        timestamp = policy.stat()
        runner = self.runner("cli-policy-replaced")
        runner._cli_policy_bound = True
        run_step = runner._run_step

        def replace_after_step(step, index):
            result = run_step(step, index)
            replacement = policy.with_name("replacement.py")
            replacement.write_bytes(original[:-1] + bytes([original[-1] ^ 1]))
            os.utime(replacement, ns=(timestamp.st_atime_ns, timestamp.st_mtime_ns))
            os.replace(replacement, policy)
            return result

        with patch.object(runner, "_run_step", side_effect=replace_after_step):
            self.assertNotEqual(runner.run(), 0)
        self.assertNotEqual(runner.summary["status"], "passed")
        owner = json.loads(runner.target_owner_path.read_text())
        self.assertFalse(owner["cache_inputs"]["workspace"]["checked"])
        policy.write_bytes(original)
        os.utime(policy, ns=(timestamp.st_atime_ns, timestamp.st_mtime_ns))
        recovery = self.runner("cli-policy-recovery")
        recovery._cli_policy_bound = True
        self.assertEqual(recovery.run(), 0)
        self.assertEqual(recovery.summary["target_cache"]["action"], "source-or-compiler-handoff")

    def test_native_acceptance_fresh_full_and_scoped_reports_observe_actual_runner_environment(self):
        for mode, selected in (("full", None), ("merge", {"scope": "tooling", "qualifies_merge": False})):
            for key in (None, "HEX_VST3_SDK_DIR", "HEX_VST3_FUTURE_FIXTURE", "HEX_TEST_SIGNING_IDENTITY"):
                name = "native-fresh-" + mode + "-" + (key or "ordinary")
                env = {**self.env, **({key: "fixture-only"} if key else {"HEX_VST3_SDK_DIR": ""})}
                runner = local_ci.LocalCiRunner(self.repo, mode=mode, run_id=name, env=env, plan=selected,
                    steps=(local_ci.Step("first", ("first",)),), min_free_bytes=0,
                    target_state_dir=self.target_state_dir)
                with self.subTest(mode=mode, key=key):
                    self.assertEqual(runner.run(), 0)
                    report = json.loads(runner.summary_path.read_text())
                    self.assertIs(report["native_acceptance_opt_in"], key is not None)
                    self.assertEqual(report["steps"][0]["status"], "passed")
                    self.assertEqual(report["status"], "passed" if selected is None else "partial")

    def test_native_acceptance_stale_reuse_plan_refuses_before_cargo_identity_target_or_steps(self):
        target = Path(self.env["FAKE_METADATA_TARGET"])
        target.mkdir()
        sentinel = target / "foreign-cache"
        sentinel.write_bytes(b"preserve target bytes")
        self.target_state_dir.mkdir()
        marker = self.target_state_dir / "existing-owner.json"
        marker.write_bytes(b"preserve owner bytes")
        report = self.base / "original-report.json"
        report.write_bytes(b"original immutable runtime report")
        cases = [("current-sdk", {"HEX_VST3_SDK_DIR": "fixture-sdk"}, False),
                 ("current-future", {"HEX_VST3_FUTURE_FIXTURE": "fixture"}, False),
                 ("current-signing", {"HEX_TEST_SIGNING_IDENTITY": "fixture identity"}, False),
                 ("missing", {}, "missing"), ("native", {}, True), ("unknown", {}, None),
                 ("zero", {}, 0), ("string", {}, "false")]
        for label, native_env, flag in cases:
            receipt = {"report": str(report), "source": "deadbeef", "dependency_inputs": {}}
            if flag != "missing":
                receipt["native_acceptance_opt_in"] = flag
            selected = {"scope": "metadata", "profile": "merge", "qualifies_merge": True,
                        "source": "deadbeef", "runtime_inputs": local_ci.ci_plan.runtime_inputs(self.repo, env=self.env),
                        "reuse": receipt}
            original_plan = json.dumps(selected, sort_keys=True)
            runner = local_ci.LocalCiRunner(self.repo, mode="merge", run_id="native-refusal-" + label,
                env=self.env, plan=selected,
                steps=(local_ci.Step("must-not-run", ("cargo", "test", "fixture")),), min_free_bytes=0,
                target_state_dir=self.target_state_dir)
            # The actual environment may change after construction/resolution.
            runner.env.update(native_env)
            with self.subTest(case=label), \
                    patch.object(runner, "_resolve_cargo_identity", wraps=runner._resolve_cargo_identity) as identity, \
                    patch.object(runner, "_prepare_target_cache", wraps=runner._prepare_target_cache) as prepare, \
                    patch.object(runner, "_run_step", wraps=runner._run_step) as execute:
                self.assertEqual(runner.run(), 2)
                self.assertIn("native/external", runner.summary["reuse_error"])
                identity.assert_not_called()
                prepare.assert_not_called()
                execute.assert_not_called()
                self.assertTrue(all(row["status"] == "not_run" for row in runner.summary["steps"]))
                self.assertIs(runner.summary["native_acceptance_opt_in"], bool(native_env))
                self.assertEqual(json.dumps(selected, sort_keys=True), original_plan)
                self.assertEqual(report.read_bytes(), b"original immutable runtime report")
                self.assertEqual({p.name: p.read_bytes() for p in target.iterdir()},
                                 {"foreign-cache": b"preserve target bytes"})
                self.assertEqual({p.name: p.read_bytes() for p in self.target_state_dir.iterdir()},
                                 {"existing-owner.json": b"preserve owner bytes"})
        self.assertNotIn("cargo ", self.call_log.read_text())

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

    def checkpoint_fixture(self, name, *, stop="disk", env_overrides=None):
        """Real runner/owner/log behavior, with only tools and disk capacity faked."""
        root = self.repo / name
        root.mkdir()
        install_delivery_source_fixture(root)
        (root / "source.rs").write_text("original source\n")
        target = root / "cargo-target"
        target.mkdir()
        dependency = self.base / (name + "-dependency")
        dependency.mkdir()
        (dependency / "Cargo.toml").write_text('[package]\nname="helper"\nversion="1.0.0"\n')
        (dependency / "lib.rs").write_text("pub fn unchanged() {}\n")
        metadata = {
            "target_directory": str(target), "workspace_root": str(root),
            "workspace_members": ["fixture 0.1.0", "other 0.1.0"],
            "packages": [
                {"id": "fixture 0.1.0", "name": "fixture", "version": "0.1.0", "manifest_path": str(root / "Cargo.toml")},
                {"id": "other 0.1.0", "name": "other", "version": "0.1.0", "manifest_path": str(root / "other/Cargo.toml")},
                {"id": "helper 1.0.0", "name": "helper", "version": "1.0.0", "source": None, "manifest_path": str(dependency / "Cargo.toml")},
            ],
        }
        script = "\n".join(
            '    printf \'%s\\n\' "$CHECKPOINT_METADATA"' if line.lstrip().startswith("printf '{\"target_directory\"") else line
            for line in FAKE_TOOL.splitlines()
        )
        script += """
case "$tool $*" in
  'cargo test checkpoint-suffix')
    if [ "$CHECKPOINT_STOP" = interrupted ]; then kill -TERM "$PPID"; exit 0; fi ;;
esac
if [ "$tool" = cargo ]; then
  echo "test result: ok. $CHECKPOINT_TEST_COUNT passed; 0 failed; 0 ignored; finished in 0.01s"
fi
"""
        (self.bin / "cargo").write_text(script)
        env = {**self.env, "FAKE_TOPLEVEL": str(root), "FAKE_COMMON_DIR": str(root / ".git"),
               "FAKE_WORKSPACE_ROOT": str(root), "FAKE_METADATA_TARGET": str(target),
               "FAKE_FILE_LIST": "source.rs", "CHECKPOINT_METADATA": json.dumps(metadata),
               "CHECKPOINT_STOP": stop or "none", "CHECKPOINT_TEST_COUNT": "0" if stop == "zero" else "2"}
        env.update(env_overrides or {})
        if stop == "failed":
            env["FAIL_MATCH"] = "checkpoint-suffix"
        steps = tuple(local_ci.Step(name, ("cargo", "test", name), requires_tests=True)
                      for name in ("checkpoint-prefix", "checkpoint-suffix"))
        case = SimpleNamespace(root=root, target=target, dependency=dependency, env=env, steps=steps)
        original = self.checkpoint_runner(case, "stopped")
        available = SimpleNamespace(total=100 * local_ci.GIB, free=4 * local_ci.GIB)
        full = SimpleNamespace(total=100 * local_ci.GIB, free=0)
        usage = [available] * 4 + [full] if stop == "disk" else [available] * 6
        with patch.object(local_ci.shutil, "disk_usage", side_effect=usage):
            result = original.run()
        self.assertEqual(result, {"disk": 2, "failed": 1, "zero": 1, "interrupted": 130, None: 0}[stop])
        case.path = original.summary_path
        case.report = json.loads(case.path.read_text())
        case.owner = original.target_owner_path
        if stop == "disk":
            self.assertEqual([row["status"] for row in case.report["steps"]], ["passed", "not_run"])
            self.assertEqual(case.report["checkpoint"]["passed_count"], 1)
            self.assertTrue(case.report["checkpoint"]["target"]["target_identity"])
        return case

    def checkpoint_runner(self, case, run_id, **kwargs):
        return local_ci.LocalCiRunner(case.root, mode="full", run_id=run_id,
            env=kwargs.pop("env", case.env), steps=kwargs.pop("steps", case.steps),
            min_free_bytes=kwargs.pop("min_free_bytes", None), target_state_dir=self.target_state_dir, **kwargs)

    def checkpoint_continue(self, case, **kwargs):
        resumed = self.checkpoint_runner(case, "continued", resume_report=case.path, **kwargs)
        with patch.object(local_ci.shutil, "disk_usage", return_value=SimpleNamespace(total=100 * local_ci.GIB, free=4 * local_ci.GIB)):
            result = resumed.run()
        return result, resumed

    def test_checkpoint_continuation_runs_only_suffix_after_owned_cache_cleanup(self):
        case = self.checkpoint_fixture("checkpoint-success")
        original_bytes = case.path.read_bytes()
        prefix_log = case.root / case.report["steps"][0]["log"]
        log_bytes = prefix_log.read_bytes()
        # Supported cleanup removes only inactive owned artifacts, retaining
        # the physical target directory and its external ownership receipt.
        inactive = case.target / "debug"
        inactive.mkdir()
        (inactive / "inactive.rlib").write_bytes(b"disposable compiler artifact")
        shutil.rmtree(inactive)
        before_calls = self.call_log.read_text()
        with patch.object(local_ci.ci_checkpoint, "require_no_jobs"):
            result, resumed = self.checkpoint_continue(case)
        self.assertEqual(result, 0)
        calls = self.call_log.read_text()[len(before_calls):]
        self.assertIn("cargo test checkpoint-suffix", calls)
        self.assertNotIn("cargo test checkpoint-prefix", calls)
        # A stopped owner is not a complete cache certificate. The existing
        # target guard may conservatively clean its owned dev packages; this
        # does not erase the immutable passed-stage observations.
        self.assertIn("cargo clean --profile dev --target-dir " + str(case.target), calls)
        self.assertIn("--package fixture --package other", calls)
        self.assertEqual(case.path.read_bytes(), original_bytes)

        self.assertEqual(prefix_log.read_bytes(), log_bytes)
        report = json.loads(resumed.summary_path.read_text())
        self.assertEqual(report["status"], "passed")
        self.assertEqual(report["target_cache"]["cleanup_status"], "passed")
        self.assertEqual([row["execution"] for row in report["steps"]], ["inherited", "executed"])
        inherited = report["steps"][0]
        self.assertIsNone(inherited["duration_seconds"])
        self.assertIsNone(inherited["started_at"])
        self.assertEqual(inherited["test_results"], case.report["steps"][0]["test_results"])
        self.assertEqual(inherited["inherited_from"]["sha256"], hashlib.sha256(original_bytes).hexdigest())
        self.assertEqual(inherited["inherited_from"]["log"]["sha256"], hashlib.sha256(log_bytes).hexdigest())
        self.assertFalse((resumed.output_dir / "01-checkpoint-prefix.log").exists())
        local_ci.ci_checkpoint.validate_inheritance(case.root, report)

        # Exercise the delivery consumer against this small complete fake plan;
        # no native build or signing is invoked.
        delivery = load_delivery_source_fixture("checkpoint_delivery")
        with patch.object(delivery.ci, "FULL_STEPS", case.steps):
            self.assertEqual(delivery.validate_ci(resumed.summary_path, resumed, report["commit"]), report)
            forged = json.loads(resumed.summary_path.read_text())
            forged["steps"][0]["test_results"]["rust_passed_by_binary"] = [99]
            resumed.summary_path.write_text(json.dumps(forged))
            with self.assertRaisesRegex(delivery.DeliveryError, "inherited"):
                delivery.validate_ci(resumed.summary_path, resumed, report["commit"])
        self.assertEqual(case.path.read_bytes(), original_bytes)

    def test_checkpoint_continuation_ignores_only_transport_call_id_environment_drift(self):
        case = self.checkpoint_fixture("checkpoint-call-id", env_overrides={"CODEX_TOOL_CALL_ID": "call-A"})
        resumed_env = {**case.env, "CODEX_TOOL_CALL_ID": "call-B"}
        inputs = local_ci.ci_checkpoint.execution_inputs(case.root, case.env, case.steps)
        self.assertEqual(inputs, local_ci.ci_checkpoint.execution_inputs(case.root, resumed_env, case.steps))
        semantic = local_ci.ci_checkpoint.execution_inputs(case.root, {**resumed_env, "RUSTFLAGS": "--cfg changed"}, case.steps)
        self.assertNotEqual(inputs["environment_sha256"], semantic["environment_sha256"])
        original_bytes = case.path.read_bytes()
        calls_before = self.call_log.read_text()
        with patch.object(local_ci.ci_checkpoint, "require_no_jobs"):
            result, resumed = self.checkpoint_continue(case, env=resumed_env)
        self.assertEqual(result, 0)
        calls = self.call_log.read_text()[len(calls_before):]
        self.assertNotIn("cargo test checkpoint-prefix", calls)
        self.assertEqual(calls.count("cargo test checkpoint-suffix"), 1)
        self.assertEqual([row["execution"] for row in resumed.summary["steps"]], ["inherited", "executed"])
        self.assertEqual(case.path.read_bytes(), original_bytes)
        local_ci.ci_checkpoint.validate_inheritance(case.root, resumed.summary)

    def assert_checkpoint_drift_refused(self, change):
        case = self.checkpoint_fixture("checkpoint-drift-" + change)
        kwargs = {}
        rustc_bytes = (self.bin / "rustc").read_bytes()
        if change == "source":
            (case.root / "source.rs").write_text("changed executable source\n")
        elif change == "plan":
            kwargs["plan"] = {"qualifies_merge": False, "scope": "full"}
        elif change == "plan-order":
            kwargs["steps"] = tuple(reversed(case.steps))
        elif change in {"argv", "test-configuration"}:
            kwargs["steps"] = (case.steps[0], local_ci.Step("checkpoint-suffix",
                (*case.steps[1].argv, "changed") if change == "argv" else case.steps[1].argv,
                requires_tests=change != "test-configuration"))
        elif change == "toolchain":
            (self.bin / "rustc").write_bytes(rustc_bytes + b"\n# changed compiler bytes\n")
        elif change == "environment":
            kwargs["env"] = {**case.env, "RUSTFLAGS": "--cfg changed"}
        elif change == "dependency":
            (case.dependency / "lib.rs").write_text("pub fn changed() {}\n")
        elif change == "floor":
            kwargs["min_free_bytes"] = 1
        elif change == "report":
            report = json.loads(case.path.read_text())
            report["steps"][0]["status"] = "failed"
            case.path.write_text(json.dumps(report))
        elif change == "report-metadata":
            report = json.loads(case.path.read_text())
            report["duration_seconds"] += 1
            case.path.write_text(json.dumps(report))
        elif change == "log":
            (case.root / case.report["steps"][0]["log"]).write_text("changed passed-stage evidence\n")
        elif change == "missing-owner":
            case.owner.unlink()
        elif change in {"changed-owner", "foreign-owner"}:
            owner = json.loads(case.owner.read_text())
            if change == "foreign-owner":
                owner["workspace_owner"]["git_common_dir"] = str(self.base / "foreign.git")
            else:
                owner["recorded_at"] = "changed receipt"
            case.owner.write_text(json.dumps(owner))
        elif change == "recreated-target":
            case.target.rename(case.target.with_name("retained-original-target"))
            case.target.mkdir()
            (case.target / "preserve").write_bytes(b"foreign new target")
        calls_before = self.call_log.read_text()
        try:
            result, resumed = self.checkpoint_continue(case, **kwargs)
        finally:
            (self.bin / "rustc").write_bytes(rustc_bytes)
        self.assertNotEqual(result, 0)
        self.assertIn("resume_error", resumed.summary)
        calls = self.call_log.read_text()[len(calls_before):]
        self.assertNotIn("cargo test checkpoint-", calls)
        self.assertNotIn("cargo clean", calls)
        self.assertTrue(all(row["status"] == "not_run" for row in resumed.summary["steps"]))
        if change == "recreated-target":
            self.assertEqual((case.target / "preserve").read_bytes(), b"foreign new target")

    @bounded_checkpoint_case
    def test_checkpoint_source_drift_is_refused(self):
        self.assert_checkpoint_drift_refused("source")

    @bounded_checkpoint_case
    def test_checkpoint_plan_drift_is_refused(self):
        self.assert_checkpoint_drift_refused("plan")

    @bounded_checkpoint_case
    def test_checkpoint_plan_order_drift_is_refused(self):
        self.assert_checkpoint_drift_refused("plan-order")

    @bounded_checkpoint_case
    def test_checkpoint_argv_drift_is_refused(self):
        self.assert_checkpoint_drift_refused("argv")

    @bounded_checkpoint_case
    def test_checkpoint_test_configuration_drift_is_refused(self):
        self.assert_checkpoint_drift_refused("test-configuration")

    @bounded_checkpoint_case
    def test_checkpoint_toolchain_drift_is_refused(self):
        self.assert_checkpoint_drift_refused("toolchain")

    @bounded_checkpoint_case
    def test_checkpoint_environment_drift_is_refused(self):
        self.assert_checkpoint_drift_refused("environment")

    @bounded_checkpoint_case
    def test_checkpoint_dependency_drift_is_refused(self):
        self.assert_checkpoint_drift_refused("dependency")

    @bounded_checkpoint_case
    def test_checkpoint_floor_drift_is_refused(self):
        self.assert_checkpoint_drift_refused("floor")

    @bounded_checkpoint_case
    def test_checkpoint_report_drift_is_refused(self):
        self.assert_checkpoint_drift_refused("report")

    @bounded_checkpoint_case
    def test_checkpoint_report_metadata_drift_is_refused(self):
        self.assert_checkpoint_drift_refused("report-metadata")

    @bounded_checkpoint_case
    def test_checkpoint_log_drift_is_refused(self):
        self.assert_checkpoint_drift_refused("log")

    @bounded_checkpoint_case
    def test_checkpoint_missing_owner_drift_is_refused(self):
        self.assert_checkpoint_drift_refused("missing-owner")

    @bounded_checkpoint_case
    def test_checkpoint_changed_owner_drift_is_refused(self):
        self.assert_checkpoint_drift_refused("changed-owner")

    @bounded_checkpoint_case
    def test_checkpoint_foreign_owner_drift_is_refused(self):
        self.assert_checkpoint_drift_refused("foreign-owner")

    @bounded_checkpoint_case
    def test_checkpoint_recreated_target_drift_is_refused(self):
        self.assert_checkpoint_drift_refused("recreated-target")

    def assert_malformed_resume_refused(self, malformed):
        case = self.checkpoint_fixture("checkpoint-malformed-" + malformed)
        report = json.loads(case.path.read_text())
        if malformed in {"tree-list", "tree-null"}:
            report["start_tree"] = report["end_tree"] = [] if malformed == "tree-list" else None
        elif malformed == "target-list":
            report["checkpoint"]["target"] = []
        elif malformed == "counts-null":
            report["steps"][0]["test_results"] = None
        elif malformed == "checkpoint-scalar":
            report["checkpoint"] = 42
        else:
            report = 42
        case.path.write_text(json.dumps(report))
        malformed_bytes = case.path.read_bytes()
        owner_bytes = case.owner.read_bytes()
        calls_before = self.call_log.read_text()
        console = io.StringIO()
        with redirect_stdout(console), redirect_stderr(console):
            result, resumed = self.checkpoint_continue(case)
        self.assertNotEqual(result, 0)
        self.assertTrue(resumed.summary["resume_error"])
        self.assertTrue(all(row["status"] == "not_run" for row in resumed.summary["steps"]))
        self.assertNotIn("Traceback", console.getvalue())
        calls = self.call_log.read_text()[len(calls_before):]
        self.assertNotIn("cargo test checkpoint-", calls)
        self.assertNotIn("cargo clean", calls)
        self.assertEqual(case.owner.read_bytes(), owner_bytes)
        self.assertEqual(case.path.read_bytes(), malformed_bytes)

    @bounded_checkpoint_case
    def test_checkpoint_tree_list_resume_is_refused(self):
        self.assert_malformed_resume_refused("tree-list")

    @bounded_checkpoint_case
    def test_checkpoint_tree_null_resume_is_refused(self):
        self.assert_malformed_resume_refused("tree-null")

    @bounded_checkpoint_case
    def test_checkpoint_target_list_resume_is_refused(self):
        self.assert_malformed_resume_refused("target-list")

    @bounded_checkpoint_case
    def test_checkpoint_counts_null_resume_is_refused(self):
        self.assert_malformed_resume_refused("counts-null")

    @bounded_checkpoint_case
    def test_checkpoint_scalar_resume_is_refused(self):
        self.assert_malformed_resume_refused("checkpoint-scalar")

    @bounded_checkpoint_case
    def test_checkpoint_report_scalar_resume_is_refused(self):
        self.assert_malformed_resume_refused("report-scalar")

    def test_checkpoint_continuation_rejects_failed_interrupted_zero_match_and_nonresource_runs(self):
        for stop in ("failed", "interrupted", "zero", None):
            with self.subTest(stop=stop):
                case = self.checkpoint_fixture("checkpoint-ineligible-" + str(stop), stop=stop)
                self.assertNotIn("checkpoint", case.report)
                original_bytes = case.path.read_bytes()
                calls_before = self.call_log.read_text()
                result, resumed = self.checkpoint_continue(case)
                self.assertNotEqual(result, 0)
                self.assertIn("resume_error", resumed.summary)
                self.assertNotIn("cargo test checkpoint-", self.call_log.read_text()[len(calls_before):])
                self.assertEqual(case.path.read_bytes(), original_bytes)

    def test_checkpoint_consumer_rejects_lost_lineage_changed_original_logs_and_reports(self):
        case = self.checkpoint_fixture("checkpoint-consumer")
        with patch.object(local_ci.ci_checkpoint, "require_no_jobs"):
            result, resumed = self.checkpoint_continue(case)
        self.assertEqual(result, 0)
        report = json.loads(resumed.summary_path.read_text())
        original = case.path.read_bytes()
        log = case.root / case.report["steps"][0]["log"]
        log_bytes = log.read_bytes()
        for change in ("lineage", "original-report", "original-log", "inherited-order"):
            with self.subTest(change=change):
                tampered = json.loads(json.dumps(report))
                if change == "lineage":
                    tampered.pop("resume")
                elif change == "original-report":
                    case.path.write_bytes(original + b"\n")
                elif change == "original-log":
                    log.write_bytes(log_bytes + b"changed\n")
                else:
                    tampered["steps"].reverse()
                try:
                    with self.assertRaises((ValueError, OSError, KeyError, TypeError)):
                        local_ci.ci_checkpoint.validate_inheritance(case.root, tampered)
                finally:
                    case.path.write_bytes(original)
                    log.write_bytes(log_bytes)

    def test_checkpoint_process_inventory_requires_idle_known_cargo_writers(self):
        checkpoint = local_ci.ci_checkpoint
        with patch.object(checkpoint.sys, "platform", "darwin"):
            for binary in ("cargo", "rustc", "rustdoc", "rustup"):
                with self.subTest(binary=binary), patch.object(checkpoint.subprocess, "run",
                        return_value=SimpleNamespace(returncode=0, stdout="1 /sbin/launchd\n42 /tools/" + binary + "\n")):
                    with self.assertRaisesRegex(ValueError, "active"):
                        checkpoint.require_no_jobs()
            for output, status in (("", 0), ("unparseable process", 0), ("1 /sbin/launchd", 1)):
                with self.subTest(output=output, status=status), patch.object(checkpoint.subprocess, "run",
                        return_value=SimpleNamespace(returncode=status, stdout=output)):
                    with self.assertRaises(ValueError):
                        checkpoint.require_no_jobs()
            with patch.object(checkpoint.subprocess, "run", side_effect=OSError("process inventory unavailable")):
                with self.assertRaisesRegex(ValueError, "cannot establish"):
                    checkpoint.require_no_jobs()
            with patch.object(checkpoint.subprocess, "run",
                    return_value=SimpleNamespace(returncode=0, stdout="1 /sbin/launchd\n42 /usr/bin/python3\n")):
                checkpoint.require_no_jobs()
        case = self.checkpoint_fixture("checkpoint-active-job")
        calls_before = self.call_log.read_text()
        with patch.object(checkpoint, "require_no_jobs", side_effect=ValueError("Cargo/compiler job is active")):
            result, resumed = self.checkpoint_continue(case)
        self.assertNotEqual(result, 0)
        self.assertIn("active", resumed.summary["resume_error"])
        calls = self.call_log.read_text()[len(calls_before):]
        self.assertNotIn("cargo test checkpoint-", calls)
        self.assertNotIn("cargo clean", calls)

    def test_checkpoint_owner_parent_alias_supports_real_stop_and_resume(self):
        physical_parent = self.base / "physical-runner-state"
        physical_parent.mkdir()
        alias_parent = self.base / "runner-state-alias"
        alias_parent.symlink_to(physical_parent, target_is_directory=True)
        self.target_state_dir = alias_parent / "target-owners"
        case = self.checkpoint_fixture("checkpoint-owner-parent-alias")
        self.assertTrue(any(parent.is_symlink() for parent in case.owner.parents))
        owner_bytes = case.owner.read_bytes()
        self.assertEqual(local_ci.ci_checkpoint.owner_digest(case.owner), hashlib.sha256(owner_bytes).hexdigest())
        with self.assertRaisesRegex(ValueError, "symlink"):
            local_ci.ci_checkpoint.file_digest(case.owner)
        original = case.path.read_bytes()
        log = case.root / case.report["steps"][0]["log"]
        log_bytes = log.read_bytes()
        calls_before = self.call_log.read_text()
        with patch.object(local_ci.ci_checkpoint, "require_no_jobs"):
            result, resumed = self.checkpoint_continue(case)
        self.assertEqual(result, 0)
        self.assertNotIn("cargo test checkpoint-prefix", self.call_log.read_text()[len(calls_before):])
        self.assertIn("cargo test checkpoint-suffix", self.call_log.read_text()[len(calls_before):])
        self.assertEqual(case.path.read_bytes(), original)
        self.assertEqual(log.read_bytes(), log_bytes)
        self.assertFalse(case.owner.is_symlink())
        local_ci.ci_checkpoint.validate_inheritance(case.root, json.loads(resumed.summary_path.read_text()))

    def test_checkpoint_owner_final_marker_symlink_remains_a_resume_refusal(self):
        case = self.checkpoint_fixture("checkpoint-owner-marker-symlink")
        original = case.path.read_bytes()
        owner_bytes = case.owner.read_bytes()
        retained_owner = self.base / "retained-owner.json"
        case.owner.rename(retained_owner)
        case.owner.symlink_to(retained_owner)
        with self.assertRaisesRegex(ValueError, "symlink"):
            local_ci.ci_checkpoint.owner_digest(case.owner)
        calls_before = self.call_log.read_text()
        result, resumed = self.checkpoint_continue(case)
        self.assertNotEqual(result, 0)
        self.assertIn("resume_error", resumed.summary)
        self.assertTrue(all(row["status"] == "not_run" for row in resumed.summary["steps"]))
        calls = self.call_log.read_text()[len(calls_before):]
        self.assertNotIn("cargo test checkpoint-", calls)
        self.assertNotIn("cargo clean", calls)
        self.assertTrue(case.owner.is_symlink())
        self.assertEqual(retained_owner.read_bytes(), owner_bytes)
        self.assertEqual(case.path.read_bytes(), original)

    def test_checkpoint_execution_inputs_refuse_mutable_native_fixtures_and_resolve_relative_tools(self):
        steps = (local_ci.Step("first", ("first",)),)
        checkpoint = local_ci.ci_checkpoint
        for key in ("HEX_VST3_TEST_PLUGIN", "HEX_VST3_FUTURE_FIXTURE", "HEX_TEST_SIGNING_IDENTITY"):
            with self.subTest(key=key):
                with self.assertRaisesRegex(ValueError, "external native fixtures/signing"):
                    checkpoint.execution_inputs(self.repo, {**self.env, key: "external-fixture"}, steps)
                self.assertIn("environment_sha256", checkpoint.execution_inputs(self.repo, {**self.env, key: ""}, steps))
        tool = self.repo / "relative-tool"
        tool.write_text("#!/bin/sh\nexit 0\n")
        tool.chmod(0o755)
        inputs = checkpoint.execution_inputs(self.repo, self.env, (local_ci.Step("relative", ("./relative-tool",)),))
        identified = inputs["tools"]["./relative-tool"]
        self.assertEqual(identified["path"], str(tool))
        self.assertEqual(identified["sha256"], hashlib.sha256(tool.read_bytes()).hexdigest())

    def test_checkpoint_unavailable_native_inputs_do_not_block_fresh_full_checks(self):
        case = self.checkpoint_fixture("checkpoint-native-fresh", stop=None,
            env_overrides={"HEX_VST3_TEST_PLUGIN": "external-fixture", "HEX_TEST_SIGNING_IDENTITY": "keychain-fixture"})
        self.assertEqual(case.report["status"], "passed")
        self.assertEqual([row["execution"] for row in case.report["steps"]], ["executed", "executed"])
        self.assertNotIn("checkpoint", case.report)
        self.assertIn("external native fixtures/signing", case.report["checkpoint_unavailable"])

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
            min_free_bytes=0,
        )
        self.assertEqual(runner.run(), 1)
        summary = self.summary("failure")
        self.assertEqual(summary["status"], "failed")
        self.assertEqual([step["status"] for step in summary["steps"]], ["failed", "not_run"])
        self.assertEqual(summary["steps"][0]["exit_code"], 7)
        self.assertGreater(summary["steps"][0]["duration_seconds"], 0)
        self.assertIsNone(summary["steps"][1]["duration_seconds"])
        self.assertIn("intentional failure", (self.repo / summary["steps"][0]["log"]).read_text())

    def test_report_enospc_preserves_failed_command_and_cleanup(self):
        self._report_enospc_cases("command", failed_command=True)

    def test_report_enospc_before_command_admission_releases_target(self):
        self._report_enospc_cases("admission", failed_command=False)

    def test_transient_report_failure_stops_commands_and_records_incomplete_evidence(self):
        runner = self.runner("transient-report", steps=(local_ci.Step("first", ("first",)), local_ci.Step("later", ("second",))))
        original = Path.replace
        faulted = False
        def replace(path, target):
            nonlocal faulted
            if Path(target) == runner.summary_path and not faulted and any(e["status"] == "passed" for e in runner.summary["steps"]):
                faulted = True
                raise OSError(errno.ENOSPC, "one-off report publication refusal")
            return original(path, target)
        output = io.StringIO()
        with patch.object(Path, "replace", replace), redirect_stderr(output):
            self.assertEqual(runner.run(), 2)
        saved = self.summary("transient-report")
        self.assertEqual((saved["status"], saved["verification_status"]), ("incomplete", "failed"))
        self.assertEqual([e["status"] for e in saved["steps"]], ["passed", "not_run"])
        self.assertIn("one-off report publication refusal", saved["report_publication_error"])
        self.assertIn("report publication failed", output.getvalue())
        self.assertNotIn("second", self.call_log.read_text())

    def test_initial_report_enospc_runs_no_commands(self):
        self._report_enospc_cases("initial", failed_command=False)

    def test_final_report_enospc_cannot_qualify_successful_commands(self):
        self._report_enospc_cases("final", failed_command=False)

    def _report_enospc_cases(self, stage, *, failed_command):
        for operation in ("write", "replace"):
            with self.subTest(stage=stage, operation=operation):
                name = f"report-{stage}-{operation}"
                runner = self.runner(name, env={**self.env, "FAIL_MATCH": "cargo build" if failed_command else "never-match"},
                    steps=(local_ci.Step("build", ("cargo", "build")), local_ci.Step("later", ("second",))))
                original_write, original_replace = Path.write_text, Path.replace
                faulted = False
                durable = None
                def should_fail():
                    nonlocal faulted
                    entries = runner.summary["steps"]
                    triggered = (stage == "initial" or
                        stage == "admission" and any(e["status"] == "running" for e in entries) or
                        stage == "command" and any(e["status"] == "failed" for e in entries) or
                        stage == "final" and runner.summary.get("ended_at") is not None)
                    faulted = faulted or triggered
                    return faulted
                def write(path, *args, **kwargs):
                    if path == runner.summary_path.with_suffix(".json.tmp") and operation == "write" and should_fail():
                        raise OSError(errno.ENOSPC, "injected report storage exhaustion")
                    return original_write(path, *args, **kwargs)
                def replace(path, target):
                    nonlocal durable
                    if Path(target) == runner.summary_path:
                        if operation == "replace" and should_fail():
                            raise OSError(errno.ENOSPC, "injected report storage exhaustion")
                        result = original_replace(path, target)
                        durable = runner.summary_path.read_bytes()
                        return result
                    return original_replace(path, target)
                previous_handlers = {sig: signal.getsignal(sig) for sig in (signal.SIGINT, signal.SIGTERM)}
                calls_before = self.call_log.read_bytes() if self.call_log.exists() else None
                output = io.StringIO()
                with patch.object(Path, "write_text", write), patch.object(Path, "replace", replace), redirect_stderr(output):
                    result = runner.run()
                self.assertTrue(faulted)
                self.assertEqual(result, 1 if failed_command else 2)
                self.assertEqual({sig: signal.getsignal(sig) for sig in previous_handlers}, previous_handlers)
                self.assertIsNone(runner.target_lock)
                self.assertNotIn(runner.summary["status"], {"passed", "partial"})
                self.assertEqual(runner.summary["verification_status"], "failed")
                self.assertIn("report publication failed", output.getvalue())
                self.assertIn(str(runner.summary_path), output.getvalue())
                self.assertNotIn("Traceback", output.getvalue())
                if durable is None:
                    self.assertFalse(runner.summary_path.exists())
                else:
                    self.assertEqual(runner.summary_path.read_bytes(), durable)
                    self.assertNotIn(json.loads(durable)["status"], {"passed", "partial"})
                if failed_command:
                    step = runner.summary["steps"][0]
                    self.assertEqual((step["status"], step["exit_code"]), ("failed", 7))
                    log = runner.root / step["log"]
                    self.assertIn("intentional failure", log.read_text())
                    self.assertIn("command: cargo build", output.getvalue())
                    self.assertIn("exit code: 7", output.getvalue())
                    self.assertIn(str(log), output.getvalue())
                    self.assertEqual(runner.summary["steps"][1]["status"], "not_run")
                elif stage == "initial":
                    self.assertEqual(self.call_log.read_bytes() if self.call_log.exists() else None, calls_before)
                elif stage == "admission":
                    self.assertTrue(all(e["status"] == "not_run" for e in runner.summary["steps"]))
                    self.assertNotIn(b"cargo build", self.call_log.read_bytes()[len(calls_before or b""):])
                else:
                    self.assertTrue(all(e["status"] == "passed" for e in runner.summary["steps"]))
                # The report failure cannot strand the shared target lease.
                self.assertEqual(self.runner("after-" + name).run(), 0)

    def test_missing_executable_is_logged_as_failure(self) -> None:
        runner = local_ci.LocalCiRunner(
            self.repo,
            run_id="missing",
            env=self.env,
            steps=(local_ci.Step("missing", ("not-installed",)),),
            min_free_bytes=0,
        )
        self.assertEqual(runner.run(), 1)
        summary = self.summary("missing")
        self.assertEqual(summary["steps"][0]["status"], "failed")
        self.assertIsNone(summary["steps"][0]["exit_code"])
        self.assertGreater(summary["steps"][0]["duration_seconds"], 0)
        self.assertIn("not-installed", (self.repo / summary["steps"][0]["log"]).read_text())
        self.assertIn("required executable not found", summary["steps"][0]["error"])

    def test_failure_details_show_bounded_compiler_and_assertion_context(self):
        for kind, detail, expected in (
            ("compiler", "error[E0425]: cannot find value `missing` in this scope\n --> src/problem.rs:3:5\n  |\n3 |     missing;\n  |     ^^^^^^^ not found", "compiler failure"),
            ("assertion", "thread 'tests::literal_pcm' panicked at src/problem.rs:3:5:\nassertion `left == right` failed\n  left: 0.1875\n right: 0.125", "test failure"),
        ):
            with self.subTest(kind=kind):
                content = ("irrelevant setup\n" * 100 + "\x1b[31m" + detail + "\x1b[0m\nAPI_TOKEN=must-not-display\n"
                           + "api_key=unconfigured-sensitive-value\n"
                           + "x" * 10000 + "\n" + "noise\n" * 100)
                runner = self.runner(kind, env={**self.env, "PRIVATE_TOKEN": "must-not-display"},
                    steps=(local_ci.Step(kind, (sys.executable, "-c", "import sys; print(" + repr(content) + "); sys.exit(7)")),))
                console = io.StringIO()
                with redirect_stdout(console):
                    self.assertEqual(runner.run(), 1)
                output = console.getvalue()
                self.assertIn(expected, output)
                self.assertIn("src/problem.rs:3:5", output)
                self.assertIn(str(runner.output_dir / f"01-{kind}.log"), output)
                self.assertIn("profile: full", output)
                self.assertIn("command:", output)
                self.assertIn("diagnostic only; partial", output)
                self.assertIn("exact argv retained in summary", output)
                self.assertNotIn("must-not-display", output)
                self.assertNotIn("unconfigured-sensitive-value", output)
                self.assertNotIn("\x1b", output)
                self.assertLess(len(output), 12000)
                raw = (self.repo / self.summary(kind)["steps"][0]["log"]).read_text()
                self.assertIn(detail, raw)
                self.assertIn("must-not-display", raw)

    def test_console_diagnostic_failure_does_not_replace_recorded_result(self):
        runner = self.runner("broken-console", env={**self.env, "FAIL_MATCH": "bad"},
                             steps=(local_ci.Step("bad", ("bad",)),))
        with patch.object(runner, "_print_failure_details", side_effect=BrokenPipeError("closed console")):
            self.assertEqual(runner.run(), 1)
        self.assertEqual(self.summary("broken-console")["status"], "failed")

    def test_failure_details_cover_zero_match_missing_tool_interrupt_and_stale(self):
        cases = (
            ("empty-tests", (sys.executable, "-c", "print('test result: ok. 0 passed;')"), {}, True, 1, "no passing tests"),
            ("missing-tool", ("not-installed",), {}, False, 1, "missing executable"),
            ("interrupted-detail", ("first",), {"INTERRUPT_ON": "1"}, False, 130, "interrupted"),
            ("stale-detail", ("first",), {"MUTATE_FILE": str(self.repo / "source.rs"), "FAKE_FILE_LIST": "source.rs"}, False, 3, "source changed"),
        )
        for name, argv, env, tests, code, category in cases:
            with self.subTest(name=name):
                (self.repo / "source.rs").write_text("original\n")
                runner = self.runner(name, env={**self.env, **env}, steps=(local_ci.Step(name, argv, requires_tests=tests),))
                console = io.StringIO()
                with redirect_stdout(console):
                    self.assertEqual(runner.run(), code)
                output = console.getvalue()
                self.assertIn(category, output)
                self.assertIn(str(runner.summary_path), output)
                self.assertIn(str(runner.output_dir / f"01-{name}.log"), output)
                self.assertIn("diagnostic only; partial", output)

    def test_log_io_failure_retains_failure_summary_and_diagnostic_fallback(self):
        for operation in ("open", "read"):
            with self.subTest(operation=operation):
                runner = self.runner("log-" + operation, steps=(local_ci.Step("first", ("first",)),))
                log = runner.output_dir / "01-first.log"
                original_open = Path.open
                original_read = Path.read_text
                def open_path(path, *args, **kwargs):
                    if operation == "open" and path == log:
                        raise PermissionError("fixture log unwritable")
                    return original_open(path, *args, **kwargs)
                def read_path(path, *args, **kwargs):
                    if operation == "read" and path == log:
                        raise PermissionError("fixture log unreadable")
                    return original_read(path, *args, **kwargs)
                console = io.StringIO()
                with patch.object(Path, "open", open_path), patch.object(Path, "read_text", read_path), redirect_stdout(console):
                    self.assertEqual(runner.run(), 1)
                output = console.getvalue()
                self.assertIn("log I/O failure", output)
                self.assertIn(str(log), output)
                self.assertEqual(self.summary("log-" + operation)["steps"][0]["status"], "failed")
                if operation == "read":
                    self.assertIn("completed first", log.read_text())

    def test_invalid_step_ids_refuse_entire_plan_and_restore_resources(self) -> None:
        handlers = {sig: local_ci.signal.getsignal(sig)
                    for sig in (local_ci.signal.SIGINT, local_ci.signal.SIGTERM)}
        for index, step_id in enumerate(("transport/routes", "../escape", "..", ".", "/absolute",
                                         "transport\\routes", "", "a" * 101, "a\n", "é")):
            with self.subTest(step_id=step_id):
                run_id = f"invalid-{index}"
                runner = self.runner(run_id, steps=(local_ci.Step("first", ("first",)),
                                                    local_ci.Step(step_id, ("cargo", "build"))))
                lock = Mock()
                lock.release.return_value = None
                runner.target_lock = lock
                with patch.object(local_ci.subprocess, "run") as command:
                    self.assertEqual(runner.run(), 2)
                command.assert_not_called()
                lock.release.assert_called_once_with()
                self.assertIsNone(runner.target_lock)
                for sig, handler in handlers.items():
                    self.assertIs(local_ci.signal.getsignal(sig), handler)
                summary = self.summary(run_id)
                self.assertEqual(summary["status"], "failed")
                self.assertEqual(summary["verification_status"], "failed")
                self.assertIsNone(summary["time_to_first_result_seconds"])
                self.assertTrue(all(s["duration_seconds"] is None for s in summary["steps"]))
                self.assertIn("invalid step IDs", summary["step_id_error"])
                self.assertNotIn("required executable", json.dumps(summary))
                self.assertEqual([s["step_id"] for s in summary["steps"]], ["first", step_id])
                self.assertTrue(all(s["status"] == "not_run" and s["log"] is None
                                    and s["exit_code"] is None for s in summary["steps"]))
                self.assertEqual(list(runner.output_dir.iterdir()), [runner.output_dir / "summary.json"])
                self.assertFalse(self.call_log.exists())

    def test_safe_step_ids_keep_distinct_numbered_log_linkage(self) -> None:
        ids = ("transport-routes", "transport_routes", "A0", "a" * 100, "transport-routes")
        runner = self.runner("safe-ids", steps=tuple(local_ci.Step(s, ("first", s)) for s in ids))
        self.assertEqual(runner.run(), 0)
        summary = self.summary("safe-ids")
        self.assertEqual(summary["status"], "passed")
        logs = []
        for index, (step_id, entry) in enumerate(zip(ids, summary["steps"]), start=1):
            log = self.repo / entry["log"]
            self.assertEqual(log, runner.output_dir / f"{index:02d}-{step_id}.log")
            self.assertEqual(log.resolve().parent, runner.output_dir.resolve())
            self.assertEqual(entry["step_id"], step_id)
            self.assertEqual(entry["status"], "passed")
            self.assertIn(f"completed first {step_id}", log.read_text())
            logs.append(log)
        self.assertEqual(len(set(logs)), len(ids))

    def test_source_change_during_run_marks_evidence_stale(self) -> None:
        source = self.repo / "source.rs"
        source.write_text("before\n")
        runner = local_ci.LocalCiRunner(
            self.repo,
            run_id="stale",
            env={**self.env, "MUTATE_FILE": str(source), "FAKE_FILE_LIST": "source.rs"},
            steps=(local_ci.Step("mutate", ("first",)),),
            min_free_bytes=0,
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
            min_free_bytes=0,
        )
        self.assertEqual(runner.run(), 130)
        summary = self.summary("interrupted")
        self.assertEqual(summary["status"], "interrupted")
        self.assertEqual(summary["exit_code"], 130)
        self.assertEqual(summary["steps"][0]["status"], "interrupted")
        self.assertEqual(summary["steps"][0]["exit_code"], 130)
        self.assertIsNotNone(summary["steps"][0]["ended_at"])
        self.assertGreater(summary["steps"][0]["duration_seconds"], 0)
        self.assertIsNotNone(summary["time_to_first_result_seconds"])
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

    def test_dev_owner_record_matches_writer_and_reader(self) -> None:
        target = Path(self.env["FAKE_METADATA_TARGET"])
        target.mkdir()
        runner = self.runner("owner-record")
        runner.cargo_identity = {"target_directory": str(target)}
        runner._dependency_inputs = {"version": 1, "sha256": "a" * 64, "packages": []}
        workspace = {"workspace_root": str(self.repo), "git_common_dir": str(self.repo / ".git")}
        marker = self.base / "owner.json"
        with patch.object(local_ci, "utc_now", return_value="2026-01-01T00:00:00Z"), \
                patch.object(runner, "_cache_provenance", return_value={"witness": "fixture"}) as provenance:
            record = runner._target_owner_record(workspace, checked=True)
            runner._write_target_owner(marker, workspace, checked=True)
        self.assertEqual(runner._load_target_owner(marker, target), record)
        self.assertEqual(record["workspace_owner"], workspace)
        self.assertEqual(record["dependency_inputs"], runner._dependency_inputs)
        self.assertEqual(record["target_identity"], runner._directory_identity(target))
        self.assertEqual(record["cache_inputs"], {"witness": "fixture"})
        self.assertEqual(provenance.call_args_list, [unittest.mock.call(checked=True)] * 2)

    def _reserved_dev_runner(self, name, *, real_cargo=False):
        target = self.repo / name
        target.mkdir()
        env = dict(self.env, CARGO_TARGET_DIR=str(target), FAKE_METADATA_TARGET=str(target))
        if real_cargo:
            # Use real offline Cargo metadata and provenance, with the same
            # deterministic Git witness as the other runner fixtures.
            git_bin = self.base / "git-only"
            git_bin.mkdir()
            (git_bin / "git").symlink_to(self.bin / "git")
            env = dict(os.environ, **env)
            env.update(PATH=str(git_bin) + os.pathsep + os.environ["PATH"],
                       CARGO_NET_OFFLINE="true", CARGO_CACHE_RUSTC_INFO="0",
                       FAKE_FILE_LIST="Cargo.toml")
            (self.repo / "Cargo.toml").write_text('[package]\nname = "fixture"\nversion = "0.1.0"\nedition = "2021"\n')
            (self.repo / "Cargo.lock").write_text('version = 4\n\n[[package]]\nname = "fixture"\nversion = "0.1.0"\n')
            (self.repo / "src").mkdir()
            (self.repo / "src/lib.rs").write_text("")
        runner = self.runner(name, env=env)
        self.assertTrue(runner._capture_repository_state("start_tree"), runner.summary.get("repository_error"))
        reservation = local_ci.FreshTargetReservation.acquire(
            runner.root, str(target), {"run_id": name}, state_dir=self.target_state_dir)
        self.addCleanup(reservation.close)
        self.assertTrue(runner._resolve_cargo_identity(reservation), runner.summary.get("cargo_metadata_error"))
        if real_cargo:
            runner._dependency_inputs = runner._dependency_context()
            runner._workspace_inputs = local_ci.build_inputs.workspace_snapshot(
                runner.root, runner.env, runner._release_metadata)
        else:
            runner._dependency_inputs = {"version": 1, "sha256": "a" * 64, "packages": []}
        return runner, reservation, target

    def test_fresh_dev_publication_transfers_same_lock_and_real_owner_schema(self):
        runner, reservation, target = self._reserved_dev_runner("fresh-owner-real", real_cargo=True)
        owner = runner.target_owner_path
        lock = reservation._lock
        locked_file = lock.file
        with reservation:
            runner._claim_fresh_dev_owner(reservation)
            self.addCleanup(lock.release)
        self.assertIs(runner.target_lock, lock)
        self.assertIs(lock.file, locked_file)
        self.assertTrue(runner.target_owner_claimed)
        record = runner._load_target_owner(owner, target)
        self.assertEqual(record["workspace_owner"], {key: runner.git_identity[key]
                                                    for key in ("workspace_root", "git_common_dir")})
        self.assertEqual(record["target_identity"], runner._directory_identity(target))
        self.assertEqual(record["dependency_inputs"], runner._dependency_inputs)
        self.assertFalse(record["cache_inputs"]["workspace"]["checked"])
        self.assertEqual(list(target.iterdir()), [])
        self.assertFalse(local_ci.target_admission_pending_path(lock.path).exists())
        with self.assertRaises(local_ci.TargetCacheError):
            runner._claim_fresh_dev_owner(reservation)
        contender = local_ci.CargoTargetLock(lock.path, {"run_id": "contender"})
        with self.assertRaisesRegex(local_ci.TargetCacheError, "already in use"):
            contender.acquire()
        self.assertIs(lock.file, locked_file)
        self.assertEqual(runner._load_target_owner(owner, target), record)

    def test_fresh_dev_refuses_absent_before_metadata_or_owner_publication(self):
        target = self.repo / "fresh-absent-refused"
        env = dict(self.env, CARGO_TARGET_DIR=str(target), FAKE_METADATA_TARGET=str(target))
        runner = self.runner("fresh-absent-refused", env=env)
        owner = local_ci.target_state_paths(target, self.target_state_dir)[1]
        with patch.object(runner, "_resolve_cargo_identity") as metadata, \
                self.assertRaisesRegex(local_ci.TargetCacheError, "create an empty directory"):
            local_ci.FreshTargetReservation.acquire(
                runner.root, str(target), {"run_id": "absent"}, state_dir=self.target_state_dir)
        metadata.assert_not_called()
        self.assertFalse(target.exists())
        self.assertFalse(owner.exists())
        self.assertFalse(list(target.parent.glob(".hex-cargo-target-*.tmp")))

    def _assert_pending_consumers_refuse(self, name, target):
        env = dict(self.env, CARGO_TARGET_DIR=str(target), FAKE_METADATA_TARGET=str(target))
        calls = self.call_log.read_text()
        for adopt in (False, True):
            consumer = self.runner(f"{name}-dev-{adopt}", env=env, adopt_target=adopt)
            self.assertEqual(consumer.run(), 1)
            self.assertIn("admission is incomplete", consumer.summary["target_cache_error"])
            self.assertIsNone(consumer.target_lock)
            release = self.runner(f"{name}-release-{adopt}", env=env)
            with self.assertRaisesRegex(local_ci.TargetCacheError, "admission is incomplete"):
                with release.guarded_release(adopt=adopt):
                    self.fail("release accepted a pending target")
            self.assertIsNone(release.target_lock)
        new_calls = self.call_log.read_text()[len(calls):]
        self.assertNotIn("cargo clean", new_calls)
        self.assertNotIn("cargo build", new_calls)

    def test_fresh_dev_publication_keeps_durable_refusal_after_precommit_failure(self):
        for event in ("same-profile", "opposite-profile", "target-contents", "target-replaced",
                      "lock", "state", "owner-bytes", "owner-inode", "temporary",
                      "link-error", "link-interrupt", "pending-file-sync", "pending-directory-sync",
                      "owner-file-sync", "owner-directory-sync"):
            with self.subTest(event=event):
                runner, reservation, target = self._reserved_dev_runner("fresh-" + event)
                owner = runner.target_owner_path
                opposite = owner.with_name(owner.name.replace(".owner.json", ".release-owner.json"))
                lock_path = reservation._lock.path
                pending = local_ci.target_admission_pending_path(lock_path)
                observed_pending = pending
                link, fsync = os.link, os.fsync
                expected = {}
                synced = []

                def change(source, destination, **kwargs):
                    nonlocal observed_pending
                    identity = pending.stat()
                    self.assertIn((identity.st_dev, identity.st_ino), synced)
                    self.assertIn("directory", synced)
                    if event == "same-profile":
                        owner.write_bytes(b"other dev owner")
                        expected[owner] = owner.read_bytes()
                    link(source, destination, **kwargs)
                    expected[owner] = owner.read_bytes()
                    if event == "opposite-profile":
                        opposite.write_bytes(b"release owner")
                        expected[opposite] = opposite.read_bytes()
                    elif event in {"target-contents", "target-replaced"}:
                        if event == "target-replaced":
                            target.rename(target.with_suffix(".old"))
                            target.mkdir()
                        (target / "keep").write_bytes(b"unowned target")
                        expected[target / "keep"] = b"unowned target"
                    elif event == "lock":
                        lock_path.rename(lock_path.with_suffix(".old"))
                        lock_path.write_bytes(b"replacement lock")
                        expected[lock_path] = lock_path.read_bytes()
                    elif event == "state":
                        old_state = self.target_state_dir.with_suffix(".old")
                        self.target_state_dir.rename(old_state)
                        self.target_state_dir.mkdir()
                        expected[old_state / owner.name] = expected.pop(owner)
                        observed_pending = old_state / pending.name
                    elif event == "owner-bytes":
                        owner.write_bytes(b"changed in place")
                        expected[owner] = owner.read_bytes()
                    elif event == "owner-inode":
                        owner.rename(owner.with_suffix(".old"))
                        owner.write_bytes(expected[owner])
                        expected[owner.with_suffix(".old")] = expected[owner]
                    elif event == "temporary":
                        temporary = owner.parent / source
                        temporary.unlink()
                        temporary.write_bytes(b"replacement temporary")
                        expected[temporary] = temporary.read_bytes()
                    elif event == "link-error":
                        raise OSError(errno.EIO, "ambiguous link")
                    elif event == "link-interrupt":
                        raise KeyboardInterrupt()

                def sync(descriptor):
                    if descriptor == reservation._state_fd:
                        key = "directory"
                        stage = "owner-directory-sync" if owner.exists() else "pending-directory-sync"
                    else:
                        info = os.fstat(descriptor)
                        key = (info.st_dev, info.st_ino)
                        identity = pending.stat()
                        stage = ("pending-file-sync" if key == (identity.st_dev, identity.st_ino)
                                 else "owner-file-sync")
                    if event == stage:
                        raise OSError(errno.EIO, "failed " + stage)
                    fsync(descriptor)
                    synced.append(key)

                error = KeyboardInterrupt if event == "link-interrupt" else local_ci.TargetCacheError
                with patch.object(local_ci.os, "link", side_effect=change), \
                        patch.object(local_ci.os, "fsync", side_effect=sync), self.assertRaises(error):
                    runner._claim_fresh_dev_owner(reservation)
                self.assertIsNone(runner.target_lock)
                self.assertFalse(runner.target_owner_claimed)
                self.assertIsNone(reservation._lock.file)
                with self.assertRaisesRegex(local_ci.TargetCacheError, "no longer live"):
                    reservation.revalidate(str(target))
                self.assertEqual(observed_pending.read_bytes(), b"")
                self.assertEqual(owner.exists(), owner in expected)
                for path, preserved in expected.items():
                    self.assertEqual(path.read_bytes(), preserved)
                self.assertEqual(set(self.target_state_dir.glob(f".{owner.name}.*.tmp")),
                                 {path for path in expected if path.suffix == ".tmp"})
                if event in {"opposite-profile", "target-contents", "link-interrupt"}:
                    self._assert_pending_consumers_refuse(event, target)
                    self.assertEqual(observed_pending.read_bytes(), b"")
                    for path, preserved in expected.items():
                        self.assertEqual(path.read_bytes(), preserved)

    def test_fresh_dev_link_interrupt_survives_temp_cleanup_failure(self):
        runner, reservation, _ = self._reserved_dev_runner("fresh-link-double-failure")
        pending = local_ci.target_admission_pending_path(reservation._lock.path)
        replacement = []
        def interrupt(source, destination, **kwargs):
            temporary = self.target_state_dir / source
            temporary.unlink()
            temporary.write_bytes(b"replacement temporary")
            replacement.append(temporary)
            raise KeyboardInterrupt()
        with patch.object(local_ci.os, "link", side_effect=interrupt):
            with self.assertRaises(KeyboardInterrupt) as caught:
                runner._claim_fresh_dev_owner(reservation)
        self.assertIsInstance(caught.exception.__cause__, local_ci.TargetCacheError)
        self.assertEqual(replacement[0].read_bytes(), b"replacement temporary")
        self.assertTrue(pending.exists())
        self.assertIsNone(reservation._lock.file)
        self.assertIsNone(runner.target_lock)

    def test_fresh_dev_handoff_interruption_closes_remaining_pins_and_lock(self):
        runner, reservation, target = self._reserved_dev_runner("fresh-close-interrupt")
        descriptors = set(reservation._handles + [reservation._state_fd])
        attempted = []
        original_finish, original_close = reservation._finish, os.close
        def interrupted_finish(*, transfer_lock):
            if not transfer_lock:
                return original_finish(transfer_lock=False)
            def close_then_interrupt(descriptor):
                original_close(descriptor)
                attempted.append(descriptor)
                if len(attempted) == 1:
                    raise KeyboardInterrupt()
            with patch.object(local_ci.os, "close", side_effect=close_then_interrupt):
                return original_finish(transfer_lock=True)
        with patch.object(reservation, "_finish", side_effect=interrupted_finish):
            with self.assertRaises(KeyboardInterrupt):
                runner._claim_fresh_dev_owner(reservation)
        self.assertEqual(set(attempted), descriptors)
        self.assertIsNone(runner.target_lock)
        self.assertIsNone(reservation._lock.file)
        self.assertFalse(runner.target_owner_claimed)
        self.assertIsNotNone(runner._load_target_owner(runner.target_owner_path, target))
        self.assertFalse(local_ci.target_admission_pending_path(reservation._lock.path).exists())

    def test_fresh_dev_publication_sync_failure_after_commit_does_not_transfer_lock(self):
        runner, reservation, target = self._reserved_dev_runner("fresh-final-sync")
        owner = runner.target_owner_path
        lock = reservation._lock
        pending = local_ci.target_admission_pending_path(lock.path)
        fsync = os.fsync

        def sync(descriptor):
            if descriptor == reservation._state_fd and not pending.exists():
                raise OSError(errno.EIO, "failed final directory sync")
            fsync(descriptor)

        with patch.object(local_ci.os, "fsync", side_effect=sync):
            with self.assertRaisesRegex(local_ci.TargetCacheError, "failed final directory sync"):
                runner._claim_fresh_dev_owner(reservation)
        self.assertIsNone(runner.target_lock)
        self.assertFalse(runner.target_owner_claimed)
        self.assertIsNone(lock.file)
        self.assertFalse(pending.exists())
        self.assertEqual(runner._load_target_owner(owner, target)["target_identity"],
                         runner._directory_identity(target))
        # The verified owner was durable before commit. If the deletion is
        # lost on crash, the restored sentinel would instead refuse admission.
        contender = local_ci.CargoTargetLock(lock.path, {"run_id": "later"})
        contender.acquire()
        self.assertIsNone(contender.release())

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
            'fi\n'
            'if [ "$1" = scripts/test_shards.py ]; then echo "test result: ok. 1 passed; 0 failed; 0 ignored;"; fi\n'
            'echo "completed $tool $*"',
        ))
        cargo = self.bin / "cargo"
        cargo.write_text(FAKE_TOOL.replace(
            'echo "completed $tool $*"',
            'if [ "$1" = test ]; then echo "test result: ok. 1 passed; 0 failed; 0 ignored;"; fi\n'
            'echo "completed $tool $*"',
        ))
        runner = self.runner("python-cargo", env={**self.env, "EXPECTED_OWNER": str(owner)},
                             steps=local_ci.FULL_STEPS)
        self.assertEqual(runner.run(), 0)
        summary = self.summary("python-cargo")
        self.assertEqual([(s["step_id"], s["argv"], s["status"]) for s in summary["steps"]],
                         [(s.step_id, list(s.argv), "passed") for s in local_ci.FULL_STEPS])
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
            "--package", "fixture", "--package", "other",
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
        install_delivery_source_fixture(worktree_b)
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
        self.assertIn("--package fixture --package other", calls)

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
        install_delivery_source_fixture(worktree_b)
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
        install_delivery_source_fixture(worktree_b)
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

    def test_exfat_refusal_preserves_owned_target_and_skips_cleanup_and_build(self):
        target = Path(self.env["FAKE_METADATA_TARGET"])
        target.mkdir()
        self.assertEqual(self.runner("storage-owner").run(), 0)
        sentinel = target / "preserve.me"
        sentinel.write_bytes(b"accepted artifacts")
        owner = next(self.target_state_dir.glob("*.owner.json"))
        before = owner.read_bytes()
        calls = self.call_log.read_text()
        runner = self.runner("exfat", adopt_target=True)
        with patch.object(local_ci.ci_target_storage, "inspect", return_value={
                "platform": "darwin", "status": "identified", "filesystem_type": "exfat"}):
            self.assertEqual(runner.run(), 1)
            with self.assertRaisesRegex(local_ci.TargetCacheError, "ExFAT.*unsupported"):
                self.release_attempt("exfat-release", adopt=True)
        self.assertIsNone(runner.target_lock)
        self.assertFalse(runner.target_owner_claimed)
        self.assertEqual(owner.read_bytes(), before)
        self.assertEqual(sentinel.read_bytes(), b"accepted artifacts")
        self.assertFalse(list(self.target_state_dir.glob("*.release-owner.json")))
        new_calls = self.call_log.read_text()[len(calls):]
        self.assertNotIn("cargo clean", new_calls)
        self.assertNotIn("cargo build", new_calls)
        report = self.summary("exfat")
        self.assertEqual(report["target_filesystem"]["filesystem_type"], "exfat")
        self.assertIn("fresh APFS", report["target_cache_error"])
        self.assertEqual(report["steps"][0]["status"], "not_run")
        # Refusal released the same cooperative lock; ordinary APFS use resumes.
        self.assertEqual(self.runner("storage-retry").run(), 0)

    def test_unidentified_macos_storage_refuses_before_owner_claim(self):
        runner = self.runner("storage-unknown")
        with patch.object(local_ci.ci_target_storage, "inspect", side_effect=ValueError("probe timed out")):
            self.assertEqual(runner.run(), 1)
        self.assertFalse(list(self.target_state_dir.glob("*.owner.json")))
        self.assertIsNone(runner.target_lock)
        self.assertIn("before cleanup", self.summary("storage-unknown")["target_cache_error"])
        self.assertNotIn("cargo build", self.call_log.read_text())

    def test_cleanup_refusal_after_passing_format_reports_actual_phase(self):
        self.assertEqual(self.runner("phase-owner").run(), 0)
        owner = next(self.target_state_dir.glob("*.owner.json"))
        before = owner.read_bytes()
        runner = self.runner("phase-failure", env={**self.env, "FAIL_MATCH": "clean"},
                             adopt_target=True,
                             steps=(local_ci.Step("format", ("first",)),
                                    local_ci.Step("build", ("cargo", "build"))))
        # Force the ordinary supported cleanup path, retaining a valid owner.
        with patch.object(runner, "_provenance_cleanup", return_value=["fixture 0.1.0"]):
            console = io.StringIO()
            with redirect_stdout(console):
                self.assertEqual(runner.run(), 1)
        summary = self.summary("phase-failure")
        self.assertEqual([s["status"] for s in summary["steps"]], ["passed", "not_run"])
        self.assertEqual(owner.read_bytes(), before)
        self.assertGreaterEqual(summary["target_cache"]["cleanup_duration_seconds"], 0)
        output = console.getvalue()
        self.assertIn("step: cargo-target-preparation", output)
        self.assertNotIn("step: format", output)
        self.assertIn("target-handoff-clean.log", output)
        self.assertIn("workspace dev-artifact cleanup failed", output)

    def test_pending_admission_refuses_dev_release_and_adoption_without_mutation(self):
        target = Path(self.env["FAKE_METADATA_TARGET"]).resolve()
        target.mkdir()
        (target / "keep").write_bytes(b"original target")
        self.assertEqual(self.runner("pending-seed", adopt_target=True).run(), 0)
        self.release_attempt("pending-release-seed", adopt=True)
        lock_path, owner = local_ci.target_state_paths(target, self.target_state_dir)
        release = owner.with_name(owner.name.replace(".owner.json", ".release-owner.json"))
        pending = local_ci.target_admission_pending_path(lock_path)
        before_owner, before_release = owner.read_bytes(), release.read_bytes()
        before_lock = lock_path.read_bytes()
        for kind in ("regular", "dangling"):
            with self.subTest(kind=kind):
                if kind == "regular":
                    pending.write_bytes(b"incomplete claim")
                else:
                    pending.symlink_to(self.base / "missing")
                for adopt in (False, True):
                    calls_before = self.call_log.read_text()
                    runner = self.runner(f"pending-{kind}-{adopt}", adopt_target=adopt)
                    self.assertEqual(runner.run(), 1)
                    self.assertIn("admission is incomplete", runner.summary["target_cache_error"])
                    self.assertIsNone(runner.target_lock)
                    with self.assertRaisesRegex(local_ci.TargetCacheError, "admission is incomplete"):
                        self.release_attempt(f"pending-release-{kind}-{adopt}", adopt=adopt)
                    calls = self.call_log.read_text()[len(calls_before):]
                    self.assertNotIn("cargo clean", calls)
                    self.assertNotIn("cargo build", calls)
                    self.assertEqual((target / "keep").read_bytes(), b"original target")
                    self.assertEqual(owner.read_bytes(), before_owner)
                    self.assertEqual(release.read_bytes(), before_release)
                    self.assertEqual(lock_path.read_bytes(), before_lock)
                    self.assertTrue(os.path.lexists(pending))
                pending.unlink()
        # Refusals release the actual flock without altering its metadata.
        lock = local_ci.CargoTargetLock(lock_path, {"run_id": "after-pending"})
        lock.acquire()
        self.assertIsNone(lock.release())

    def test_pending_admission_inspection_failure_releases_lock(self):
        target = Path(self.env["FAKE_METADATA_TARGET"]).resolve()
        lock_path, _ = local_ci.target_state_paths(target, self.target_state_dir)
        lock = local_ci.CargoTargetLock(lock_path, {"run_id": "inspection-error"})
        original = os.stat
        pending_name = local_ci.target_admission_pending_path(lock_path).name
        def deny(path, *args, **kwargs):
            if path == pending_name:
                raise PermissionError("pending inspection denied")
            return original(path, *args, **kwargs)
        with patch.object(local_ci.os, "stat", side_effect=deny):
            with self.assertRaisesRegex(local_ci.TargetCacheError, "pending inspection denied"):
                lock.acquire()
        self.assertIsNone(lock.file)
        lock.acquire()
        self.assertIsNone(lock.release())

    def test_pending_guard_refuses_replaced_lock_or_state_directory(self):
        target = Path(self.env["FAKE_METADATA_TARGET"]).resolve()
        for kind in ("lock", "directory"):
            with self.subTest(kind=kind):
                state = self.base / f"state-{kind}"
                state.mkdir()
                path, _ = local_ci.target_state_paths(target, state)
                path.write_bytes(b"previous lock metadata")
                lock = local_ci.CargoTargetLock(path, {"run_id": kind})
                original = os.stat
                replaced = False
                def replace(path_arg, *args, **kwargs):
                    nonlocal replaced
                    if not replaced and ((kind == "lock" and path_arg == path.name)
                                         or (kind == "directory" and path_arg == state)):
                        replaced = True
                        if kind == "lock":
                            path.rename(state / "original.lock")
                            path.write_bytes(b"replacement")
                        else:
                            state.rename(self.base / "original-state")
                            state.mkdir()
                    return original(path_arg, *args, **kwargs)
                with patch.object(local_ci.os, "stat", side_effect=replace):
                    with self.assertRaisesRegex(local_ci.TargetCacheError, "identity changed"):
                        lock.acquire()
                self.assertTrue(replaced)
                self.assertIsNone(lock.file)
                original_path = state / "original.lock" if kind == "lock" else self.base / "original-state" / path.name
                self.assertEqual(original_path.read_bytes(), b"previous lock metadata")
                if kind == "lock":
                    self.assertEqual(path.read_bytes(), b"replacement")

    def test_pending_guard_interruption_releases_flock(self):
        target = Path(self.env["FAKE_METADATA_TARGET"]).resolve()
        lock_path, _ = local_ci.target_state_paths(target, self.target_state_dir)
        pending = local_ci.target_admission_pending_path(lock_path).name
        original = os.stat
        def interrupt(path, *args, **kwargs):
            if path == pending:
                raise KeyboardInterrupt()
            return original(path, *args, **kwargs)
        for stage in ("flock", "pending"):
            with self.subTest(stage=stage):
                lock = local_ci.CargoTargetLock(lock_path, {"run_id": stage})
                operation = (patch.object(local_ci.fcntl, "flock", side_effect=KeyboardInterrupt())
                             if stage == "flock" else patch.object(local_ci.os, "stat", side_effect=interrupt))
                with operation, self.assertRaises(KeyboardInterrupt):
                    lock.acquire()
                self.assertIsNone(lock.file)
                lock.acquire()
                self.assertIsNone(lock.release())

    def test_pending_guard_rechecks_lock_after_pending_lookup(self):
        target = Path(self.env["FAKE_METADATA_TARGET"]).resolve()
        for kind in ("lock", "directory"):
            with self.subTest(kind=kind):
                state = self.base / f"late-state-{kind}"
                state.mkdir()
                path, _ = local_ci.target_state_paths(target, state)
                path.write_bytes(b"previous lock metadata")
                pending = local_ci.target_admission_pending_path(path).name
                lock = local_ci.CargoTargetLock(path, {"run_id": kind})
                original = os.stat
                def replace_during_pending(path_arg, *args, **kwargs):
                    if path_arg == pending:
                        if kind == "lock":
                            path.rename(state / "original.lock")
                            path.write_bytes(b"replacement")
                        else:
                            state.rename(self.base / "late-original-state")
                            state.mkdir()
                        raise FileNotFoundError("pending absent")
                    return original(path_arg, *args, **kwargs)
                with patch.object(local_ci.os, "stat", side_effect=replace_during_pending):
                    with self.assertRaisesRegex(local_ci.TargetCacheError, "identity changed"):
                        lock.acquire()
                self.assertIsNone(lock.file)
                original_path = state / "original.lock" if kind == "lock" else self.base / "late-original-state" / path.name
                self.assertEqual(original_path.read_bytes(), b"previous lock metadata")

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

        original_flock = local_ci.fcntl.flock
        def failed_unlock(descriptor, operation):
            if operation == local_ci.fcntl.LOCK_UN:
                raise OSError("unlock denied")
            return original_flock(descriptor, operation)

        holder = local_ci.CargoTargetLock(path, {"run_id": "release-unlock-failed"})
        holder.acquire()
        with patch.object(local_ci.fcntl, "flock", side_effect=failed_unlock):
            release_error = holder.release()
        self.assertIn("unlock denied", release_error or "")
        next_holder = local_ci.CargoTargetLock(path, {"run_id": "after-release-failure"})
        next_holder.acquire()
        next_holder.release()

        original_release = local_ci.CargoTargetLock.release

        def release_with_unlock_failure(lock: local_ci.CargoTargetLock) -> str | None:
            with patch.object(local_ci.fcntl, "flock", side_effect=failed_unlock):
                return original_release(lock)

        with patch.object(local_ci.CargoTargetLock, "release", release_with_unlock_failure):
            self.assertEqual(self.runner("runner-lock-release-failed").run(), 0)
        release_summary = self.summary("runner-lock-release-failed")
        self.assertIn("unlock denied", release_summary["target_lock_release_error"])

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
        install_delivery_source_fixture(root)
        api = root / "crates/fixture-api"
        app = root / "crates/fixture-app"
        helper = self.base / "shared-external-helper"
        retained = self.base / "retained-external-helper"
        for directory in (api / "src", app / "src", helper / "src", retained / "src"):
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
        (retained / "Cargo.toml").write_text(
            '[package]\nname = "retained-helper"\nversion = "0.1.0"\nedition = "2021"\n')
        (retained / "src/lib.rs").write_text("pub fn value() -> u32 { 9 }\n")
        (app / "Cargo.toml").write_text(
            '[package]\nname = "fixture-app"\nversion = "0.1.0"\nedition = "2021"\n'
            '[dependencies]\nfixture-api = { path = "../fixture-api" }\n'
            f'external-helper = {{ path = {json.dumps(str(helper))} }}\n'
            f'retained-helper = {{ path = {json.dumps(str(retained))} }}\n',
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
            "if os.environ.get('ASSERT_PATH_CLEAN') == '1' and sys.argv[1:2] == ['build']:\n"
            "    assert not glob.glob(os.path.join(os.environ['CARGO_TARGET_DIR'], 'debug/deps/libexternal_helper-*.rlib'))\n"
            "os.execv(real, [real, *sys.argv[1:]])\n",
            encoding="utf-8",
        )
        cargo_wrapper.chmod(0o755)
        env["PATH"] = f"{wrapper_dir}{os.pathsep}{env.get('PATH', '')}"
        run_a = local_ci.LocalCiRunner(
            root, run_id="real-a", env=env, steps=steps, min_free_bytes=0, target_state_dir=state
        )
        self.assertEqual(run_a.run(), 0)
        release_binary = target / "release/fixture-app"
        release_digest = hashlib.sha256(release_binary.read_bytes()).hexdigest()
        helper_artifacts = sorted((target / "debug/deps").glob("libexternal_helper-*.rlib"))
        self.assertTrue(helper_artifacts, "external path dependency artifact was not produced")
        helper_digests = {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in helper_artifacts}
        retained_digests = {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                            for p in (target / "debug/deps").glob("libretained_helper-*.rlib")}
        self.assertTrue(retained_digests)
        api_artifacts = sorted((target / "debug/deps").glob("libfixture_api-*.rlib"))
        self.assertTrue(api_artifacts, "old workspace API artifact was not produced")

        env_b_runtime = {
            **env,
            "PATH": env["PATH"],
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
        self.assertEqual(set(cleanup_packages), {"fixture-api", "fixture-app"})
        cleanup_log = worktree_b / summary_b["target_cache"]["cleanup_log"]
        self.assertIn("Removed", cleanup_log.read_text(encoding="utf-8"))
        self.assertNotIn("qualifier", cleanup_log.read_text(encoding="utf-8"))
        self.assertEqual(
            hashlib.sha256(release_binary.read_bytes()).hexdigest(), release_digest,
            "dev handoff cleanup changed the release artifact",
        )
        surviving_helpers = {path.name: hashlib.sha256(path.read_bytes()).hexdigest()
                             for path in (target / "debug/deps").glob("libexternal_helper-*.rlib")}
        self.assertEqual(surviving_helpers, helper_digests,
                         "dev handoff cleanup changed an external path dependency artifact")
        # A changed path dependency is invalidated by its unique name too.
        # The wrapper observes removal before rebuilding; the other path package
        # and the release binary must remain byte-identical.
        (helper / "src/lib.rs").write_text("pub fn helper() -> u32 { 2 }\n")
        run_c = local_ci.LocalCiRunner(
            worktree_b, run_id="path-change", env={**env, "ASSERT_PATH_CLEAN": "1"},
            steps=(steps[0],), min_free_bytes=0, target_state_dir=state)
        self.assertEqual(run_c.run(), 0)
        cache = run_c.summary["target_cache"]
        selected = [cache["cleanup_argv"][i + 1] for i, arg in enumerate(cache["cleanup_argv"][:-1])
                    if arg == "--package"]
        self.assertEqual(set(selected), {"fixture-api", "fixture-app", "external-helper"})
        self.assertNotIn("qualifier", (worktree_b / cache["cleanup_log"]).read_text())
        self.assertEqual({p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                          for p in (target / "debug/deps").glob("libretained_helper-*.rlib")}, retained_digests)
        self.assertEqual(hashlib.sha256(release_binary.read_bytes()).hexdigest(), release_digest)
        print(
            "real cargo handoff: workspace dev outputs rebuilt; external path dependency and "
            f"release binary preserved (target={target})",
            flush=True,
        )

    @unittest.skipUnless(shutil.which("cargo") and shutil.which("git"), "requires real Cargo and Git")
    def test_real_duplicate_path_package_names_refuse_without_cleaning_artifacts(self):
        root = self.base / "collision-workspace"
        (root / "src").mkdir(parents=True)
        install_delivery_source_fixture(root)
        dependencies = []
        for alias, version in (("first", "0.1.0"), ("second", "0.2.0")):
            dependency = self.base / alias
            (dependency / "src").mkdir(parents=True)
            (dependency / "Cargo.toml").write_text(
                f'[package]\nname="same-name"\nversion="{version}"\nedition="2021"\n')
            (dependency / "src/lib.rs").write_text('pub fn value() -> u32 { 1 }\n')
            dependencies.append(f'{alias}={{package="same-name",path={json.dumps(str(dependency))}}}')
        (root / "Cargo.toml").write_text(
            '[package]\nname="collision-app"\nversion="0.1.0"\nedition="2021"\n'
            '[workspace]\n[dependencies]\n' + '\n'.join(dependencies) + '\n')
        (root / "src/main.rs").write_text('fn main() { println!("{}", first::value()+second::value()); }\n')
        (root / ".gitignore").write_text("/artifacts/\n")
        target = self.base / "collision-target"
        env = {**os.environ, "CARGO_TARGET_DIR": str(target)}
        for argv in (["git", "init", "-q"], ["cargo", "build", "--offline"],
                     ["git", "config", "user.name", "cleanup fixture"],
                     ["git", "config", "user.email", "fixture@example.invalid"],
                     ["git", "add", "."], ["git", "commit", "-qm", "collision fixture"]):
            result = subprocess.run(argv, cwd=root, env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
        before = {str(p.relative_to(target)): hashlib.sha256(p.read_bytes()).hexdigest()
                  for p in target.rglob("*") if p.is_file()}
        state = self.base / "collision-ownership"
        runner = local_ci.LocalCiRunner(
            root, run_id="ambiguous", env=env, adopt_target=True, min_free_bytes=0,
            steps=(local_ci.Step("build", ("cargo", "build", "--offline")),), target_state_dir=state)
        self.assertEqual(runner.run(), 1)
        self.assertIn("ambiguous package name", runner.summary["target_cache_error"])
        self.assertFalse(list(state.glob("*.owner.json")))
        self.assertFalse((runner.output_dir / "target-handoff-clean.log").exists())
        self.assertEqual({str(p.relative_to(target)): hashlib.sha256(p.read_bytes()).hexdigest()
                          for p in target.rglob("*") if p.is_file()}, before)
        print("real Cargo source/version name collision refused before cleanup; artifacts preserved", flush=True)


if __name__ == "__main__":
    unittest.main()
