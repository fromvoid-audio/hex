#!/usr/bin/env python3
"""Serialize participating Cargo-heavy commands across local worktrees.

The lock is advisory: commands that do not use this wrapper are not excluded.
The child inherits the locked descriptor so an abruptly killed wrapper cannot
release the slot while that child still holds the descriptor.
"""

import argparse
from contextlib import contextmanager
import fcntl
import math
import os
from pathlib import Path
import stat
import subprocess
import sys
import time


class SlotWaitExpired(Exception):
    pass


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


def acquire(fd, path, deadline):
    if deadline is None:
        fcntl.flock(fd, fcntl.LOCK_EX)
    else:
        while True:
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise SlotWaitExpired(path) from None
                time.sleep(min(0.05, remaining))
                if time.monotonic() >= deadline:
                    raise SlotWaitExpired(path) from None
            else:
                break
    opened = os.fstat(fd)
    current = os.lstat(path)
    if (opened.st_dev, opened.st_ino) != (current.st_dev, current.st_ino):
        raise OSError(f"cargo slot file changed while waiting: {path}")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--wait-seconds", type=float, metavar="SECONDS",
        help="maximum time waiting for the slot; zero tries immediately",
    )
    parser.add_argument("command", nargs=argparse.REMAINDER)
    options = parser.parse_args()
    command = options.command
    if command[:1] == ["--"]:
        command = command[1:]
    if not command:
        parser.error("a command is required")

    budget = options.wait_seconds
    if budget is not None and (not math.isfinite(budget) or budget < 0):
        parser.error("--wait-seconds requires a finite nonnegative number")
    deadline = None if budget is None else time.monotonic() + budget
    path = Path(os.environ.get("HEX_CARGO_SLOT_FILE", Path.home() / ".cache/hex/cargo-slot.lock"))
    path.parent.mkdir(parents=True, exist_ok=True)
    admission = path.with_name(path.name + ".admission")
    try:
        with lock_file(path) as fd:
            print(f"waiting for cargo slot: {path}", file=sys.stderr, flush=True)
            # Only the reserved waiter competes for execution. Release admission
            # before starting the child so another waiter can reserve its turn.
            with lock_file(admission) as reservation:
                acquire(reservation, admission, deadline)
                print(f"next cargo slot reserved: {path}", file=sys.stderr, flush=True)
                acquire(fd, path, deadline)
            print(f"cargo slot acquired: {path}", file=sys.stderr, flush=True)
            result = subprocess.run(command, pass_fds=(fd,), check=False)
            return result.returncode if result.returncode >= 0 else 128 - result.returncode
    except SlotWaitExpired as error:
        print(f"timed out waiting for cargo slot after {budget:g}s: {error}",
              file=sys.stderr)
        return 75


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
    except (OSError, ValueError) as error:
        print(f"cargo slot: {error}", file=sys.stderr)
        sys.exit(1)
