#!/usr/bin/env python3
"""Serialize participating Cargo-heavy commands across local worktrees.

The lock is advisory: commands that do not use this wrapper are not excluded.
The child inherits the locked descriptor so an abruptly killed wrapper cannot
release the slot while that child still holds the descriptor.
"""

import fcntl
import os
from pathlib import Path
import stat
import subprocess
import sys


def main():
    if len(sys.argv) < 2:
        print("usage: python3 scripts/cargo-slot.py COMMAND [ARG ...]", file=sys.stderr)
        return 2

    path = Path(os.environ.get("HEX_CARGO_SLOT_FILE", Path.home() / ".cache/hex/cargo-slot.lock"))
    path.parent.mkdir(parents=True, exist_ok=True)
    flags = os.O_CREAT | os.O_RDWR | getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(path, flags, 0o600)
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            raise OSError("cargo slot is not a regular file")
        print(f"waiting for cargo slot: {path}", file=sys.stderr, flush=True)
        fcntl.flock(fd, fcntl.LOCK_EX)
        opened = os.fstat(fd)
        current = os.lstat(path)
        if (opened.st_dev, opened.st_ino) != (current.st_dev, current.st_ino):
            raise OSError("cargo slot file changed while waiting")
        print(f"cargo slot acquired: {path}", file=sys.stderr, flush=True)
        result = subprocess.run(sys.argv[1:], pass_fds=(fd,), check=False)
        return result.returncode if result.returncode >= 0 else 128 - result.returncode
    finally:
        os.close(fd)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
    except (OSError, ValueError) as error:
        print(f"cargo slot: {error}", file=sys.stderr)
        sys.exit(1)
