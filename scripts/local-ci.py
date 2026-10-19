#!/usr/bin/env python3
"""Run Hex's local validation pipeline and retain a machine-readable report."""

from __future__ import annotations

import argparse
from contextlib import contextmanager
from dataclasses import dataclass
from datetime import datetime, timezone
import errno
import fcntl
import hashlib
import json
import os
import shutil
from pathlib import Path
import signal
import stat
import subprocess
import sys
import tempfile
import threading
import time
import uuid


ROOT = Path(__file__).resolve().parent.parent
GIB = 1024 ** 3
DEFAULT_FULL_MIN_FREE_BYTES = 2 * GIB


@contextmanager
def terminal_progress(label: str, *, announce: bool = True):
    """Show activity only around work whose child output is captured/logged.

    Callers must not print or inherit subprocess terminal output in this scope.
    Redirected stdout or stderr gets one plain stage line and no worker thread.
    """
    stream = sys.stderr
    if not (sys.stdout.isatty() and stream.isatty()):
        if announce:
            print(f"{label}...", file=stream, flush=True)
        yield
        return

    stopped = threading.Event()

    def draw(frame: str) -> None:
        # Leave a spare column so a narrow/resized terminal never wraps frames.
        width = max(1, shutil.get_terminal_size().columns - 1)
        stream.write(f"\r\033[2K{(frame + ' ' + label)[:width]}")
        stream.flush()

    def animate() -> None:
        frames = "/-\\|"
        index = 0
        while not stopped.wait(0.1):
            try:
                draw(frames[index % len(frames)])
            except (OSError, ValueError):
                # A display error must not replace the actual subprocess result.
                return
            index += 1

    worker = threading.Thread(target=animate, name="hex-terminal-progress")
    try:
        draw("|")
        worker.start()
        yield
    finally:
        stopped.set()
        try:
            if worker.ident is not None:
                worker.join()
        finally:
            try:
                stream.write("\r\033[2K")
                stream.flush()
            except (OSError, ValueError):
                pass


@dataclass(frozen=True)
class Step:
    step_id: str
    argv: tuple[str, ...]
    uses_cargo_target: bool = False


QUICK_STEPS = (
    Step("changelog", ("python3", "scripts/changelog.py", "validate", "--if-present")),
    Step("architecture", ("python3", "scripts/check-architecture.py")),
    Step("gpui-patch", ("python3", "scripts/check-gpui-patch.py")),
    Step("dependency-patches", ("python3", "scripts/check-dependency-patches.py")),
    Step("ui-style", ("python3", "scripts/check-ui-style.py")),
    Step("device-layout", ("python3", "scripts/check-device-layout.py")),
    Step("format", ("cargo", "fmt", "--all", "--check")),
)

