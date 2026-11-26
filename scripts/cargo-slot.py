#!/usr/bin/env python3
"""Serialize participating Cargo-heavy commands across local worktrees.

The lock is advisory: commands that do not use this wrapper are not excluded.
The child inherits the locked descriptor so an abruptly killed wrapper cannot
release the slot while that child still holds the descriptor.

When holding the execution lock, the wrapper writes a non-authoritative
`<slot>.holder.json` sidecar (PID, child PID, cwd, command, start time, wait
duration) so contending callers and `--status` can explain prolonged waits
without disrupting or killing active work. Lock ownership is always determined
solely by `flock`, never by the sidecar file.
"""

import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import errno
import fcntl
import json
import math
import os
from pathlib import Path
import shlex
import stat
import subprocess
import sys
import tempfile
import time


class SlotWaitExpired(Exception):
    pass


def default_slot_path():
    return Path(os.environ.get("HEX_CARGO_SLOT_FILE", Path.home() / ".cache/hex/cargo-slot.lock"))


def holder_metadata_path(slot_path):
    return slot_path.with_name(slot_path.name + ".holder.json")


@contextmanager
def lock_file(path):
    flags = os.O_CREAT | os.O_RDWR | getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(path, flags, 0o600)
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            raise OSError(f"cargo slot lock is not a regular file: {path}")
        yield fd
    finally:
        os.close(fd)


def probe_locked(path):
    """Non-disruptively check whether an existing lock file is currently held.

    Returns None when the lock file does not exist, True when currently locked
    by another process, or False when unlocked. Never creates or removes files.
    """
    flags = os.O_RDWR | getattr(os, "O_NOFOLLOW", 0)
    try:
        fd = os.open(path, flags)
    except FileNotFoundError:
        return None
    except OSError as error:
        if error.errno == errno.ELOOP:
            raise OSError(f"cargo slot lock is a symbolic link: {path}") from None
        raise
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            raise OSError(f"cargo slot lock is not a regular file: {path}")
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return True
        else:
            fcntl.flock(fd, fcntl.LOCK_UN)
            return False
    finally:
        os.close(fd)


def read_holder_metadata(slot_path):
    """Best-effort read of non-authoritative holder metadata; never raises."""
    meta_path = holder_metadata_path(slot_path)
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    try:
        fd = os.open(meta_path, flags)
    except OSError:
        return None
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_size > 64 * 1024:
            return None
        with os.fdopen(fd, "rb", closefd=False) as handle:
            raw = handle.read(64 * 1024 + 1)
        if len(raw) > 64 * 1024:
            return None
        parsed = json.loads(raw.decode("utf-8"))
        return parsed if isinstance(parsed, dict) else None
    except (OSError, UnicodeDecodeError, ValueError):
        return None
    finally:
        os.close(fd)


def write_holder_metadata(slot_path, metadata):
    """Atomically publish holder metadata while holding the execution lock."""
    meta_path = holder_metadata_path(slot_path)
    try:
        if meta_path.is_symlink():
            return
        payload = (json.dumps(metadata, sort_keys=True) + "\n").encode("utf-8")
        fd, tmp_name = tempfile.mkstemp(prefix=".cargo-slot-holder-", dir=meta_path.parent)
        try:
            os.fchmod(fd, 0o600)
            with os.fdopen(fd, "wb") as handle:
                handle.write(payload)
                handle.flush()
            os.replace(tmp_name, meta_path)
        except OSError:
            try:
                os.unlink(tmp_name)
            except OSError:
                pass
    except OSError:
        pass


def clear_holder_metadata(slot_path, owner_pid):
    """Remove holder metadata on normal release if it still belongs to this PID."""
    meta = read_holder_metadata(slot_path)
    if not meta or meta.get("pid") != owner_pid:
        return
    meta_path = holder_metadata_path(slot_path)
    try:
        if not meta_path.is_symlink():
            os.unlink(meta_path)
    except OSError:
        pass


def format_holder_summary(metadata, now_epoch=None):
    if not metadata:
        return "holder metadata unavailable"
    now = time.time() if now_epoch is None else now_epoch
    pid = metadata.get("pid")
    child_pid = metadata.get("child_pid")
    cwd = metadata.get("cwd", "<unknown>")
    argv = metadata.get("argv")
    if isinstance(argv, list) and argv:
        cmd = " ".join(shlex.quote(str(arg)) for arg in argv)
        if len(cmd) > 160:
            cmd = cmd[:157] + "..."
    else:
        cmd = "<unknown>"
    acquired_epoch = metadata.get("acquired_epoch_s")
    if isinstance(acquired_epoch, (int, float)) and math.isfinite(acquired_epoch):
        held = max(0.0, now - acquired_epoch)
        held_text = f"{held:.1f}s"
    else:
        held_text = "unknown"
    child_text = str(child_pid) if child_pid is not None else "starting"
    return f"pid={pid} child_pid={child_text} held={held_text} cwd={cwd} cmd={cmd}"


