#!/usr/bin/env python3
"""Run Hex's local validation pipeline and retain a machine-readable report."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
import uuid


ROOT = Path(__file__).resolve().parent.parent


@dataclass(frozen=True)
class Step:
    step_id: str
    argv: tuple[str, ...]


QUICK_STEPS = (
    Step("architecture", ("python3", "scripts/check-architecture.py")),
    Step("gpui-patch", ("python3", "scripts/check-gpui-patch.py")),
    Step("ui-style", ("python3", "scripts/check-ui-style.py")),
    Step("format", ("cargo", "fmt", "--all", "--check")),
)

FULL_STEPS = (
    Step(
        "runner-tests",
        ("python3", "-m", "unittest", "discover", "-s", "scripts/tests", "-p", "test_local_ci.py"),
    ),
    *QUICK_STEPS[:3],
    QUICK_STEPS[3],
    Step("workspace-tests", ("cargo", "test", "--workspace", "--locked")),
    Step("ui-feature-tests", ("cargo", "test", "-p", "hex-ui", "--features", "ui-test-support", "--locked")),
    Step("audio-allocation-probe", ("cargo", "test", "-p", "hex-audio", "--features", "allocation-probe", "--locked")),
    Step("stream-allocation-probe", ("cargo", "test", "-p", "hex-stream", "--features", "allocation-probe", "--locked")),
    Step("workspace-clippy", ("cargo", "clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings")),
    Step("ui-feature-clippy", ("cargo", "clippy", "-p", "hex-ui", "--all-targets", "--features", "ui-test-support", "--locked", "--", "-D", "warnings")),
    Step("audio-feature-clippy", ("cargo", "clippy", "-p", "hex-audio", "--all-targets", "--features", "allocation-probe", "--locked", "--", "-D", "warnings")),
    Step("stream-feature-clippy", ("cargo", "clippy", "-p", "hex-stream", "--all-targets", "--features", "allocation-probe", "--locked", "--", "-D", "warnings")),
    Step("desktop-build", ("cargo", "build", "-p", "hex-desktop", "--locked")),
)


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def command_output(argv: list[str], cwd: Path, env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(argv, cwd=cwd, env=env, capture_output=True, text=True, check=False)


class InterruptedRun(Exception):
    """Raised when a user or supervisor interrupts the pipeline."""


class LocalCiRunner:
    def __init__(
        self,
        root: Path = ROOT,
        *,
        mode: str = "full",
        run_id: str | None = None,
        env: dict[str, str] | None = None,
        steps: tuple[Step, ...] | None = None,
    ) -> None:
        self.root = root.resolve()
        self.mode = mode
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

    def _write_summary(self) -> None:
        temporary = self.summary_path.with_suffix(".json.tmp")
        temporary.write_text(json.dumps(self.summary, indent=2, sort_keys=True) + "\n")
        temporary.replace(self.summary_path)

    def _capture_repository_state(self, key: str) -> bool:
        try:
            commit = command_output(["git", "rev-parse", "HEAD"], self.root, self.env)
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
        if commit.returncode != 0 or status.returncode != 0 or tracked.returncode != 0:
            self.summary["repository_error"] = {
                "commit_exit_code": commit.returncode,
                "status_exit_code": status.returncode,
                "files_exit_code": tracked.returncode,
                "commit_stderr": commit.stderr.strip(),
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
            self.summary["commit"] = snapshot["commit"]
            self.summary["dirty"] = snapshot["dirty"]
            self.summary["dirty_files"] = snapshot["dirty_files"]
            self.summary["repository_commands"] = [
                ["git", "rev-parse", "HEAD"],
                ["git", "status", "--porcelain=v1", "--untracked-files=all"],
                ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
            ]
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
            with log_path.open("w", encoding="utf-8") as log:
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
            else:
                self._write_summary()
                for index, step in enumerate(self.steps, start=1):
                    result = self._run_step(step, index)
                    if result["status"] != "passed":
                        failed = True
                        break
                self.summary["verification_status"] = "failed" if failed else ("partial" if self.mode == "quick" else "passed")
                self.summary["status"] = self.summary["verification_status"]
                self.summary["exit_code"] = 1 if failed else 0
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
    arguments = parser.parse_args(argv)
    mode = "quick" if arguments.quick else "full"
    runner = LocalCiRunner(mode=mode)
    try:
        exit_code = runner.run()
    except FileExistsError:
        print(f"run output already exists: {runner.output_dir}", file=sys.stderr)
        return 2
    print(f"local ci {runner.summary['status']}: {runner.summary_path}")
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