FULL_STEPS = (
    Step(
        "runner-tests",
        ("python3", "-m", "unittest", "discover", "-s", "scripts/tests", "-p", "test_*.py"),
    ),
    *QUICK_STEPS,
    Step("dependency-future-lints", ("python3", "scripts/check-dependency-future-lints.py"), uses_cargo_target=True),
    Step("workspace-tests", ("cargo", "test", "--workspace", "--locked")),
    Step("prepared-export-instrument-tests", ("cargo", "test", "-p", "hex-export", "--features", "instrument-test-support", "--locked")),
    Step("ui-feature-tests", ("cargo", "test", "-p", "hex-ui", "--features", "ui-test-support", "--locked")),
    Step("device-gallery-tests", ("cargo", "test", "-p", "hex-ui", "--example", "layout_review", "--features", "ui-test-support", "--locked")),
    Step("audio-allocation-probe", ("cargo", "test", "-p", "hex-audio", "-p", "hex-desktop", "--features", "allocation-probe", "--locked")),
    Step("stream-allocation-probe", ("cargo", "test", "-p", "hex-stream", "--features", "allocation-probe", "--locked")),
    Step("workspace-clippy", ("cargo", "clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings")),
    Step("prepared-export-instrument-clippy", ("cargo", "clippy", "-p", "hex-export", "--all-targets", "--features", "instrument-test-support", "--locked", "--", "-D", "warnings")),
    Step("ui-feature-clippy", ("cargo", "clippy", "-p", "hex-ui", "--all-targets", "--features", "ui-test-support", "--locked", "--", "-D", "warnings")),
    Step("audio-feature-clippy", ("cargo", "clippy", "-p", "hex-audio", "-p", "hex-desktop", "--all-targets", "--features", "allocation-probe", "--locked", "--", "-D", "warnings")),
    Step("stream-feature-clippy", ("cargo", "clippy", "-p", "hex-stream", "--all-targets", "--features", "allocation-probe", "--locked", "--", "-D", "warnings")),
    Step("desktop-build", ("cargo", "build", "-p", "hex-desktop", "--locked")),
)


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def command_output(argv: list[str], cwd: Path, env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(argv, cwd=cwd, env=env, capture_output=True, text=True, check=False)


class InterruptedRun(Exception):
    """Raised when a user or supervisor interrupts the pipeline."""


class TargetCacheError(Exception):
    """Raised when Cargo target ownership is unknown or already in use."""


def target_state_paths(target_directory: Path, state_dir: Path | None = None) -> tuple[Path, Path]:
    """Return persistent cooperative-lock and owner-marker paths for a target."""
    identity = hashlib.sha256(os.fsencode(str(target_directory))).hexdigest()
    location = state_dir or (Path(tempfile.gettempdir()) / "hex-local-ci-targets")
    return location / f"{identity}.lock", location / f"{identity}.owner.json"


class CargoTargetLock:
    """Nonblocking cooperative lock; direct Cargo commands do not participate."""

    def __init__(self, path: Path, holder: dict[str, object]) -> None:
        self.path = path
        self.holder = holder
        self.file = None

    def acquire(self) -> None:
        try:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            if self.path.is_symlink():
                raise TargetCacheError(f"Cargo target lock path is a symlink: {self.path}")
            self.file = self.path.open("a+", encoding="utf-8")
            fcntl.flock(self.file.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            previous = ""
            detail = ""
            if self.file is not None:
                try:
                    self.file.seek(0)
                    previous = self.file.read().strip()
                except OSError as read_error:
                    detail = f"; could not read current holder: {read_error}"
                finally:
                    try:
                        self.file.close()
                    except OSError as close_error:
                        detail = f"{detail}; closing lock file failed: {close_error}"
                    self.file = None
            holder_detail = f"; current holder: {previous}" if previous else ""
            raise TargetCacheError(
                f"Cargo target is already in use by another local CI run{holder_detail}{detail}"
            ) from error
        except OSError as error:
            detail = str(error)
            if self.file is not None:
                try:
                    self.file.close()
                except OSError as close_error:
                    detail = f"{detail}; closing lock file also failed: {close_error}"
                self.file = None
            if error.errno in (errno.EACCES, errno.EAGAIN):
                raise TargetCacheError("Cargo target is already in use by another local CI run") from error
            raise TargetCacheError(f"could not open or lock Cargo target: {detail}") from error
        try:
            self._write({"active_holder": self.holder})
        except OSError as error:
            detail = str(error)
            if self.file is not None:
                try:
                    fcntl.flock(self.file.fileno(), fcntl.LOCK_UN)
                except OSError as unlock_error:
                    detail = f"{detail}; unlocking also failed: {unlock_error}"
                finally:
                    try:
                        self.file.close()
                    except OSError as close_error:
                        detail = f"{detail}; closing lock file also failed: {close_error}"
                    self.file = None
            raise TargetCacheError(f"could not record Cargo target lock holder: {detail}") from error

    def _write(self, value: dict[str, object]) -> None:
        if self.file is None:
            return
        self.file.seek(0)
        self.file.truncate()
        json.dump(value, self.file, sort_keys=True)
        self.file.write("\n")
        self.file.flush()

    def release(self) -> str | None:
        if self.file is None:
            return None
        failure = None
        try:
            try:
                self._write({"last_holder": self.holder, "released_at": utc_now()})
            except OSError as error:
                failure = f"could not record Cargo target lock release: {error}"
            try:
                fcntl.flock(self.file.fileno(), fcntl.LOCK_UN)
            except OSError as error:
                if failure is None:
                    failure = f"could not explicitly unlock Cargo target: {error}"
        finally:
            try:
                self.file.close()
            except OSError as error:
                if failure is None:
                    failure = f"could not close Cargo target lock file: {error}"
            self.file = None
        return failure


class LocalCiRunner:
    def __init__(
        self,
        root: Path = ROOT,
        *,
        mode: str = "full",
        run_id: str | None = None,
        env: dict[str, str] | None = None,
        steps: tuple[Step, ...] | None = None,
        min_free_bytes: int | None = None,
        adopt_target: bool = False,
        target_state_dir: Path | None = None,
    ) -> None:
        self.root = root.resolve()
        default_floor = DEFAULT_FULL_MIN_FREE_BYTES if mode == "full" else 0
        self.min_free_bytes = default_floor if min_free_bytes is None else min_free_bytes
        if (
            isinstance(self.min_free_bytes, bool)
            or not isinstance(self.min_free_bytes, int)
            or not 0 <= self.min_free_bytes <= sys.maxsize
        ):
            raise ValueError("minimum free space must be a bounded nonnegative integer byte count")
        self.mode = mode
        self.adopt_target = adopt_target
        self.target_state_dir = target_state_dir
        self.run_id = run_id or f"{datetime.now(timezone.utc):%Y%m%dT%H%M%SZ}-{uuid.uuid4().hex[:8]}"
        self.env = dict(os.environ if env is None else env)
        self.steps = steps or (QUICK_STEPS if mode == "quick" else FULL_STEPS)
        self.output_dir = self.root / "artifacts" / "local-ci" / self.run_id
        self.summary_path = self.output_dir / "summary.json"
        self.started_monotonic = time.monotonic()
        self.summary: dict[str, object] = {
            "schema_version": 1,
            "run_id": self.run_id,
            "mode": self.mode,
            "status": "incomplete",
            "repo_root": str(self.root),
            "commit": None,
            "dirty": None,
            "dirty_files": None,
            "started_at": utc_now(),
            "ended_at": None,
            "exit_code": None,
            "invocation_argv": list(sys.argv),
            "steps": [],
        }
        self.git_identity: dict[str, object] | None = None
        self.cargo_identity: dict[str, object] | None = None
        self._lexical_cargo_target: Path | None = None
        self.target_lock: CargoTargetLock | None = None
        self.target_owner_path: Path | None = None
        self.target_owner_claimed = False

    def _write_summary(self) -> None:
        temporary = self.summary_path.with_suffix(".json.tmp")
        temporary.write_text(json.dumps(self.summary, indent=2, sort_keys=True) + "\n")
        temporary.replace(self.summary_path)

    def _capture_repository_state(self, key: str) -> bool:
        try:
            commit = command_output(["git", "rev-parse", "HEAD"], self.root, self.env)
            top_level = command_output(["git", "rev-parse", "--show-toplevel"], self.root, self.env)
            common_dir = command_output(["git", "rev-parse", "--git-common-dir"], self.root, self.env)
            status = command_output(["git", "status", "--porcelain=v1", "--untracked-files=all"], self.root, self.env)
            tracked = subprocess.run(
                ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
                cwd=self.root,
                env=self.env,
                capture_output=True,
                check=False,
            )
        except OSError as error:
            self.summary["repository_error"] = str(error)
            return False
        if (
            commit.returncode != 0
            or top_level.returncode != 0
            or common_dir.returncode != 0
            or status.returncode != 0
            or tracked.returncode != 0
        ):
            self.summary["repository_error"] = {
                "commit_exit_code": commit.returncode,
                "top_level_exit_code": top_level.returncode,
                "common_dir_exit_code": common_dir.returncode,
                "status_exit_code": status.returncode,
                "files_exit_code": tracked.returncode,
                "commit_stderr": commit.stderr.strip(),
                "top_level_stderr": top_level.stderr.strip(),
                "common_dir_stderr": common_dir.stderr.strip(),
                "status_stderr": status.stderr.strip(),
                "files_stderr": tracked.stderr.decode("utf-8", errors="replace").strip(),
            }
            return False
        dirty_files = [line for line in status.stdout.splitlines() if line]
        digest = hashlib.sha256()
        paths = sorted(path for path in tracked.stdout.split(b"\0") if path)
        try:
            for relative in paths:
                digest.update(len(relative).to_bytes(8, "big"))
                digest.update(relative)
                path = self.root / os.fsdecode(relative)
                try:
                    metadata = path.lstat()
                except FileNotFoundError:
                    digest.update(b"missing\0")
                    continue
                if path.is_symlink():
                    digest.update(b"symlink\0")
                    digest.update(os.fsencode(os.readlink(path)))
                elif path.is_file():
                    digest.update(b"file\0")
                    with path.open("rb") as source:
                        while chunk := source.read(1024 * 1024):
                            digest.update(chunk)
                    digest.update(str(metadata.st_mode & 0o111).encode("ascii"))
                else:
                    digest.update(f"other:{metadata.st_mode}\0".encode("ascii"))
        except OSError as error:
            self.summary["repository_error"] = f"could not hash repository contents: {error}"
            return False
        snapshot = {
            "commit": commit.stdout.strip(),
            "dirty": bool(dirty_files),
            "dirty_files": dirty_files,
            "content_sha256": digest.hexdigest(),
        }
        self.summary[key] = snapshot
        if key == "start_tree":
            root_path = Path(top_level.stdout.strip()).resolve()
            common_path = Path(common_dir.stdout.strip())
            if not common_path.is_absolute():
                common_path = root_path / common_path
            self.git_identity = {
                "workspace_root": str(root_path),
                "git_common_dir": str(common_path.resolve()),
                "commit": snapshot["commit"],
                "dirty": snapshot["dirty"],
                "dirty_files": snapshot["dirty_files"],
                "content_sha256": snapshot["content_sha256"],
            }
            self.summary["source_identity"] = self.git_identity
            self.summary["commit"] = snapshot["commit"]
            self.summary["dirty"] = snapshot["dirty"]
            self.summary["dirty_files"] = snapshot["dirty_files"]
            self.summary["repository_commands"] = [
                ["git", "rev-parse", "HEAD"],
                ["git", "rev-parse", "--show-toplevel"],
                ["git", "rev-parse", "--git-common-dir"],
                ["git", "status", "--porcelain=v1", "--untracked-files=all"],
                ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
            ]
        return True

    def _resolve_cargo_identity(self) -> bool:
        command = [
            "cargo",
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--locked",
            "--manifest-path",
            str(self.root / "Cargo.toml"),
        ]
        configured_target = self.env.get("CARGO_TARGET_DIR")
        try:
            result = subprocess.run(
                command,
                cwd=self.root,
                env=self.env,
                capture_output=True,
                text=True,
                check=False,
            )
        except OSError as error:
            self.summary["cargo_metadata_error"] = f"could not resolve Cargo target: {error}"
            return False
        if result.returncode != 0:
            self.summary["cargo_metadata_error"] = {
                "exit_code": result.returncode,
                "stderr": result.stderr.strip(),
            }
            return False
        try:
            metadata = json.loads(result.stdout)
            raw_target = metadata["target_directory"]
            raw_workspace_root = metadata["workspace_root"]
            members = metadata["workspace_members"]
            packages = metadata["packages"]
            if (
                not isinstance(raw_target, str)
                or not isinstance(raw_workspace_root, str)
                or not isinstance(members, list)
                or not all(isinstance(member, str) for member in members)
                or not isinstance(packages, list)
            ):
                raise ValueError("Cargo metadata fields have invalid types")
            workspace_root = Path(raw_workspace_root).resolve()
            target = Path(raw_target)
            if not target.is_absolute():
                target = self.root / target
            self._lexical_cargo_target = target.absolute()
            target = target.resolve()
            by_id = {
                package["id"]: package
                for package in packages
                if isinstance(package, dict)
                and isinstance(package.get("id"), str)
                and isinstance(package.get("name"), str)
                and isinstance(package.get("version"), str)
            }
            workspace_packages = []
            for member in members:
                package = by_id.get(member)
                if package is None:
                    raise ValueError("workspace package metadata is incomplete")
                workspace_packages.append(
                    {
                        "id": member,
                        "name": package["name"],
                        "version": package["version"],
                        "spec": member,
                    }
                )
            if not workspace_packages:
                raise ValueError("Cargo workspace has no members to clean safely")
        except (KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
            self.summary["cargo_metadata_error"] = f"Cargo metadata was incomplete or invalid: {error}"
            return False

        if workspace_root != self.root:
            self.summary["cargo_metadata_error"] = (
                f"Cargo workspace root {workspace_root} does not match local CI root {self.root}"
            )
            return False
        self.env["CARGO_TARGET_DIR"] = str(target)
        lock_path, owner_path = target_state_paths(target, self.target_state_dir)
        self.target_owner_path = owner_path
        self.cargo_identity = {
            "manifest_path": str(self.root / "Cargo.toml"),
            "workspace_root": str(workspace_root),
            "target_directory": str(target),
            "target_source": "environment" if configured_target is not None else "cargo metadata/config/default",
            "configured_target_value": configured_target,
            "metadata_command": command,
            "workspace_packages": workspace_packages,
            "lock_path": str(lock_path),
            "owner_path": str(owner_path),
        }
        self.summary["cargo_identity"] = self.cargo_identity
        return True

    @staticmethod
    def _uses_cargo_target(step: Step) -> bool:
        return step.uses_cargo_target or bool(
            len(step.argv) > 1
            and step.argv[0] == "cargo"
            and step.argv[1] in {"build", "check", "test", "clippy", "run", "bench", "doc", "clean", "rustc"}
        )

    @staticmethod
    def _directory_identity(path: Path) -> dict[str, int] | None:
        try:
            status = path.stat()
        except FileNotFoundError:
            return None
        except OSError as error:
            raise TargetCacheError(f"could not inspect Cargo target identity: {error}") from error
        if not path.is_dir():
            raise TargetCacheError("resolved Cargo target path is not a directory")
        return {"device": status.st_dev, "inode": status.st_ino}

    def _load_target_owner(self, owner_path: Path, target: Path) -> dict[str, object] | None:
        if not owner_path.exists():
            return None
        try:
            if owner_path.is_symlink():
                raise ValueError("owner marker is a symlink")
            value = json.loads(owner_path.read_text(encoding="utf-8"))
            if not isinstance(value, dict):
                raise ValueError("owner marker is not an object")
            owner = value.get("workspace_owner")
            target_identity = value.get("target_identity")
            if (
                value.get("schema_version") != 1
                or value.get("target_directory") != str(target)
                or not isinstance(owner, dict)
                or not isinstance(owner.get("workspace_root"), str)
                or not isinstance(owner.get("git_common_dir"), str)
                or (
                    target_identity is not None
                    and (
                        not isinstance(target_identity, dict)
                        or set(target_identity) != {"device", "inode"}
                        or not all(isinstance(item, int) and not isinstance(item, bool) for item in target_identity.values())
                    )
                )
            ):
                raise ValueError("owner fields have invalid types")
            return value
        except (OSError, json.JSONDecodeError, ValueError) as error:
            raise TargetCacheError(
                f"Cargo target ownership marker is malformed at {owner_path}; "
                "use a fresh target or explicitly adopt it after checking ownership"
            ) from error

    def _write_target_owner(self, path: Path, owner: dict[str, object]) -> None:
        temporary = path.with_name(f"{path.name}.{os.getpid()}.{self.run_id}.tmp")
        payload = {
            "schema_version": 1,
            "target_directory": self.cargo_identity["target_directory"] if self.cargo_identity else None,
            "workspace_owner": {
                "workspace_root": owner["workspace_root"],
                "git_common_dir": owner["git_common_dir"],
            },
            "target_identity": self._directory_identity(
                Path(str(self.cargo_identity["target_directory"]))
            ) if self.cargo_identity else None,
            "recorded_at": utc_now(),
        }
        temporary.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        temporary.replace(path)

    def _prepare_target_cache(self) -> bool:
        if self.cargo_identity is None or self.git_identity is None:
            self.summary["target_cache_error"] = "Cargo or Git identity is unavailable"
            return False
        target = Path(str(self.cargo_identity["target_directory"]))
        lock_path, owner_path = target_state_paths(target, self.target_state_dir)
        holder = {
            "run_id": self.run_id,
            "workspace_root": self.git_identity["workspace_root"],
            "commit": self.git_identity["commit"],
            "target_directory": str(target),
        }
        lock = CargoTargetLock(lock_path, holder)
        try:
            lock.acquire()
        except TargetCacheError as error:
            self.summary["target_cache_error"] = str(error)
            return False
        self.target_lock = lock
        try:
            malformed_owner = False
            try:
                owner_record = self._load_target_owner(owner_path, target)
            except TargetCacheError:
                if not self.adopt_target:
                    raise
                owner_record = None
                malformed_owner = True
            current_workspace = {
                "workspace_root": self.git_identity["workspace_root"],
                "git_common_dir": self.git_identity["git_common_dir"],
            }
            target_identity = self._directory_identity(target)
            ownership_action = "reuse"
            cleanup_argv: list[str] | None = None
            previous_workspace = None
            if malformed_owner:
                ownership_action = "adopted"
                cleanup_argv = self._workspace_clean_argv(target)
            elif owner_record is None:
                nonempty = False
                if target.exists():
                    try:
                        nonempty = next(target.iterdir(), None) is not None
                    except OSError as error:
                        raise TargetCacheError(f"could not inspect unowned Cargo target: {error}") from error
                if nonempty and not self.adopt_target:
                    raise TargetCacheError(
                        f"Cargo target ownership is unknown at {target}; choose an empty target or pass --adopt-target"
                    )
                if nonempty:
                    ownership_action = "adopted"
                    cleanup_argv = self._workspace_clean_argv(target)
                else:
                    ownership_action = "claim-empty"
            else:
                previous_workspace = owner_record["workspace_owner"]
                prior_target_identity = owner_record.get("target_identity")
                same_workspace = previous_workspace == current_workspace
                if prior_target_identity != target_identity:
                    if not self.adopt_target:
                        raise TargetCacheError(
                            "Cargo target directory identity changed; choose a fresh target or pass --adopt-target"
                        )
                    ownership_action = "adopted"
                    cleanup_argv = self._workspace_clean_argv(target)
                elif not same_workspace and previous_workspace.get("git_common_dir") != current_workspace["git_common_dir"]:
                    if not self.adopt_target:
                        raise TargetCacheError(
                            "Cargo target is owned by a different repository; choose a fresh target or pass --adopt-target"
                        )
                    ownership_action = "adopted"
                    cleanup_argv = self._workspace_clean_argv(target)
                elif not same_workspace:
                    ownership_action = "worktree-handoff"
                    cleanup_argv = self._workspace_clean_argv(target)

            self.summary["target_cache"] = {
                "action": ownership_action,
                "previous_workspace_owner": previous_workspace,
                "current_workspace_owner": current_workspace,
                "target_identity_before": target_identity,
                "cleanup_argv": cleanup_argv,
                "cleanup_status": "not_needed" if cleanup_argv is None else "pending",
                "lock_path": str(lock_path),
                "lock_acquired": True,
            }
            if cleanup_argv is not None:
                result = self._run_target_cleanup(cleanup_argv)
                target_cache = self.summary["target_cache"]
                assert isinstance(target_cache, dict)
                target_cache["cleanup_status"] = "passed" if result else "failed"
                if not result:
                    self.summary["target_cache_error"] = "workspace dev-artifact cleanup failed; previous owner was preserved"
                    return False

            self._write_target_owner(owner_path, current_workspace)
            self.target_owner_claimed = True
            return True
        except (TargetCacheError, OSError) as error:
            self.summary["target_cache_error"] = str(error)
            release_error = lock.release()
            if release_error is not None:
                self.summary["target_lock_release_error"] = release_error
            self.target_lock = None
            return False

    def _workspace_clean_argv(self, target: Path) -> list[str]:
        if self.cargo_identity is None:
            raise TargetCacheError("workspace package metadata is unavailable")
        packages = self.cargo_identity["workspace_packages"]
        if not isinstance(packages, list) or not packages:
            raise TargetCacheError("workspace package list is unavailable")
        argv = [
            "cargo",
            "clean",
            "--profile",
            "dev",
            "--target-dir",
            str(target),
            "--manifest-path",
            str(self.root / "Cargo.toml"),
        ]
        for package in packages:
            if not isinstance(package, dict) or not isinstance(package.get("spec"), str):
                raise TargetCacheError("workspace package metadata is malformed")
            argv.extend(("--package", package["spec"]))
        return argv

    def _release_scope(self) -> list[dict[str, str]]:
        """Cargo clean ignores URL/version qualifiers: reject external name collisions."""
        result = command_output(
            ["cargo", "metadata", "--format-version", "1", "--locked"], self.root, self.env
        )
        if result.returncode != 0:
            raise TargetCacheError(f"release package metadata failed: {result.stderr.strip()}")
        try:
            metadata = json.loads(result.stdout)
            members = set(metadata["workspace_members"])
            packages = metadata["packages"]
            scope = sorted(
                ({"name": p["name"], "version": p["version"], "spec": p["id"]}
                 for p in packages if p["id"] in members), key=lambda p: p["name"]
            )
            names = {p["name"] for p in scope}
            if not scope or len(names) != len(scope) or any(
                p["id"] not in members and p["name"] in names for p in packages
            ):
                raise ValueError("workspace package names overlap other packages")
            if any(not all(isinstance(v, str) and v for v in p.values()) for p in scope):
                raise ValueError("invalid package fields")
            if self.cargo_identity is None or {p["spec"] for p in scope} != {
                p["spec"] for p in self.cargo_identity["workspace_packages"]
            }:
                raise ValueError("workspace package metadata changed")
            return scope
        except (KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
            raise TargetCacheError(f"unsafe release package selection: {error}") from error

    @staticmethod
    def _write_release_owner(path: Path, payload: dict[str, object]) -> None:
        if path.is_symlink():
            raise TargetCacheError("release ownership marker is a symlink")
        temporary = path.with_name(f"{path.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
        with temporary.open("x", encoding="utf-8") as stream:
            json.dump(payload, stream, indent=2, sort_keys=True)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        temporary.replace(path)
        descriptor = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)

    def inspect_recreated_release_target(self) -> dict[str, object]:
        """Read-only explicit recovery authority, while the target lock is held.

        This never adopts an unknown/foreign owner or certifies its old artifact.
        The caller must revalidate the returned hash and replacement identity at
        the guarded mutation boundary; ordinary automatic restart stays stricter.
        """
        if self.target_lock is None or self.cargo_identity is None:
            raise TargetCacheError("recreated-target inspection requires target ownership")
        target = Path(str(self.cargo_identity["target_directory"]))
        lexical = self._lexical_cargo_target
        if lexical is None:
            raise TargetCacheError("recreated target has no lexical path evidence")
        for path in (lexical, *lexical.parents):
            if path.is_symlink():
                raise TargetCacheError("recreated target path contains a symlink")
        identity = self._directory_identity(target)
        if identity is None:
            raise TargetCacheError("recreated target must exist")
        _, dev_owner = target_state_paths(target, self.target_state_dir)
        owner = dev_owner.with_name(dev_owner.name.replace(".owner.json", ".release-owner.json"))
        try:
            descriptor = os.open(owner, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
            with os.fdopen(descriptor, "rb") as stream:
                if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
                    raise ValueError("owner is not regular")
                raw = stream.read(128 * 1024 + 1)
            if len(raw) > 128 * 1024:
                raise ValueError("owner exceeds bounded metadata size")
            record = json.loads(raw)
            workspace = {k: self.git_identity[k] for k in ("workspace_root", "git_common_dir")}
            previous = record["source"]
            old_identity = record["target_identity"]
            def hex_string(value, size):
                return (isinstance(value, str) and len(value) == size
                        and all(c in "0123456789abcdef" for c in value))
            if (type(record.get("schema_version")) is not int or record["schema_version"] != 1
                    or record.get("profile") != "release"
                    or record.get("state") not in {"pending", "ready"}
                    or record.get("target_directory") != str(target)
                    or record.get("workspace_owner") != workspace
                    or not isinstance(old_identity, dict) or set(old_identity) != {"device", "inode"}
                    or any(type(old_identity[k]) is not int or old_identity[k] < (1 if k == "inode" else 0)
                           for k in ("device", "inode"))
                    or old_identity == identity
                    or not isinstance(previous, dict)
                    or set(previous) != {"commit", "dirty", "dirty_files", "content_sha256"}
                    or previous["dirty"] is not False or previous["dirty_files"] != []
                    or not hex_string(previous["commit"], 40)
                    or not hex_string(previous["content_sha256"], 64)):
                raise ValueError("owner is not this repository's prior target")
            scope = self._release_scope()
            packages = record["packages"]
            if (not isinstance(packages, list) or not packages
                    or any(not isinstance(p, dict) or set(p) != {"name", "version", "spec"}
                           or not all(isinstance(v, str) and v for v in p.values()) for p in packages)):
                raise ValueError("invalid prior package scope")
            names = {p["name"] for p in packages}
            if len(names) != len(packages) or packages != [p for p in scope if p["name"] in names]:
                raise ValueError("prior package ownership changed")
            source = self.summary["start_tree"]
            if not hex_string(source["commit"], 40):
                raise ValueError("current source has no exact commit")
            ancestry = subprocess.run(["git", "merge-base", "--is-ancestor", previous["commit"],
                                      source["commit"]], cwd=self.root, env=self.env,
                                     capture_output=True, check=False)
            if ancestry.returncode != 0:
                raise ValueError("prior owner source is not an ancestor of current source")
            if record["state"] == "ready":
                artifact = record["artifact"]
                argv = record["build_argv"]
                if (not isinstance(artifact, dict) or set(artifact) != {"path", "sha256"}
                        or not isinstance(artifact["path"], str)
                        or not Path(artifact["path"]).is_absolute()
                        or ".." in Path(artifact["path"]).parts
                        or target / "release" not in Path(artifact["path"]).parents
                        or not hex_string(artifact["sha256"], 64)
                        or not isinstance(argv, list) or not all(isinstance(a, str) for a in argv)
                        or argv[:2] != ["cargo", "build"] or "--release" not in argv):
                    raise ValueError("malformed historical release artifact")
            if (not self._capture_repository_state("recovery_inspected_tree")
                    or self.summary["recovery_inspected_tree"] != source
                    or self._directory_identity(target) != identity):
                raise ValueError("source or replacement changed during inspection")
            return {"owner_sha256": hashlib.sha256(raw).hexdigest(), "target_identity": identity}
        except (OSError, KeyError, TypeError, ValueError) as error:
            raise TargetCacheError(f"recreated release target inspection refused: {error}") from error

    @contextmanager
    def guarded_release(self, *, adopt: bool = False, expected_owner_sha256: str | None = None,
                        expected_recreated_target_identity: dict[str, int] | None = None,
                        recovery_inactive_check=None):
        """Explicit release handoff; caller must identify its artifact before exit.

        Uses the dev driver's SAME target-wide lock, but a separate durable
        marker. Exceptions leave pending ownership, requiring explicit recovery.
        Raw Cargo commands do not participate in this cooperative contract.
        """
        if self.target_lock is not None:
            raise TargetCacheError("release preparation requires an unheld runner")
        self.output_dir.mkdir(parents=True, exist_ok=True)
        if not self._capture_repository_state("start_tree") or self.summary["start_tree"]["dirty"]:
            raise TargetCacheError("release preparation requires exact clean source")
        if not self._resolve_cargo_identity() or not self._check_disk_headroom():
            raise TargetCacheError(f"release preparation failed: {self.summary}")
        target = Path(str(self.cargo_identity["target_directory"]))
        lock_path, dev_owner_path = target_state_paths(target, self.target_state_dir)
        owner_path = dev_owner_path.with_name(dev_owner_path.name.replace(".owner.json", ".release-owner.json"))
        lock = CargoTargetLock(lock_path, {
            "run_id": self.run_id, "profile": "release", "target_directory": str(target),
            "workspace_root": str(self.root), "commit": self.git_identity["commit"],
        })
        lock.acquire()
        self.target_lock = lock
        try:
            replacement = expected_recreated_target_identity
            if replacement is not None:
                if (not adopt or expected_owner_sha256 is None
                        or not isinstance(replacement, dict) or set(replacement) != {"device", "inode"}
                        or any(type(replacement[k]) is not int or replacement[k] < (1 if k == "inode" else 0)
                               for k in ("device", "inode"))
                        or not callable(recovery_inactive_check)):
                    raise TargetCacheError("explicit recreated-target recovery requires identity, owner hash and inactivity check")
                inspected = self.inspect_recreated_release_target()
                if inspected != {"owner_sha256": expected_owner_sha256, "target_identity": replacement}:
                    raise TargetCacheError("recreated target or owner changed since inspection")
            expected_owner_bytes = None
            if expected_owner_sha256 is not None:
                if (not adopt or len(expected_owner_sha256) != 64
                        or any(c not in "0123456789abcdef" for c in expected_owner_sha256)):
                    raise TargetCacheError("expected release owner requires explicit adoption and its SHA256")
                try:
                    descriptor = os.open(owner_path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
                    with os.fdopen(descriptor, "rb") as stream:
                        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
                            raise TargetCacheError("expected release owner is not a regular file")
                        expected_owner_bytes = stream.read(128 * 1024 + 1)
                    if (len(expected_owner_bytes) > 128 * 1024
                            or hashlib.sha256(expected_owner_bytes).hexdigest() != expected_owner_sha256):
                        raise TargetCacheError("release owner changed since restart inspection; preserved unchanged")
                except OSError as error:
                    raise TargetCacheError("expected release owner is unavailable; preserved unchanged") from error
            # Recheck source after acquiring ownership; a changed checkout refuses.
            if not self._capture_repository_state("release_locked_tree") or self.summary["release_locked_tree"] != self.summary["start_tree"]:
                raise TargetCacheError("release source changed before lock acquisition")
            scope = self._release_scope()
            workspace = {k: self.git_identity[k] for k in ("workspace_root", "git_common_dir")}
            source = self.summary["start_tree"]
            identity = self._directory_identity(target)
            record = None
            malformed = False
            if owner_path.exists() or owner_path.is_symlink():
                try:
                    if owner_path.is_symlink():
                        raise ValueError("symlink marker")
                    record = json.loads(expected_owner_bytes if expected_owner_bytes is not None else owner_path.read_text())
                    if (not isinstance(record, dict) or record.get("schema_version") != 1
                            or not {"target_identity", "source", "workspace_owner", "packages"}.issubset(record)
                            or record.get("profile") != "release" or record.get("target_directory") != str(target)
                            or record.get("state") not in {"pending", "ready"}
                            or not isinstance(record.get("workspace_owner"), dict)
                            or set(record["workspace_owner"]) != {"workspace_root", "git_common_dir"}
                            or not all(isinstance(v, str) and v for v in record["workspace_owner"].values())
                            or not isinstance(record.get("source"), dict)
                            or set(record["source"]) != {"commit", "dirty", "dirty_files", "content_sha256"}
                            or record["source"].get("dirty") is not False
                            or record["source"].get("dirty_files") != []
                            or not isinstance(record["source"].get("commit"), str)
                            or not isinstance(record["source"].get("content_sha256"), str)
                            or len(record["source"]["content_sha256"]) != 64
                            or any(c not in "0123456789abcdef" for c in record["source"]["content_sha256"])
                            or not isinstance(record.get("target_identity"), (dict, type(None)))
                            or (record["target_identity"] is not None and
                                (set(record["target_identity"]) != {"device", "inode"} or
                                 not all(type(v) is int and v >= 0 for v in record["target_identity"].values())))
                            or not isinstance(record.get("packages"), list)
                            or not record["packages"]
                            or any(not isinstance(p, dict) or set(p) != {"name", "version", "spec"}
                                   or not all(isinstance(v, str) and v for v in p.values()) for p in record["packages"])):
                        raise ValueError("invalid release owner fields")
                    if record["state"] == "ready":
                        artifact = record.get("artifact")
                        if (not isinstance(artifact, dict) or set(artifact) != {"path", "sha256"}
                                or not all(isinstance(v, str) for v in artifact.values())
                                or len(artifact["sha256"]) != 64
                                or any(c not in "0123456789abcdef" for c in artifact["sha256"])
                                or not isinstance(record.get("build_argv"), list)):
                            raise ValueError("invalid release artifact fields")
                except (ValueError, TypeError, OSError) as error:
                    if not adopt or expected_owner_sha256 is not None:
                        raise TargetCacheError(f"malformed release ownership: {error}") from error
                    record = None
                    malformed = True
            if record is not None:
                # Automatic restart recovery retains the inspected owner, not
                # manual adoption's permission to take over another target.
                if expected_owner_sha256 is not None and (
                        (replacement is None and record["target_identity"] != identity)
                        or record["workspace_owner"] != workspace):
                    raise TargetCacheError("release target identity/workspace changed since restart inspection; preserved unchanged")
                old_scope = [(p["name"], p["version"]) for p in record["packages"]]
                old_names = {name for name, _version in old_scope}
                retained_scope = [(p["name"], p["version"]) for p in scope if p["name"] in old_names]
                # New members are cleanable through the current workspace; every
                # old member must still be present with its original name/version.
                if old_scope != retained_scope:
                    raise TargetCacheError("removed, renamed or version-changed workspace packages require a fresh release target")
                if record["state"] == "pending" and not adopt:
                    raise TargetCacheError("release handoff is pending; explicit adoption/recovery required")
                if (record.get("target_identity") != identity or record["workspace_owner"].get("git_common_dir") != workspace["git_common_dir"]) and not adopt:
                    raise TargetCacheError("release target identity/repository changed; explicit adoption required")
            # An existing dev marker is NOT evidence of release ownership.
            populated = target.exists() and any(target.iterdir())
            if record is None and populated and not adopt:
                raise TargetCacheError("release ownership is unknown; explicit adoption or an empty target required")
            reuse = bool(record and record["state"] == "ready" and not adopt
                         and record.get("target_identity") == identity
                         and record["workspace_owner"] == workspace and record["source"] == source)
            pending = {"schema_version": 1, "profile": "release", "state": "pending",
                       "target_directory": str(target), "target_identity": identity,
                       "workspace_owner": workspace, "source": source, "packages": scope,
                       "recorded_at": utc_now()}
            report = {**pending, "owner_path": str(owner_path), "lock_path": str(lock_path),
                      "action": "reuse" if reuse else ("adopted" if adopt else "handoff"),
                      "cleanup_status": "not_needed", "previous_owner": record,
                      "malformed_previous_owner": malformed}
            if replacement is not None:
                # Last check before pending ownership/cleanup, under the SAME
                # target lock. No process control is permitted by this callback.
                recovery_inactive_check()
                if self.inspect_recreated_release_target() != inspected:
                    raise TargetCacheError("recreated target or owner changed before mutation")
                report["explicit_recreated_target"] = inspected
            self.summary["release_target_cache"] = report
            self._write_release_owner(owner_path, pending)
            if not reuse and populated:
                argv = self._workspace_clean_argv(target)
                argv[3] = "release"
                report["cleanup_argv"] = argv
                report["cleanup_status"] = "pending"
                # Keep release and dev cleanup evidence independent.
                log_path = self.output_dir / "release-handoff-clean.log"
                with log_path.open("w") as log:
                    result = subprocess.run(argv, cwd=self.root, env=self.env, stdout=log,
                                            stderr=subprocess.STDOUT, check=False)
                report.update(cleanup_exit_code=result.returncode, cleanup_log=str(log_path))
                if result.returncode != 0:
                    report["cleanup_status"] = "failed"
                    raise TargetCacheError("release cleanup failed; pending owner retained")
                report["cleanup_status"] = "passed"
            yield report
            if not isinstance(report.get("artifact"), dict) or report.get("build_exit_code") != 0:
                raise TargetCacheError("release artifact identification is required")
            artifact = report["artifact"]
            if self.identify_release_artifact(Path(artifact["path"]), report["build_argv"]) != artifact:
                raise TargetCacheError("release artifact changed before ownership completion")
            if not self._capture_repository_state("release_end_tree") or self.summary["release_end_tree"] != source:
                raise TargetCacheError("release source changed during build")
            pending.update(state="ready", target_identity=self._directory_identity(target),
                           artifact=report["artifact"], build_argv=report.get("build_argv"), recorded_at=utc_now())
            self._write_release_owner(owner_path, pending)
            report["state"] = "ready"
        except BaseException as error:
            self.summary["release_target_error"] = {"type": type(error).__name__, "message": str(error)}
            raise
        finally:
            try:
                self._write_summary()
            finally:
                error = lock.release()
                self.target_lock = None
            if error:
                raise TargetCacheError(error)

    def identify_release_artifact(self, path: Path, argv: list[str]) -> dict[str, str]:
        """Identify the Cargo-reported optimized executable while still locked."""
        if self.target_lock is None or self.cargo_identity is None:
            raise TargetCacheError("release artifact identification requires target ownership")
        if argv[:2] != ["cargo", "build"] or "--release" not in argv or "--target-dir" in argv:
            raise TargetCacheError("unsupported release build arguments")
        target = Path(str(self.cargo_identity["target_directory"]))
        lexical = path.absolute()
        resolved = lexical.resolve()
        # Cargo's owned target is canonical. System aliases above it (macOS
        # /var -> /private/var, for example) are not artifact symlinks. Locate
        # the outermost lexical spelling of that same root, then inspect every
        # component below it without resolving away an in-target symlink.
        roots = [p for p in reversed(lexical.parents) if p.resolve() == target]
        root = roots[0] if roots else None
        if (target.resolve() != target or root is None or root.is_symlink()
                or self._directory_identity(root) != self._directory_identity(target)
                or not resolved.is_relative_to(target) or not lexical.is_file()):
            raise TargetCacheError("release artifact is not a plain file inside the owned target")
        relative = lexical.relative_to(root)
        component = root
        for part in relative.parts:
            if part == "..":
                raise TargetCacheError("release artifact traverses outside its target-relative path")
            component = component / part
            if component.is_symlink():
                raise TargetCacheError("release artifact is not a plain file inside the owned target")
        digest = hashlib.sha256()
        with path.open("rb") as stream:
            while chunk := stream.read(1024 * 1024):
                digest.update(chunk)
        return {"path": str(resolved), "sha256": digest.hexdigest()}

    def _run_target_cleanup(self, argv: list[str]) -> bool:
        log_path = self.output_dir / "target-handoff-clean.log"
        try:
            with log_path.open("w", encoding="utf-8") as log:
                result = subprocess.run(
                    argv,
                    cwd=self.root,
                    env=self.env,
                    stdout=log,
                    stderr=subprocess.STDOUT,
                    check=False,
                )
        except OSError as error:
            log_path.write_text(f"workspace dev cleanup could not run: {error}\n", encoding="utf-8")
            self.summary["target_cache"]["cleanup_error"] = str(error)  # type: ignore[index]
            self.summary["target_cache"]["cleanup_log"] = str(log_path.relative_to(self.root))  # type: ignore[index]
            return False
        target_cache = self.summary["target_cache"]
        assert isinstance(target_cache, dict)
        target_cache["cleanup_exit_code"] = result.returncode
        target_cache["cleanup_log"] = str(log_path.relative_to(self.root))
        return result.returncode == 0

    def _check_disk_headroom(self) -> bool:
        """Read-only floor check; never remove caches, logs, source or artifacts."""
        configured = self.env.get("CARGO_TARGET_DIR")
        target = Path(configured) if configured else self.root / "target"
        if not target.is_absolute():
            target = self.root / target
        probes = []
        self.summary["disk_headroom"] = {
            "checked_at": utc_now(),
            "minimum_free_bytes": self.min_free_bytes,
            "target_source": "CARGO_TARGET_DIR" if configured else "workspace default",
            "probes": probes,
        }
        for label, requested in (("workspace", self.root), ("target", target)):
            probe = requested.absolute()
            while not probe.exists() and probe != probe.parent:
                probe = probe.parent
            entry = {"role": label, "requested_path": str(requested), "probed_path": str(probe)}
            probes.append(entry)
            try:
                usage = shutil.disk_usage(probe)
            except OSError as error:
                entry["error"] = str(error)
                self.summary["disk_error"] = f"could not inspect disk headroom for {label}: {error}"
                return False
            entry.update({"total_bytes": usage.total, "free_bytes": usage.free})
            if usage.free < self.min_free_bytes:
                self.summary["disk_error"] = (
                    f"{label} has {usage.free / GIB:.2f} GiB free; "
                    f"checks require the configured floor of {self.min_free_bytes / GIB:.2f} GiB. "
                    "Inspect inactive generated caches manually; preserve active/shared builds, "
                    "source/media and accepted artifacts. No files were removed. See docs/local-ci.md."
                )
                return False
        return True

    def _run_step(self, step: Step, index: int) -> dict[str, object]:
        log_path = self.output_dir / f"{index:02d}-{step.step_id}.log"
        entry: dict[str, object] = {
            "step_id": step.step_id,
            "argv": list(step.argv),
            "status": "running",
            "started_at": utc_now(),
            "ended_at": None,
            "exit_code": None,
            "log": str(log_path.relative_to(self.root)),
            "error": None,
        }
        steps = self.summary["steps"]
        assert isinstance(steps, list)
        steps.append(entry)
        self._write_summary()
        print(f"[{index}/{len(self.steps)}] {step.step_id}: running", flush=True)
        try:
            with log_path.open("w", encoding="utf-8") as log, \
                    terminal_progress(f"checking {step.step_id}", announce=False):
                result = subprocess.run(
                    list(step.argv),
                    cwd=self.root,
                    env=self.env,
                    stdout=log,
                    stderr=subprocess.STDOUT,
                    check=False,
                )
            entry["exit_code"] = result.returncode
            entry["status"] = "passed" if result.returncode == 0 else "failed"
        except FileNotFoundError as error:
            entry["status"] = "failed"
            entry["error"] = f"required executable not found: {error.filename}"
            log_path.write_text(entry["error"] + "\n", encoding="utf-8")
        except InterruptedRun:
            entry["status"] = "interrupted"
            entry["error"] = "pipeline interrupted"
            entry["exit_code"] = 130
            entry["ended_at"] = utc_now()
            with log_path.open("a", encoding="utf-8") as log:
                log.write("\npipeline interrupted\n")
            raise
        except OSError as error:
            entry["status"] = "failed"
            entry["error"] = str(error)
            log_path.write_text(f"command could not run: {error}\n", encoding="utf-8")
        entry["ended_at"] = utc_now()
        self._write_summary()
        print(f"[{index}/{len(self.steps)}] {step.step_id}: {entry['status']}", flush=True)
        return entry

    def run(self) -> int:
        self.output_dir.mkdir(parents=True, exist_ok=False)
        self._write_summary()
        previous_handlers: dict[int, object] = {}

        def interrupt_handler(_signum: int, _frame: object) -> None:
            raise InterruptedRun

        for signum in (signal.SIGINT, signal.SIGTERM):
            previous_handlers[signum] = signal.signal(signum, interrupt_handler)

        failed = False
        try:
            if not self._capture_repository_state("start_tree"):
                self.summary["status"] = "failed"
                self.summary["exit_code"] = 2
                self.summary["metadata_error"] = "could not capture exact commit and dirty state"
            elif any(self._uses_cargo_target(step) for step in self.steps) and not self._resolve_cargo_identity():
                self.summary["status"] = "failed"
                self.summary["exit_code"] = 2
                self.summary["metadata_error"] = "could not resolve exact Cargo workspace and target identity"
                print(self.summary["cargo_metadata_error"], file=sys.stderr, flush=True)
            elif not self._check_disk_headroom():
                self.summary["status"] = "failed"
                self.summary["exit_code"] = 2
                print(self.summary["disk_error"], file=sys.stderr, flush=True)
            else:
                self._write_summary()
                for index, step in enumerate(self.steps, start=1):
                    if self.mode == "full" and self._uses_cargo_target(step) and not self._check_disk_headroom():
                        print(self.summary["disk_error"], file=sys.stderr, flush=True)
                        failed = True
                        break
                    if self._uses_cargo_target(step) and self.target_lock is None:
                        if not self._prepare_target_cache():
                            print(
                                self.summary.get("target_cache_error", "could not prepare Cargo target ownership"),
                                file=sys.stderr,
                                flush=True,
                            )
                            failed = True
                            break
                    result = self._run_step(step, index)
                    if result["status"] != "passed":
                        failed = True
                        break
                self.summary["verification_status"] = "failed" if failed else ("partial" if self.mode == "quick" else "passed")
                self.summary["status"] = self.summary["verification_status"]
                self.summary["exit_code"] = (2 if "disk_error" in self.summary else 1) if failed else 0
        except InterruptedRun:
            self.summary["status"] = "interrupted"
            self.summary["exit_code"] = 130
            steps = self.summary["steps"]
            if isinstance(steps, list):
                for entry in steps:
                    if isinstance(entry, dict) and entry["status"] == "running":
                        entry["status"] = "interrupted"
                        entry["ended_at"] = utc_now()
                        entry["error"] = "pipeline interrupted"
        finally:
            if self.target_lock is not None:
                if self.target_owner_claimed and self.target_owner_path is not None and self.git_identity is not None:
                    try:
                        self._write_target_owner(self.target_owner_path, self.git_identity)
                    except (OSError, TargetCacheError) as error:
                        self.summary["target_owner_update_error"] = str(error)
                        self.summary["status"] = "incomplete"
                        self.summary["exit_code"] = 2
                release_error = self.target_lock.release()
                if release_error is not None:
                    self.summary["target_lock_release_error"] = release_error
                self.target_lock = None
            self.summary["ended_at"] = utc_now()
            self.summary["duration_seconds"] = round(time.monotonic() - self.started_monotonic, 3)
            if self.summary.get("start_tree") is not None:
                if self._capture_repository_state("end_tree"):
                    start_tree = self.summary["start_tree"]
                    end_tree = self.summary["end_tree"]
                    stale = start_tree != end_tree
                    self.summary["stale"] = stale
                    if stale:
                        self.summary["verification_status"] = self.summary.get("status")
                        self.summary["status"] = "stale"
                        self.summary["exit_code"] = 3
                else:
                    self.summary["stale"] = None
                    self.summary["status"] = "incomplete"
                    self.summary["exit_code"] = 2
            steps = self.summary["steps"]
            if isinstance(steps, list):
                next_index = len(steps) + 1
                for index, step in enumerate(self.steps[next_index - 1 :], start=next_index):
                    steps.append(
                        {
                            "step_id": step.step_id,
                            "argv": list(step.argv),
                            "status": "not_run",
                            "started_at": None,
                            "ended_at": None,
                            "exit_code": None,
                            "log": None,
                            "error": "pipeline stopped before this step",
                        }
                    )
            self._write_summary()
            for signum, handler in previous_handlers.items():
                signal.signal(signum, handler)
        return int(self.summary["exit_code"] if self.summary["exit_code"] is not None else 2)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--quick",
        action="store_true",
        help="run architecture/UI/GPUI guards and formatting only; this is not full CI",
    )
    parser.add_argument(
        "--min-free-gib", type=int,
        help="read-only free-space floor in GiB; default 2 for full and 0 for quick; never deletes files",
    )
    parser.add_argument(
        "--adopt-target",
        action="store_true",
        help="assert sole ownership of an unowned Cargo target and clean only this workspace's dev artifacts",
    )
    parser.add_argument("--prepare-changelog", action="store_true",
                        help="generate notes before freezing source, then exit without running sealed checks")
    parser.add_argument("--changelog-base", help="explicit first-generation Git baseline")
    parser.add_argument("--changelog-through", default="HEAD", help="immutable generation range end")
    arguments = parser.parse_args(argv)
    if arguments.prepare_changelog:
        command = [sys.executable, str(ROOT / "scripts/changelog.py"), "prepare",
                   "--source", arguments.changelog_through]
        if arguments.changelog_base:
            command.extend(["--base", arguments.changelog_base])
        return subprocess.run(command, cwd=ROOT).returncode
    if arguments.changelog_base or arguments.changelog_through != "HEAD":
        parser.error("changelog range arguments require --prepare-changelog")
    if arguments.min_free_gib is not None and not 0 <= arguments.min_free_gib <= sys.maxsize // GIB:
        parser.error("--min-free-gib must be a bounded nonnegative integer")
    mode = "quick" if arguments.quick else "full"
    runner = LocalCiRunner(
        mode=mode,
        min_free_bytes=None if arguments.min_free_gib is None else arguments.min_free_gib * GIB,
        adopt_target=arguments.adopt_target,
    )
    try:
        exit_code = runner.run()
    except FileExistsError:
        print(f"run output already exists: {runner.output_dir}", file=sys.stderr)
        return 2
    except OSError as error:
        print(f"local ci could not retain its complete report at {runner.summary_path}: {error}", file=sys.stderr)
        return 2
    print(f"local ci {runner.summary['status']}: {runner.summary_path}")
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
