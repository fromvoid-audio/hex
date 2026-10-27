"""Fixed signed-helper entry: accept one bounded update request, then wait for quit.

The enclosing helper bundle and its scripts must be copied to private custody
and verified before execution. This entry never reads the source checkout,
compiles code, opens a shell, or accepts an executable/script path.
"""
from __future__ import annotations

import importlib.util
import json
import os
from pathlib import Path
import re
import select
import stat
import subprocess
import sys
import time

MAX_REQUEST = 4096
HELPER_ID = "org.portalsurfer.hex.update"


def refuse(message: str) -> None:
    raise RuntimeError(message)


def read_request(timeout: float = 15.0) -> dict:
    deadline = time.monotonic() + timeout
    data = bytearray()
    while len(data) <= MAX_REQUEST:
        remaining = deadline - time.monotonic()
        if remaining <= 0 or not select.select([sys.stdin.fileno()], [], [], remaining)[0]:
            refuse("update request timed out")
        byte = os.read(sys.stdin.fileno(), 1)
        if not byte:
            refuse("update request ended before completion")
        if byte == b"\n":
            break
        data.extend(byte)
    else:
        refuse("update request is too large")
    try:
        request = json.loads(data)
    except (ValueError, UnicodeDecodeError) as error:
        raise RuntimeError("invalid update request") from error
    if (not isinstance(request, dict) or set(request) != {"parent_pid", "target", "selected_receipt", "action"}
            or type(request["parent_pid"]) is not int or request["parent_pid"] <= 1
            or not isinstance(request["target"], str) or not Path(request["target"]).is_absolute()
            or Path(request["target"]).name.casefold() != "hex.app"
            or not isinstance(request["selected_receipt"], str)
            or re.fullmatch(r"[0-9a-f]{64}", request["selected_receipt"]) is None
            or request["action"] not in ("install", "restart")):
        refuse("invalid update request")
    return request


