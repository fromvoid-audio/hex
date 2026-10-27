"""Observe an explicitly approved parent exit before deferred publication.

The pipe is inherited from the application, not a persistent command channel.
A readiness response means registration is complete, not that exit is approved.
Missing approval, timeout, process replacement and abnormal exit all refuse.
This module does not publish, inspect projects, or restart an application.
"""
from __future__ import annotations

from contextlib import closing
import os
import select
import stat
import subprocess
import time

APPROVAL = b"hex-update-approved-v1\n"
READY = b"hex-update-observer-ready-v1\n"
# Darwin sys/event.h: report wait status for a process the observer may signal.
NOTE_EXITSTATUS = 0x04000000


class ExitObservationError(Exception):
    """The approved normal exit could not be established."""


def observe_parent_exit(parent_pid: int, approval_fd: int, ready_fd: int,
                        parent_executable, *, timeout: float = 60.0) -> None:
    """Wait for an approval and this exact parent's zero-status exit.

    The caller must keep both pipes private, retain the selected candidate, and
    independently recheck target, candidate, signature, and publication lock.
    Spawn only after any save and user confirmation; the deadline begins here.
    The bounded timeout is refusal, never consent. Only the application sends
    APPROVAL, after its ordinary shutdown path has completed successfully. The
    path check compares current filesystem objects, not the mapped executable
    image; the parent must also hold validated startup custody.
    """
    if (type(parent_pid) is not int or parent_pid <= 1 or os.getppid() != parent_pid
            or not 0 < timeout <= 3600):
        raise ExitObservationError("invalid parent or observation timeout")
    if not hasattr(select, "kqueue"):
        raise ExitObservationError("parent exit observation requires macOS kqueue")
    deadline = time.monotonic() + timeout
    approval = bytearray()
    closed = False
    exited = False
    with closing(select.kqueue()) as queue:
        events = [
            select.kevent(parent_pid, filter=select.KQ_FILTER_PROC,
                          flags=select.KQ_EV_ADD | select.KQ_EV_ENABLE | select.KQ_EV_ONESHOT,
                          fflags=select.KQ_NOTE_EXIT | NOTE_EXITSTATUS),
            select.kevent(approval_fd, filter=select.KQ_FILTER_READ,
                          flags=select.KQ_EV_ADD | select.KQ_EV_ENABLE),
        ]
        try:
            queue.control(events, 0, 0)
            # kqueue pins the process before consulting its path, so a reused
            # PID cannot become the observed process during the check. `ps` and
            # stat compare current paths, not the image mapped into the process.
            result = subprocess.run(["/bin/ps", "-ww", "-p", str(parent_pid), "-o", "comm="],
                                    capture_output=True, text=True, timeout=5, check=True)
            paths = result.stdout.splitlines()
            if len(paths) != 1 or not paths[0].strip().startswith("/"):
                raise ExitObservationError("parent executable is unavailable")
            expected = os.stat(parent_executable)
            actual = os.stat(paths[0].strip())
            if (not stat.S_ISREG(expected.st_mode) or not stat.S_ISREG(actual.st_mode)
                    or (expected.st_dev, expected.st_ino) != (actual.st_dev, actual.st_ino)):
                raise ExitObservationError("parent executable does not match target")
            if os.getppid() != parent_pid:
                raise ExitObservationError("parent changed during observation setup")
            if os.write(ready_fd, READY) != len(READY):
                raise ExitObservationError("observer readiness was not delivered")
            os.close(ready_fd)
            while not (closed and exited):
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise ExitObservationError("approved shutdown timed out")
                observed = queue.control(None, 2, remaining)
                if not observed:
                    raise ExitObservationError("approved shutdown timed out")
                for event in observed:
                    if event.flags & select.KQ_EV_ERROR:
                        raise ExitObservationError("exit observation failed")
                    if event.filter == select.KQ_FILTER_PROC and event.ident == parent_pid:
                        if (not event.fflags & select.KQ_NOTE_EXIT
                                or not event.fflags & NOTE_EXITSTATUS or event.data != 0):
                            raise ExitObservationError("parent did not exit normally")
                        exited = True
                    elif event.filter == select.KQ_FILTER_READ and event.ident == approval_fd:
                        chunk = os.read(approval_fd, len(APPROVAL) + 1 - len(approval))
                        if not chunk:
                            closed = True
                            queue.control([select.kevent(approval_fd, filter=select.KQ_FILTER_READ,
                                                         flags=select.KQ_EV_DELETE)], 0, 0)
                        else:
                            approval.extend(chunk)
                            if len(approval) > len(APPROVAL) or not APPROVAL.startswith(approval):
                                raise ExitObservationError("invalid shutdown approval")
                    else:
                        raise ExitObservationError("unexpected observer event")
            if approval != APPROVAL:
                raise ExitObservationError("shutdown was not approved")
        except (OSError, subprocess.SubprocessError) as error:
            raise ExitObservationError("could not observe approved shutdown") from error