def inspect_slot_status(slot_path):
    """Read-only inspection of the execution lock, admission queue, and holder."""
    admission_path = slot_path.with_name(slot_path.name + ".admission")
    execution_locked = probe_locked(slot_path)
    admission_locked = probe_locked(admission_path)
    holder = read_holder_metadata(slot_path)
    now_epoch = time.time()
    held_seconds = None
    if (
        execution_locked is True
        and holder is not None
        and isinstance(holder.get("acquired_epoch_s"), (int, float))
        and math.isfinite(holder["acquired_epoch_s"])
    ):
        held_seconds = round(max(0.0, now_epoch - holder["acquired_epoch_s"]), 3)
    return {
        "slot_path": str(slot_path),
        "state": "occupied" if execution_locked else "idle",
        "lock_file_present": execution_locked is not None,
        "admission_reserved": bool(admission_locked),
        "held_seconds": held_seconds,
        "holder": holder if execution_locked else None,
        "stale_holder_metadata": holder if (not execution_locked and holder is not None) else None,
    }


def acquire(fd, path, deadline, on_contended=None):
    contended_reported = False
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        if on_contended is not None:
            on_contended()
            contended_reported = True
        if deadline is None:
            fcntl.flock(fd, fcntl.LOCK_EX)
        else:
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise SlotWaitExpired(path) from None
                time.sleep(min(0.05, remaining))
                try:
                    fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except BlockingIOError:
                    if time.monotonic() >= deadline:
                        raise SlotWaitExpired(path) from None
                else:
                    break
    opened = os.fstat(fd)
    current = os.lstat(path)
    if (opened.st_dev, opened.st_ino) != (current.st_dev, current.st_ino):
        raise OSError(f"cargo slot file changed while waiting: {path}")
    return contended_reported


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--wait-seconds",
        type=float,
        metavar="SECONDS",
        help="maximum time waiting for the slot; zero tries immediately",
    )
    parser.add_argument(
        "--status",
        action="store_true",
        help="inspect current slot occupancy and holder metadata without acquiring the lock",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="with --status, emit machine-readable JSON",
    )
    parser.add_argument("command", nargs=argparse.REMAINDER)
    options = parser.parse_args()
    path = default_slot_path()

    if options.status:
        if options.command and options.command != ["--"]:
            parser.error("--status does not accept a command")
        status = inspect_slot_status(path)
        if options.json:
            print(json.dumps(status, sort_keys=True))
        else:
            state = status["state"]
            if state == "occupied":
                summary = format_holder_summary(status["holder"])
                print(
                    f"cargo slot occupied: {path} "
                    f"(admission_reserved={str(status['admission_reserved']).lower()}) "
                    f"{summary}"
                )
            else:
                extra = " (lock file absent)" if not status["lock_file_present"] else ""
                if status["stale_holder_metadata"] is not None:
                    extra += " (stale holder metadata present; not removed)"
                print(f"cargo slot idle: {path}{extra}")
        return 0

    if options.json:
        parser.error("--json is only valid with --status")

    command = options.command
    if command[:1] == ["--"]:
        command = command[1:]
    if not command:
        parser.error("a command is required")

    budget = options.wait_seconds
    if budget is not None and (not math.isfinite(budget) or budget < 0):
        parser.error("--wait-seconds requires a finite nonnegative number")
    wait_started = time.monotonic()
    deadline = None if budget is None else wait_started + budget
    path.parent.mkdir(parents=True, exist_ok=True)
    admission = path.with_name(path.name + ".admission")

    def report_contention(stage):
        holder = read_holder_metadata(path)
        summary = format_holder_summary(holder)
        print(
            f"cargo slot busy ({stage}): {path} ({summary})",
            file=sys.stderr,
            flush=True,
        )

    try:
        with lock_file(path) as fd:
            print(f"waiting for cargo slot: {path}", file=sys.stderr, flush=True)
            # Only the reserved waiter competes for execution. Release admission
            # before starting the child so another waiter can reserve its turn.
            with lock_file(admission) as reservation:
                acquire(
                    reservation,
                    admission,
                    deadline,
                    on_contended=lambda: report_contention("admission"),
                )
                print(f"next cargo slot reserved: {path}", file=sys.stderr, flush=True)
                acquire(
                    fd,
                    path,
                    deadline,
                    on_contended=lambda: report_contention("execution"),
                )
            wait_elapsed = max(0.0, time.monotonic() - wait_started)
            print(
                f"cargo slot acquired: {path} (waited {wait_elapsed:.3f}s)",
                file=sys.stderr,
                flush=True,
            )
            now_epoch = time.time()
            metadata = {
                "pid": os.getpid(),
                "child_pid": None,
                "ppid": os.getppid(),
                "cwd": os.getcwd(),
                "argv": command,
                "acquired_at_utc": datetime.fromtimestamp(now_epoch, timezone.utc).strftime(
                    "%Y-%m-%dT%H:%M:%SZ"
                ),
                "acquired_epoch_s": round(now_epoch, 3),
                "wait_seconds": round(wait_elapsed, 3),
            }
            write_holder_metadata(path, metadata)
            try:
                child = subprocess.Popen(command, pass_fds=(fd,))
                metadata["child_pid"] = child.pid
                write_holder_metadata(path, metadata)
                returncode = child.wait()
            finally:
                clear_holder_metadata(path, os.getpid())
            return returncode if returncode >= 0 else 128 - returncode
    except SlotWaitExpired as error:
        holder = read_holder_metadata(path)
        summary = format_holder_summary(holder)
        print(
            f"timed out waiting for cargo slot after {budget:g}s: {error} ({summary})",
            file=sys.stderr,
        )
        return 75


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
    except (OSError, ValueError) as error:
        print(f"cargo slot: {error}", file=sys.stderr)
        sys.exit(1)