def runtime_module(resources: Path):
    source = resources / "runtime/scripts/hex-test-build.py"
    spec = importlib.util.spec_from_file_location("hex_update_runtime", source)
    if spec is None or spec.loader is None:
        refuse("sealed update runtime unavailable")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def verify_custody(bundle: Path, target: Path, runtime) -> None:
    session = bundle.parent
    root = session.parent
    if (bundle.name != "hex-update.app" or root != target.parent / ".hex-update-runtime"
            or re.fullmatch(r"[0-9a-f]{32}", session.name) is None):
        refuse("update helper custody is invalid")
    runtime.assert_plain_path(bundle)
    for path in (root, session):
        metadata = path.lstat()
        if (not stat.S_ISDIR(metadata.st_mode) or metadata.st_uid != os.getuid()
                or stat.S_IMODE(metadata.st_mode) != 0o700):
            refuse("update helper custody is not private")
    config = json.loads((runtime.ROOT / "scripts/hex-test-build.json").read_bytes())
    signing = runtime.signing_configuration(config)
    requirement = (f'identifier "{HELPER_ID}" and anchor apple generic and '
                   f'certificate leaf[subject.OU] = "{signing["team_identifier"]}" and '
                   'certificate leaf[field.1.2.840.113635.100.6.1.13] exists and '
                   f'certificate leaf = H"{signing["certificate_sha1"]}"')
    subprocess.run(["/usr/bin/codesign", "--verify", "--strict", "--test-requirement",
                    "=" + requirement, str(bundle)], check=True, timeout=15,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def verify_interpreter(resources: Path, runtime) -> None:
    policy_path = resources / "policy.json"
    runtime.assert_plain_path(policy_path)
    if policy_path.stat().st_size > 4096:
        refuse("invalid runtime policy")
    policy = json.loads(policy_path.read_bytes())
    if (not isinstance(policy, dict)
            or set(policy) != {"version", "interpreter", "interpreter_sha256", "interpreter_device", "interpreter_inode"}
            or type(policy["version"]) is not int or policy["version"] != 1
            or not isinstance(policy["interpreter"], str)
            or not Path(policy["interpreter"]).is_absolute()
            or not isinstance(policy["interpreter_sha256"], str)
            or re.fullmatch(r"[0-9a-f]{64}", policy["interpreter_sha256"]) is None
            or type(policy["interpreter_device"]) is not int or policy["interpreter_device"] < 0
            or type(policy["interpreter_inode"]) is not int or policy["interpreter_inode"] <= 0):
        refuse("invalid runtime policy")
    expected = Path(policy["interpreter"])
    runtime.assert_plain_path(expected)
    before = expected.stat()
    process = subprocess.run(["/bin/ps", "-ww", "-p", str(os.getpid()), "-o", "comm="],
                             capture_output=True, text=True, check=True, timeout=5)
    lines = process.stdout.splitlines()
    if len(lines) != 1 or not lines[0].strip().startswith("/"):
        refuse("running interpreter is unavailable")
    current = Path(lines[0].strip()).stat()
    if (not stat.S_ISREG(before.st_mode) or before.st_size > 128 * 1024 * 1024
            or (before.st_dev, before.st_ino) != (policy["interpreter_device"], policy["interpreter_inode"])
            or (current.st_dev, current.st_ino) != (before.st_dev, before.st_ino)
            or runtime.sha(expected) != policy["interpreter_sha256"]):
        refuse("running interpreter does not match the sealed policy")
    after = expected.stat()
    if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != (
            after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns):
        refuse("running interpreter changed during verification")


def verify_selection(target: Path, selected: str, runtime) -> None:
    # Deferred publication is only available to a test bundle which already
    # participates in the desktop/installer lifetime lock. Never create the
    # lock here: an older running build would not hold it.
    lock = target.parent / ".hex-test-build.lock"
    runtime.assert_plain_path(lock)
    try:
        lock_metadata = lock.lstat()
    except FileNotFoundError as error:
        raise RuntimeError("installed app has no update custody") from error
    if not stat.S_ISREG(lock_metadata.st_mode):
        refuse("installed app has invalid update custody")
    installed = runtime.verify(target)
    if (installed.get("schema") != 2 or installed.get("profile") != "release"
            or installed.get("app_path") != str(target)):
        refuse("installed app is not an update-capable test bundle")
    if not (target.parent / ".hex-pending-update").exists():
        refuse("pending update unavailable")
    pending = runtime.pending_candidate(target)
    with runtime.activation_lock(pending):
        receipt_path = pending / "Contents/Resources/test-build.json"
        runtime.assert_plain_path(receipt_path)
        if not receipt_path.is_file() or runtime.sha(receipt_path) != selected:
            refuse("pending update selection changed")
        receipt = runtime.verify(pending)
        target_identity = runtime.pending_target_identity(target)
        if (receipt.get("schema") != 2 or receipt.get("app_path") != str(target)
                or receipt.get("profile") != "release"
                or receipt.get("verification") != {"mode": "full-ci", "tests": "passed"}
                or target_identity != receipt.get("pending_target_identity")):
            refuse("pending update does not match the installed app")
        old = target_identity.get("signature")
        new = runtime.native_signature(pending / "Contents/MacOS/hex")
        if not isinstance(old, dict):
            refuse("installed app signing identity unavailable")
        if old.get("team_identifier") not in (None, "not set") and (
                not old.get("designated_requirement")
                or old.get("team_identifier") != new.get("team_identifier")
                or old.get("designated_requirement") != new.get("designated_requirement")):
            refuse("pending update changes the installed signing requirement")


def write_result(session: Path, status: str, detail: str = "") -> None:
    # Only write an owned private session, with exclusive creation. The app can
    # report this bounded status on its next launch; no project data is written.
    path = session / "result.json"
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "w") as output:
        json.dump({"version": 1, "status": status, "detail": detail[:512]}, output)
        output.write("\n")
        output.flush()
        os.fsync(output.fileno())


def main() -> int:
    bundle = Path(__file__).resolve().parents[2]
    resources = bundle / "Contents/Resources"
    session = bundle.parent
    custody_verified = False
    installed = False
    runtime = None
    try:
        request = read_request()
        target = Path(request["target"])
        runtime = runtime_module(resources)
        verify_custody(bundle, target, runtime)
        custody_verified = True
        verify_interpreter(resources, runtime)
        verify_selection(target, request["selected_receipt"], runtime)
        observer_path = resources / "runtime/scripts/pending_update_exit.py"
        spec = importlib.util.spec_from_file_location("hex_update_exit", observer_path)
        if spec is None or spec.loader is None:
            refuse("shutdown observer unavailable")
        observer = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(observer)
        observer.observe_parent_exit(request["parent_pid"], sys.stdin.fileno(), sys.stdout.fileno(),
                                     target / "Contents/MacOS/hex")
        runtime._apply_verified_pending(target, request["selected_receipt"])
        installed = True
        if request["action"] == "restart":
            subprocess.run(["/usr/bin/open", "-a", str(target)], check=True, timeout=15,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            write_result(session, "restart_requested")
        else:
            write_result(session, "installed")
        return 0
    except Exception as error:  # process boundary: report refusal without fabricating success
        if custody_verified:
            try:
                if runtime is not None and isinstance(error, getattr(runtime, "PublicationUncertain", ())):
                    status = "publication_uncertain"
                elif runtime is not None and isinstance(error, getattr(runtime, "PublicationFollowupFailed", ())):
                    status = "installed_followup_failed"
                else:
                    status = "installed_restart_failed" if installed else "refused"
                write_result(session, status, str(error))
            except OSError:
                pass
        print("update helper refused: " + str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
