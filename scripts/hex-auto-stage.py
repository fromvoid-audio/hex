#!/usr/bin/env python3
"""Idle-aware local Mac post-merge auto-stager for Hex next-launch updates.

Checks whether Hex is running (via the cooperative `.hex-test-build.lock` lease
and process inspection) and refuses to build while Hex is active so background
compilation never competes with live audio. When `origin/main` advances past
the last staged/installed commit, checks out a clean detached worktree, runs
local CI, and stages a verified `.hex-pending-update` bundle via
`scripts/bundle-macos.sh --prepare-only`.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import fcntl
import json
import os
from pathlib import Path
import plistlib
import re
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
LAUNCHD_LABEL = "org.portalsurfer.hex.auto-stage"
DEFAULT_REMOTE = "origin"
DEFAULT_BRANCH = "main"
SHA40_RE = re.compile(r"^[0-9a-f]{40}$")


def utc_now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def default_state_file() -> Path:
    return (
        Path.home()
        / "Library"
        / "Application Support"
        / "hex"
        / "auto-stage-state.json"
    )


def default_app_path(repo_root: Path = ROOT) -> Path:
    config_path = repo_root / "scripts" / "hex-test-build.json"
    config = json.loads(config_path.read_text(encoding="utf-8"))
    configured = Path(os.environ.get("HEX_DEV_APP_PATH", config["app_path"]))
    if configured.is_absolute():
        return configured
    common = git_common_dir(repo_root)
    return (common.parent / configured).resolve()


def git_common_dir(repo_root: Path) -> Path:
    out = subprocess.run(
        ["git", "rev-parse", "--git-common-dir"],
        cwd=repo_root,
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    path = Path(out)
    return path if path.is_absolute() else (repo_root / path).resolve()


def is_lease_held(app_path: Path) -> bool:
    """Return True if `.hex-test-build.lock` is currently locked by Hex or a build."""
    lock_path = app_path.parent / ".hex-test-build.lock"
    if not lock_path.exists():
        return False
    if lock_path.is_symlink() or not lock_path.is_file():
        # Treat invalid/unexpected lock metadata conservatively as busy.
        return True
    try:
        with lock_path.open("r+") as handle:
            try:
                fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                return True
            else:
                fcntl.flock(handle.fileno(), fcntl.LOCK_UN)
                return False
    except OSError:
        return True


def find_running_hex_pids(
    app_path: Path,
    *,
    ps_runner=None,
) -> list[int]:
    """Return PIDs of any running `hex.app` or raw `hex-desktop` processes."""
    if ps_runner is None:
        result = subprocess.run(
            ["ps", "-ww", "-axo", "pid=,comm="],
            check=True,
            capture_output=True,
            text=True,
        )
        stdout = result.stdout
    else:
        stdout = ps_runner()

    prefix = str(app_path.absolute()).casefold() + "/"
    pids: list[int] = []
    for line in stdout.splitlines():
        fields = line.strip().split(None, 1)
        if len(fields) != 2:
            continue
        pid_str, comm = fields
        try:
            pid = int(pid_str)
        except ValueError:
            continue
        comm_path = Path(comm.rstrip("/"))
        if (
            str(comm_path.absolute()).casefold().startswith(prefix)
            or comm_path.name.casefold() == "hex-desktop"
        ):
            pids.append(pid)
    return pids


def check_hex_idle(app_path: Path, *, ps_runner=None) -> tuple[bool, str]:
    """Verify neither the bundle lease nor any Hex process is active."""
    if is_lease_held(app_path):
        return False, "hex update lease (.hex-test-build.lock) is currently held"
    pids = find_running_hex_pids(app_path, ps_runner=ps_runner)
    if pids:
        joined = ", ".join(str(p) for p in pids)
        return False, f"hex is running (pid {joined})"
    return True, "idle"


def read_receipt_source(app_path: Path) -> str | None:
    receipt = app_path / "Contents" / "Resources" / "test-build.json"
    if not receipt.is_file() or receipt.is_symlink():
        return None
    try:
        data = json.loads(receipt.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    source = data.get("source")
    if isinstance(source, str) and SHA40_RE.match(source):
        return source
    return None


def load_state(path: Path) -> dict:
    if not path.is_file():
        return {}
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):
        return {}


def save_state(path: Path, state: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp_name = tempfile.mkstemp(prefix=".auto-stage.", suffix=".tmp", dir=str(path.parent))
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            json.dump(state, stream, indent=2, sort_keys=True)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(tmp_name, path)
    except Exception:
        try:
            os.unlink(tmp_name)
        except OSError:
            pass
        raise


def resolve_remote_sha(
    repo_root: Path,
    remote: str,
    branch: str,
    *,
    fetch: bool = True,
    git_runner=None,
) -> str:
    def _run(argv: list[str]) -> str:
        if git_runner is not None:
            return git_runner(argv)
        return subprocess.run(
            argv,
            cwd=repo_root,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()

    if fetch:
        _run(["git", "fetch", "--quiet", remote, branch])
    sha = _run(["git", "rev-parse", f"{remote}/{branch}"])
    if not SHA40_RE.match(sha):
        raise RuntimeError(f"unexpected commit sha for {remote}/{branch}: {sha!r}")
    return sha


def render_launchd_plist(
    script_path: Path,
    *,
    repo_root: Path = ROOT,
    interval_secs: int = 900,
    log_dir: Path | None = None,
) -> bytes:
    if log_dir is None:
        log_dir = Path.home() / "Library" / "Logs" / "hex"
    payload = {
        "Label": LAUNCHD_LABEL,
        "ProgramArguments": [
            "/usr/bin/python3",
            str(script_path.resolve()),
            "--repo",
            str(repo_root.resolve()),
        ],
        "StartInterval": int(interval_secs),
        "RunAtLoad": False,
        "LowPriorityIO": True,
        "Nice": 10,
        "StandardOutPath": str(log_dir / "auto-stage.stdout.log"),
        "StandardErrorPath": str(log_dir / "auto-stage.stderr.log"),
        "EnvironmentVariables": {
            "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",
        },
    }
    return plistlib.dumps(payload, fmt=plistlib.FMT_XML)


def prepare_clean_worktree(
    repo_root: Path,
    worktree_dir: Path,
    target_sha: str,
    *,
    cmd_runner=None,
) -> None:
    def _run(argv: list[str], cwd: Path) -> None:
        if cmd_runner is not None:
            cmd_runner(argv, cwd)
        else:
            subprocess.run(argv, cwd=cwd, check=True)

    if worktree_dir.exists():
        _run(["git", "checkout", "--detach", "--force", target_sha], worktree_dir)
        _run(["git", "clean", "-fdx"], worktree_dir)
    else:
        worktree_dir.parent.mkdir(parents=True, exist_ok=True)
        _run(
            ["git", "worktree", "add", "--detach", str(worktree_dir), target_sha],
            repo_root,
        )


def collect_included_prs(
    worktree_dir: Path,
    target_sha: str,
    *,
    git_runner=None,
) -> list[dict[str, str]]:
    """Extract PR metadata from the commit message, or fall back to commit link."""
    def _run(argv: list[str]) -> str:
        if git_runner is not None:
            return git_runner(argv)
        return subprocess.run(
            argv,
            cwd=worktree_dir,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()

    subject = _run(["git", "log", "-1", "--format=%s", target_sha])
    match = re.search(r"\(#(\d+)\)", subject)
    pr_number = match.group(1) if match else "0"
    return [
        {
            "pr": f"https://github.com/PORTALSURFER/hex/pull/{pr_number}",
            "head": target_sha,
        }
    ]


def stage_update(
    *,
    repo_root: Path,
    app_path: Path,
    state_file: Path,
    worktree_dir: Path,
    remote: str = DEFAULT_REMOTE,
    branch: str = DEFAULT_BRANCH,
    fetch: bool = True,
    dry_run: bool = False,
    skip_ci: bool = False,
    ci_report: Path | None = None,
    ps_runner=None,
    git_runner=None,
    cmd_runner=None,
) -> dict:
    idle, reason = check_hex_idle(app_path, ps_runner=ps_runner)
    if not idle:
        return {
            "action": "skipped_running",
            "reason": reason,
            "app_path": str(app_path),
        }

    target_sha = resolve_remote_sha(
        repo_root, remote, branch, fetch=fetch, git_runner=git_runner
    )
    state = load_state(state_file)
    last_staged = state.get("last_staged_sha")
    installed_sha = read_receipt_source(app_path)
    pending_sha = read_receipt_source(app_path.parent / ".hex-pending-update" / "hex.app")

    if target_sha in {last_staged, installed_sha, pending_sha}:
        return {
            "action": "up_to_date",
            "target_sha": target_sha,
            "last_staged_sha": last_staged,
            "installed_sha": installed_sha,
            "pending_sha": pending_sha,
        }

    ci_cmd = (
        None
        if skip_ci
        else [sys.executable, str(worktree_dir / "scripts" / "local-ci.py"), "--profile", "full"]
    )
    bundle_cmd_preview = [
        str(worktree_dir / "scripts" / "bundle-macos.sh"),
        "--prepare-only",
        "--candidate",
        target_sha,
    ]
    if dry_run:
        return {
            "action": "dry_run_plan",
            "target_sha": target_sha,
            "worktree_dir": str(worktree_dir),
            "app_path": str(app_path),
            "ci_command": ci_cmd,
            "bundle_command": bundle_cmd_preview,
        }

    prepare_clean_worktree(repo_root, worktree_dir, target_sha, cmd_runner=cmd_runner)

    # Re-check Hex is still idle before starting heavy CI or bundling.
    idle, reason = check_hex_idle(app_path, ps_runner=ps_runner)
    if not idle:
        return {
            "action": "aborted_hex_started",
            "reason": reason,
            "target_sha": target_sha,
        }

    def _run_cmd(argv: list[str], cwd: Path) -> None:
        if cmd_runner is not None:
            cmd_runner(argv, cwd)
        else:
            subprocess.run(argv, cwd=cwd, check=True)

    resolved_ci_report = ci_report
    if not skip_ci:
        assert ci_cmd is not None
        _run_cmd(ci_cmd, worktree_dir)
        if resolved_ci_report is None:
            resolved_ci_report = (
                worktree_dir / "artifacts" / "local-ci" / "latest-summary.json"
            )
    elif resolved_ci_report is None:
        raise RuntimeError("--skip-ci requires --ci-report when not using --dry-run")

    # Re-check idle immediately before final optimized bundle preparation.
    idle, reason = check_hex_idle(app_path, ps_runner=ps_runner)
    if not idle:
        return {
            "action": "aborted_hex_started",
            "reason": reason,
            "target_sha": target_sha,
        }

    with tempfile.TemporaryDirectory(prefix="hex-auto-stage-") as meta_dir:
        meta_path = Path(meta_dir)
        included_path = meta_path / "included.json"
        checklist_path = meta_path / "checklist.md"
        included_path.write_text(
            json.dumps(
                collect_included_prs(worktree_dir, target_sha, git_runner=git_runner),
                indent=2,
            )
            + "\n",
            encoding="utf-8",
        )
        checklist_path.write_text(
            f"- Launch hex and confirm version matches `{target_sha[:8]}`.\n",
            encoding="utf-8",
        )
        bundle_cmd = [
            str(worktree_dir / "scripts" / "bundle-macos.sh"),
            "--prepare-only",
            "--ci-report",
            str(resolved_ci_report),
            "--candidate",
            target_sha,
            "--included",
            str(included_path),
            "--checklist",
            str(checklist_path),
        ]
        _run_cmd(bundle_cmd, worktree_dir)

    next_state = {
        **state,
        "last_staged_sha": target_sha,
        "last_attempted_sha": target_sha,
        "last_status": "staged",
        "updated_at": utc_now(),
    }
    save_state(state_file, next_state)
    return {
        "action": "staged",
        "target_sha": target_sha,
        "app_path": str(app_path),
        "state_file": str(state_file),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=ROOT, help="Hex git repository root")
    parser.add_argument("--app-path", type=Path, default=None, help="Installed hex.app path")
    parser.add_argument("--state-file", type=Path, default=default_state_file())
    parser.add_argument("--worktree-dir", type=Path, default=None)
    parser.add_argument("--remote", default=DEFAULT_REMOTE)
    parser.add_argument("--branch", default=DEFAULT_BRANCH)
    parser.add_argument("--no-fetch", action="store_true", help="Skip git fetch before checking ref")
    parser.add_argument("--dry-run", action="store_true", help="Check readiness and print plan only")
    parser.add_argument("--skip-ci", action="store_true", help="Use an explicit --ci-report instead of running local-ci.py")
    parser.add_argument("--ci-report", type=Path, default=None)
    parser.add_argument("--print-launchd-plist", action="store_true", help="Print macOS LaunchAgent plist XML and exit")
    parser.add_argument("--interval-secs", type=int, default=900, help="LaunchAgent StartInterval seconds")
    args = parser.parse_args(argv)

    if args.print_launchd_plist:
        plist_bytes = render_launchd_plist(
            Path(__file__),
            repo_root=args.repo,
            interval_secs=args.interval_secs,
        )
        sys.stdout.write(plist_bytes.decode("utf-8"))
        return 0

    app_path = args.app_path if args.app_path is not None else default_app_path(args.repo)
    worktree_dir = (
        args.worktree_dir
        if args.worktree_dir is not None
        else app_path.parent / ".hex-auto-stage-worktree"
    )
    result = stage_update(
        repo_root=args.repo,
        app_path=app_path,
        state_file=args.state_file,
        worktree_dir=worktree_dir,
        remote=args.remote,
        branch=args.branch,
        fetch=not args.no_fetch,
        dry_run=args.dry_run,
        skip_ci=args.skip_ci,
        ci_report=args.ci_report,
    )
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
